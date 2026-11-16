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

/// An HTTP client with the haggle User-Agent, timeout and size limit.
pub struct Fetcher {
    #[allow(dead_code)]
    client: reqwest::Client,
    #[allow(dead_code)]
    max_bytes: usize,
}

impl Fetcher {
    /// Builds a fetcher with the given request `timeout` and `max_bytes` page limit.
    ///
    /// Returns `FetchError::Network` when the HTTP client cannot be built.
    pub fn new(_timeout: Duration, _max_bytes: usize) -> Result<Fetcher, FetchError> {
        unimplemented!("Fetcher::new")
    }
}

/// Fetches `url`, sending `previous` validators so an unchanged page gives `Fetched::NotModified`.
///
/// Returns `FetchError` when the site blocks or rate limits the request,
/// answers with another error status, the network fails, or the page
/// exceeds the size limit.
pub async fn fetch(
    _client: &Fetcher,
    _url: &url::Url,
    _previous: Option<&Validators>,
) -> Result<Fetched, FetchError> {
    unimplemented!("fetch")
}
