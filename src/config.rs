//! Environment and flags into one `Config`, with validation.

use std::env;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

/// The address the HTTP listener binds to when `HAGGLE_LISTEN` is not set.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8421";

/// The host of the SMTP server that sends the alert emails.
pub const SMTP_HOST: &str = "smtp.gmail.com";

/// The SMTP submission port of `SMTP_HOST`, which upgrades to TLS with STARTTLS.
pub const SMTP_PORT: u16 = 587;

/// The directory holding the live database; tests and the demo must never use it.
pub const LIVE_DB_ROOT: &str = "/mnt/database";

const DB_VAR: &str = "HAGGLE_DB";
const LISTEN_VAR: &str = "HAGGLE_LISTEN";
const GMAIL_USER_VAR: &str = "GMAIL_USER";
const GMAIL_PASSWORD_VAR: &str = "GMAIL_APP_PASSWORD";
const ALERT_EMAIL_VAR: &str = "ALERT_EMAIL";
const HIDDEN_PASSWORD: &str = "***";
const NOT_SOCKET_ADDRESS: &str = "not a socket address";
const NOT_EMAIL_ADDRESS: &str = "not an email address";
const PATH_VAR: &str = "path";

/// The validated runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Path of the SQLite database file.
    pub db_path: PathBuf,
    /// Address the HTTP listener binds to.
    pub listen: SocketAddr,
    /// Email settings, or `None` when no email variable is configured.
    pub email: Option<EmailConfig>,
}

/// Settings for sending alert emails through Gmail.
#[derive(Clone, PartialEq, Eq)]
pub struct EmailConfig {
    /// The Gmail address that logs in and sends the emails.
    pub user: String,
    /// The Gmail app password of `user`.
    pub password: String,
    /// The address that receives the alerts.
    pub to: String,
}

impl fmt::Debug for EmailConfig {
    /// Formats the settings with the password masked, so it never reaches logs or errors.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmailConfig")
            .field("user", &self.user)
            .field("password", &HIDDEN_PASSWORD)
            .field("to", &self.to)
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
    /// the email settings `GMAIL_USER`, `GMAIL_APP_PASSWORD` and `ALERT_EMAIL` are all
    /// optional, but must be set together.
    /// Returns `ConfigError::Missing` for an unset `HAGGLE_DB`, or for the first unset
    /// email variable when only some of them are set, and `ConfigError::Invalid` for a
    /// listen address that does not parse or a `GMAIL_USER` or `ALERT_EMAIL` that is not
    /// an email address. The password never appears in an error.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let db_path = PathBuf::from(required(&get, DB_VAR)?);
        let listen: SocketAddr = match optional(&get, LISTEN_VAR) {
            Some(text) => parse_listen(&text)?,
            None => DEFAULT_LISTEN
                .parse()
                .expect("DEFAULT_LISTEN is a socket address"),
        };
        let email = email_from_vars(&get)?;
        Ok(Config {
            db_path,
            listen,
            email,
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

/// Builds the email settings from the variables, or `None` when none of them is set.
fn email_from_vars(
    get: &impl Fn(&str) -> Option<String>,
) -> Result<Option<EmailConfig>, ConfigError> {
    let user = optional(get, GMAIL_USER_VAR);
    let password = optional(get, GMAIL_PASSWORD_VAR);
    let to = optional(get, ALERT_EMAIL_VAR);
    let (user, password, to) = match (user, password, to) {
        (None, None, None) => return Ok(None),
        (Some(user), Some(password), Some(to)) => (user, password, to),
        (None, _, _) => return Err(ConfigError::Missing(GMAIL_USER_VAR)),
        (_, None, _) => return Err(ConfigError::Missing(GMAIL_PASSWORD_VAR)),
        (_, _, None) => return Err(ConfigError::Missing(ALERT_EMAIL_VAR)),
    };
    ensure_email_address(GMAIL_USER_VAR, &user)?;
    ensure_email_address(ALERT_EMAIL_VAR, &to)?;
    Ok(Some(EmailConfig { user, password, to }))
}

/// Checks that `value` has one `@` with text on both sides and no whitespace.
///
/// Returns `ConfigError::Invalid` for variable `name` when it does not.
fn ensure_email_address(name: &'static str, value: &str) -> Result<(), ConfigError> {
    let looks_like_address = !value.contains(char::is_whitespace)
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@')
        });
    if looks_like_address {
        Ok(())
    } else {
        Err(invalid(name, value, NOT_EMAIL_ADDRESS))
    }
}

/// Parses a listen address, failing with `ConfigError::Invalid` when it is not one.
fn parse_listen(text: &str) -> Result<SocketAddr, ConfigError> {
    text.parse()
        .map_err(|_| invalid(LISTEN_VAR, text, NOT_SOCKET_ADDRESS))
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
