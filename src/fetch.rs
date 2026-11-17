//! HTTP fetching of product pages with conditional GET, robots.txt rules and polite per-host pacing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// The User-Agent header sent with every request.
pub const USER_AGENT: &str = concat!(
    "haggle/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/NC4321/haggle)"
);

/// Default request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Default maximum accepted page size in bytes (5 MiB).
pub const DEFAULT_MAX_BYTES: usize = 5 * 1024 * 1024;

/// Cache validators of a previously fetched page, used for conditional GET.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// The outcome of a successful fetch.
#[derive(Debug)]
pub enum Fetched {
    Body {
        html: String,
        validators: Validators,
    },
    NotModified,
}

/// Why a page could not be fetched.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FetchError {
    #[error("blocked by the site")]
    Blocked,
    #[error("rate limited by the site")]
    RateLimited,
    #[error("HTTP status {0}")]
    Http(u16),
    #[error("network error: {0}")]
    Network(String),
    #[error("page larger than the size limit")]
    TooLarge,
}

/// Connect timeout cap, lowered to the request timeout when that is smaller.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum number of redirects followed per request.
const MAX_REDIRECTS: usize = 5;

/// The Accept header sent with every request.
const ACCEPT_HEADER: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8";

/// Largest part of a robots.txt body that is read; the rest is ignored.
const MAX_ROBOTS_BYTES: usize = 500 * 1024;

/// Path of the robots.txt file at the root of an origin.
const ROBOTS_PATH: &str = "/robots.txt";

/// Per-host politeness settings.
#[derive(Debug, Clone, Copy)]
pub struct Politeness {
    /// Minimum time between two requests to the same host.
    pub host_interval: Duration,
    /// How long a fetched robots.txt is reused.
    pub robots_ttl: Duration,
    /// Minimum time a host is left alone after answering 429.
    pub rate_limit_pause: Duration,
}

impl Default for Politeness {
    fn default() -> Politeness {
        Politeness {
            host_interval: Duration::from_secs(5),
            robots_ttl: Duration::from_secs(24 * 3600),
            rate_limit_pause: Duration::from_secs(3600),
        }
    }
}

/// What is known about one host: when it was last asked and whether it asked us to stop.
#[derive(Default)]
struct HostState {
    last_request: Option<Instant>,
    paused_until: Option<Instant>,
}

/// A fetched robots.txt; an empty body allows everything.
struct CachedRobots {
    body: String,
    fetched_at: Instant,
}

/// Politeness state shared by all requests of a fetcher, keyed by host or origin.
#[derive(Default)]
struct PoliteState {
    hosts: HashMap<String, HostState>,
    robots: HashMap<String, CachedRobots>,
}

/// An HTTP client with the haggle User-Agent, timeout and size limit.
pub struct Fetcher {
    client: reqwest::Client,
    max_bytes: usize,
    politeness: Politeness,
    state: Mutex<PoliteState>,
}

