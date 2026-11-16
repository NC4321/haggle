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
