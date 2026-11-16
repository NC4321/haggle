//! Environment and flags into one `Config`, with validation.

use std::env;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use url::Url;

/// The address the HTTP listener binds to when `HAGGLE_LISTEN` is not set.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8421";

/// The directory holding the live database; tests and the demo must never use it.
pub const LIVE_DB_ROOT: &str = "/mnt/database";

const DB_VAR: &str = "HAGGLE_DB";
const LISTEN_VAR: &str = "HAGGLE_LISTEN";
const NTFY_URL_VAR: &str = "HAGGLE_NTFY_URL";
const NTFY_TOKEN_VAR: &str = "HAGGLE_NTFY_TOKEN";
const HTTP_SCHEMES: [&str; 2] = ["http", "https"];
const HIDDEN_TOKEN: &str = "***";
const NOT_SOCKET_ADDRESS: &str = "not a socket address";
const NOT_HTTP_URL: &str = "not an http(s) URL";
const PATH_VAR: &str = "path";

/// The validated runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Path of the SQLite database file.
    pub db_path: PathBuf,
    /// Address the HTTP listener binds to.
    pub listen: SocketAddr,
    /// ntfy settings, or `None` when no ntfy URL is configured.
    pub ntfy: Option<NtfyConfig>,
}

/// Settings for sending notifications to an ntfy server.
#[derive(Clone, PartialEq, Eq)]
pub struct NtfyConfig {
    /// The http or https URL of the ntfy topic.
    pub url: Url,
    /// Access token, or `None` when the server needs none.
    pub token: Option<String>,
}

impl fmt::Debug for NtfyConfig {
    /// Formats the settings with the token masked, so it never reaches logs or errors.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NtfyConfig")
            .field("url", &self.url.as_str())
            .field("token", &self.token.as_ref().map(|_| HIDDEN_TOKEN))
            .finish()
    }
}

/// Why the configuration is unusable.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// A required variable is unset or empty.
    #[error("{0} is not set")]
    Missing(&'static str),
    /// A variable is set, but its value is not acceptable.
    #[error("{name}={value}: {reason}")]
    Invalid {
        name: &'static str,
        value: String,
        reason: String,
    },
    /// A path points into the live database tree.
    #[error("refusing to use the live database at {0}")]
    LiveDatabase(PathBuf),
}

impl Config {
    /// Reads the configuration from the process environment.
    ///
    /// Returns the first `ConfigError` found in the `HAGGLE_*` variables.
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_vars(|name| env::var(name).ok())
    }

    /// Builds the configuration from variables looked up through `get`.
    ///
    /// Values are trimmed, and an empty or whitespace-only value counts as unset.
    /// `HAGGLE_DB` is required, `HAGGLE_LISTEN` defaults to `DEFAULT_LISTEN`, and
    /// `HAGGLE_NTFY_TOKEN` is ignored unless `HAGGLE_NTFY_URL` is set.
    /// Returns `ConfigError::Missing` for an unset `HAGGLE_DB` and
    /// `ConfigError::Invalid` for a listen address or ntfy URL that does not parse.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let db_path = PathBuf::from(required(&get, DB_VAR)?);
        let listen: SocketAddr = match optional(&get, LISTEN_VAR) {
            Some(text) => parse_listen(&text)?,
            None => DEFAULT_LISTEN
                .parse()
                .expect("DEFAULT_LISTEN is a socket address"),
        };
        let ntfy = ntfy_from_vars(&get)?;
        Ok(Config {
            db_path,
            listen,
            ntfy,
        })
    }
}

/// Refuses `path` when it lies inside the live database tree at `LIVE_DB_ROOT`.
///
/// The path is made absolute against the current directory and normalised lexically,
/// with no filesystem access, so `..` components cannot step around the root.
/// Returns `ConfigError::LiveDatabase` carrying the original `path` when it is refused,
/// and `ConfigError::Invalid` when the path cannot be made absolute.
pub fn ensure_scratch_path(path: &Path) -> Result<(), ConfigError> {
    let absolute = std::path::absolute(path).map_err(|error| {
        invalid(
            PATH_VAR,
            path.display().to_string(),
            format!("cannot make absolute: {error}"),
        )
    })?;
    if normalise(&absolute).starts_with(LIVE_DB_ROOT) {
        return Err(ConfigError::LiveDatabase(path.to_path_buf()));
    }
    Ok(())
}

/// Reads `name` through `get`, trimmed; an empty or whitespace-only value is `None`.
fn optional(get: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    get(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Reads the required variable `name`, failing with `ConfigError::Missing` when unset.
fn required(
    get: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    optional(get, name).ok_or(ConfigError::Missing(name))
}

/// Builds the ntfy settings from the variables, or `None` when no URL is set.
fn ntfy_from_vars(
    get: &impl Fn(&str) -> Option<String>,
) -> Result<Option<NtfyConfig>, ConfigError> {
    let Some(text) = optional(get, NTFY_URL_VAR) else {
        return Ok(None);
    };
    let url = parse_ntfy_url(&text)?;
    let token = optional(get, NTFY_TOKEN_VAR);
    Ok(Some(NtfyConfig { url, token }))
}

/// Parses a listen address, failing with `ConfigError::Invalid` when it is not one.
fn parse_listen(text: &str) -> Result<SocketAddr, ConfigError> {
    text.parse()
        .map_err(|_| invalid(LISTEN_VAR, text, NOT_SOCKET_ADDRESS))
}

/// Parses an http or https URL, failing with `ConfigError::Invalid` for any other value.
fn parse_ntfy_url(text: &str) -> Result<Url, ConfigError> {
    match Url::parse(text) {
        Ok(url) if HTTP_SCHEMES.contains(&url.scheme()) => Ok(url),
        _ => Err(invalid(NTFY_URL_VAR, text, NOT_HTTP_URL)),
    }
}

/// Builds a `ConfigError::Invalid` for variable or path `name` holding `value`.
fn invalid(name: &'static str, value: impl Into<String>, reason: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        name,
        value: value.into(),
        reason: reason.into(),
    }
}

/// Resolves `.` and `..` components of an absolute `path` without touching the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut normalised = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalised.pop();
            }
            other => normalised.push(other),
        }
    }
    normalised
}