impl Fetcher {
    /// Builds a fetcher with the given request `timeout` and `max_bytes` page limit.
    ///
    /// Returns `FetchError::Network` when the HTTP client cannot be built.
    pub fn new(timeout: Duration, max_bytes: usize) -> Result<Fetcher, FetchError> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .connect_timeout(CONNECT_TIMEOUT.min(timeout))
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(network_error)?;
        Ok(Fetcher {
            client,
            max_bytes,
            politeness: Politeness::default(),
            state: Mutex::new(PoliteState::default()),
        })
    }

    /// Replaces the politeness settings (`Fetcher::new` uses `Politeness::default()`).
    pub fn with_politeness(mut self, politeness: Politeness) -> Fetcher {
        self.politeness = politeness;
        self
    }

    /// When requests to `url`'s host may start again after a 429, if they are paused.
    ///
    /// Returns `None` when the host is not paused, the pause is over, or `url` has no host.
    pub async fn paused_until(&self, url: &url::Url) -> Option<Instant> {
        let host_key = host_key(url).ok()?;
        let state = self.state.lock().await;
        state
            .hosts
            .get(&host_key)?
            .paused_until
            .filter(|&until| until > Instant::now())
    }

    /// Waits until `host_interval` has passed since the previous request to `host_key`.
    ///
    /// The request slot is reserved before sleeping, so concurrent callers queue up one interval apart.
    async fn wait_for_turn(&self, host_key: &str) {
        let start = {
            let mut state = self.state.lock().await;
            let host = state.hosts.entry(host_key.to_owned()).or_default();
            let now = Instant::now();
            let start = host
                .last_request
                .map_or(now, |last| (last + self.politeness.host_interval).max(now));
            host.last_request = Some(start);
            start
        };
        tokio::time::sleep(start.saturating_duration_since(Instant::now())).await;
    }

    /// Pauses requests to `host_key` for the configured pause or `retry_after`, whichever is longer.
    async fn pause_host(&self, host_key: &str, retry_after: Option<Duration>) {
        let pause = retry_after
            .unwrap_or_default()
            .max(self.politeness.rate_limit_pause);
        let mut state = self.state.lock().await;
        state
            .hosts
            .entry(host_key.to_owned())
            .or_default()
            .paused_until = Some(Instant::now() + pause);
    }

    /// Whether robots.txt of `url`'s origin allows fetching `url`.
    ///
    /// Uses the cached robots.txt while it is younger than `robots_ttl`, else fetches it.
    async fn robots_allow(&self, url: &url::Url, host_key: &str) -> Result<bool, FetchError> {
        let origin = format!("{}://{host_key}", url.scheme());
        let cached = {
            let state = self.state.lock().await;
            state
                .robots
                .get(&origin)
                .filter(|robots| robots.fetched_at.elapsed() < self.politeness.robots_ttl)
                .map(|robots| robots.body.clone())
        };
        let body = match cached {
            Some(body) => body,
            None => self.refresh_robots(&origin, host_key).await?,
        };
        let path_and_query = &url[url::Position::BeforePath..url::Position::AfterQuery];
        Ok(robots_allows(&body, ROBOTS_AGENT, path_and_query))
    }

    /// Fetches and caches the robots.txt of `origin`, returning its body.
    ///
    /// A 4xx answer gives an empty body (allow all). A 5xx answer gives `FetchError::Blocked`
    /// and is not cached. Returns `FetchError::Network` when the request fails.
    async fn refresh_robots(&self, origin: &str, host_key: &str) -> Result<String, FetchError> {
        self.wait_for_turn(host_key).await;
        let response = self
            .client
            .get(format!("{origin}{ROBOTS_PATH}"))
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if status.is_server_error() {
            return Err(FetchError::Blocked);
        }
        let body = if status.is_success() {
            let bytes = read_body_prefix(response, MAX_ROBOTS_BYTES).await?;
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            String::new()
        };
        self.state.lock().await.robots.insert(
            origin.to_owned(),
            CachedRobots {
                body: body.clone(),
                fetched_at: Instant::now(),
            },
        );
        Ok(body)
    }
}

/// The key identifying `url`'s host: host name and port.
///
/// Returns `FetchError::Network` when `url` has no host or port.
fn host_key(url: &url::Url) -> Result<String, FetchError> {
    match (url.host_str(), url.port_or_known_default()) {
        (Some(host), Some(port)) => Ok(format!("{host}:{port}")),
        _ => Err(FetchError::Network(format!("URL has no host: {url}"))),
    }
}

fn network_error(error: reqwest::Error) -> FetchError {
    FetchError::Network(error.to_string())
}

