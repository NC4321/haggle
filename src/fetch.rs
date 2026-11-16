//! HTTP fetching of product pages with conditional GET.

use std::time::Duration;

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

/// An HTTP client with the haggle User-Agent, timeout and size limit.
pub struct Fetcher {
    client: reqwest::Client,
    max_bytes: usize,
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
        Ok(Fetcher { client, max_bytes })
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
    check_status(response.status())?;
    let too_large = response
        .content_length()
        .is_some_and(|length| length > client.max_bytes as u64);
    if too_large {
        return Err(FetchError::TooLarge);
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