fn header_text(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn check_status(status: reqwest::StatusCode) -> Result<(), FetchError> {
    match status.as_u16() {
        403 => Err(FetchError::Blocked),
        429 => Err(FetchError::RateLimited),
        _ if status.is_success() => Ok(()),
        code => Err(FetchError::Http(code)),
    }
}

/// Reads the body in chunks, failing as soon as it exceeds `max_bytes`.
async fn read_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, FetchError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if body.len() + chunk.len() > max_bytes {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads at most `limit` bytes of the body and ignores the rest.
async fn read_body_prefix(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, FetchError> {
    let mut body = Vec::new();
    while body.len() < limit {
        let Some(chunk) = response.chunk().await.map_err(network_error)? else {
            break;
        };
        let room = limit - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
    }
    Ok(body)
}

/// A failed page request, with the delay the site asked for when it answered 429.
struct PageFailure {
    error: FetchError,
    retry_after: Option<Duration>,
}

impl From<FetchError> for PageFailure {
    fn from(error: FetchError) -> PageFailure {
        PageFailure {
            error,
            retry_after: None,
        }
    }
}

/// The Retry-After delay of `response` when given in seconds; the HTTP-date form is ignored.
fn retry_after_delay(response: &reqwest::Response) -> Option<Duration> {
    header_text(response, reqwest::header::RETRY_AFTER)?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}

/// Fetches `url`, sending `previous` validators so an unchanged page gives `Fetched::NotModified`.
///
/// Returns `FetchError` when the site blocks or rate limits the request,
/// answers with another error status, the network fails, or the page
/// exceeds the size limit.
pub async fn fetch(
    client: &Fetcher,
    url: &url::Url,
    previous: Option<&Validators>,
) -> Result<Fetched, FetchError> {
    request_page(client, url, previous)
        .await
        .map_err(|failure| failure.error)
}

/// Fetches `url` politely: robots.txt, host spacing and 429 pauses, then `fetch`.
///
/// Returns `FetchError::RateLimited` without any request while the host is paused after a 429
/// (a new 429 pauses it for the longer of `rate_limit_pause` and Retry-After),
/// `FetchError::Blocked` when robots.txt disallows `url` or cannot be had (5xx),
/// `FetchError::Network` when robots.txt cannot be requested, and otherwise the errors of `fetch`.
pub async fn fetch_page(
    client: &Fetcher,
    url: &url::Url,
    previous: Option<&Validators>,
) -> Result<Fetched, FetchError> {
    let host_key = host_key(url)?;
    if client.paused_until(url).await.is_some() {
        return Err(FetchError::RateLimited);
    }
    if !client.robots_allow(url, &host_key).await? {
        return Err(FetchError::Blocked);
    }
    client.wait_for_turn(&host_key).await;
    match request_page(client, url, previous).await {
        Err(PageFailure {
            error: FetchError::RateLimited,
            retry_after,
        }) => {
            client.pause_host(&host_key, retry_after).await;
            Err(FetchError::RateLimited)
        }
        result => result.map_err(|failure| failure.error),
    }
}

/// The request path shared by `fetch` and `fetch_page`, keeping the Retry-After delay of a 429.
async fn request_page(
    client: &Fetcher,
    url: &url::Url,
    previous: Option<&Validators>,
) -> Result<Fetched, PageFailure> {
    use reqwest::header::{ACCEPT, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

    let mut request = client.client.get(url.clone()).header(ACCEPT, ACCEPT_HEADER);
    if let Some(previous) = previous {
        if let Some(etag) = &previous.etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = &previous.last_modified {
            request = request.header(IF_MODIFIED_SINCE, last_modified);
        }
    }
    let response = request.send().await.map_err(network_error)?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(PageFailure {
            error: FetchError::RateLimited,
            retry_after: retry_after_delay(&response),
        });
    }
    check_status(response.status())?;
    let too_large = response
        .content_length()
        .is_some_and(|length| length > client.max_bytes as u64);
    if too_large {
        return Err(FetchError::TooLarge.into());
    }
    let validators = Validators {
        etag: header_text(&response, ETAG),
        last_modified: header_text(&response, LAST_MODIFIED),
    };
    let body = read_body(response, client.max_bytes).await?;
    Ok(Fetched::Body {
        html: String::from_utf8_lossy(&body).into_owned(),
        validators,
    })
}

/// The product token matched against robots.txt User-agent lines.
pub const ROBOTS_AGENT: &str = "haggle";

/// Longest delay a failing watch is pushed back.
pub const MAX_BACKOFF: Duration = Duration::from_secs(7 * 24 * 3600);

/// Comment marker in robots.txt; the rest of the line is ignored.
const COMMENT_MARKER: char = '#';

/// Separator between a robots.txt field name and its value.
const FIELD_SEPARATOR: char = ':';

/// robots.txt field names, matched case-insensitively.
const USER_AGENT_FIELD: &str = "user-agent";
const ALLOW_FIELD: &str = "allow";
const DISALLOW_FIELD: &str = "disallow";

/// The User-agent value whose groups apply when no group names the agent.
const WILDCARD_AGENT: &str = "*";

/// Path pattern wildcard matching any run of characters.
const WILDCARD: char = '*';

/// Path pattern suffix anchoring the match to the end of the path.
const END_ANCHOR: char = '$';

/// One Allow or Disallow rule of a robots.txt group.
struct RobotsRule {
    allow: bool,
    pattern: String,
}

/// A robots.txt group: the User-agent values it names and its rules.
struct RobotsGroup {
    agents: Vec<String>,
    rules: Vec<RobotsRule>,
}

impl RobotsGroup {
    /// Whether this group names `agent`, compared case-insensitively.
    fn names(&self, agent: &str) -> bool {
        self.agents
            .iter()
            .any(|name| name.eq_ignore_ascii_case(agent))
    }
}

/// Splits robots.txt into groups, ignoring comments, unknown fields and empty rule values.
///
/// Consecutive User-agent lines share one group; a User-agent line after rules starts a new one.
fn parse_groups(robots_txt: &str) -> Vec<RobotsGroup> {
    let mut groups: Vec<RobotsGroup> = Vec::new();
    let mut in_agent_run = false;
    for line in robots_txt.lines() {
        let content = line
            .split_once(COMMENT_MARKER)
            .map_or(line, |(before_comment, _)| before_comment);
        let Some((field, value)) = content.split_once(FIELD_SEPARATOR) else {
            continue;
        };
        let value = value.trim();
        let name = field.trim().to_ascii_lowercase();
        match name.as_str() {
            USER_AGENT_FIELD => {
                match groups.last_mut() {
                    Some(group) if in_agent_run => group.agents.push(value.to_owned()),
                    _ => groups.push(RobotsGroup {
                        agents: vec![value.to_owned()],
                        rules: Vec::new(),
                    }),
                }
                in_agent_run = true;
            }
            ALLOW_FIELD | DISALLOW_FIELD => {
                in_agent_run = false;
                let Some(group) = groups.last_mut() else {
                    continue;
                };
                if !value.is_empty() {
                    group.rules.push(RobotsRule {
                        allow: name == ALLOW_FIELD,
                        pattern: value.to_owned(),
                    });
                }
            }
            _ => {}
        }
    }
    groups
}

/// The rules that apply to `agent`: those of the groups naming it, else those of the wildcard groups.
///
/// Returns an empty list when no group applies.
fn applicable_rules<'a>(groups: &'a [RobotsGroup], agent: &str) -> Vec<&'a RobotsRule> {
    let named: Vec<&RobotsGroup> = groups.iter().filter(|group| group.names(agent)).collect();
    let selected = if named.is_empty() {
        groups
            .iter()
            .filter(|group| group.names(WILDCARD_AGENT))
            .collect()
    } else {
        named
    };
    selected
        .into_iter()
        .flat_map(|group| group.rules.iter())
        .collect()
}

/// Whether a robots.txt `pattern` matches the start of `path`.
///
/// `*` matches any run of characters; a trailing `$` requires the match to reach the end of `path`.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (body, anchored) = match pattern.strip_suffix(END_ANCHOR) {
        Some(body) => (body, true),
        None => (pattern, false),
    };
    let segments: Vec<&str> = body.split(WILDCARD).collect();
    match segments.as_slice() {
        // `split` always yields at least one segment, so this arm only satisfies the match.
        [] => false,
        [whole] if anchored => path == *whole,
        [whole] => path.starts_with(*whole),
        [first, middle @ .., last] => {
            let Some(mut rest) = path.strip_prefix(*first) else {
                return false;
            };
            for &segment in middle {
                let Some(index) = rest.find(segment) else {
                    return false;
                };
                rest = &rest[index + segment.len()..];
            }
            if anchored {
                rest.ends_with(*last)
            } else {
                rest.contains(*last)
            }
        }
    }
}

/// Whether `path` (path plus optional ?query of a URL) may be fetched by `agent` under `robots_txt`.
///
/// Follows RFC 9309: groups naming `agent` are used, else the `*` groups, else everything is allowed.
/// Among the rules matching `path`, the longest pattern wins and Allow wins a tie.
/// Returns `true` when no rule matches.
pub fn robots_allows(robots_txt: &str, agent: &str, path: &str) -> bool {
    let groups = parse_groups(robots_txt);
    applicable_rules(&groups, agent)
        .into_iter()
        .filter(|rule| pattern_matches(&rule.pattern, path))
        .max_by_key(|rule| (rule.pattern.len(), rule.allow))
        .is_none_or(|rule| rule.allow)
}

/// Delay before the next check after `failures` consecutive failures.
///
/// `interval` doubled `failures` times, capped at `MAX_BACKOFF`, and never below `interval`:
/// `interval` itself when `failures` is 0. Saturates instead of overflowing.
pub fn backoff_delay(interval: Duration, failures: u32) -> Duration {
    let mut delay = interval;
    for _ in 0..failures {
        // Stop once capped; a zero interval never grows and would otherwise loop for every failure.
        if delay >= MAX_BACKOFF || delay.is_zero() {
            break;
        }
        delay = delay.saturating_mul(2);
    }
    delay.min(MAX_BACKOFF).max(interval)
}
