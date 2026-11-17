use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use haggle::config::{Config, ConfigError, DEFAULT_LISTEN, LIVE_DB_ROOT, ensure_scratch_path};

const DB: (&str, &str) = ("HAGGLE_DB", "/var/lib/haggle/haggle.db");
const NTFY_URL: &str = "https://ntfy.example.com/haggle";

/// Builds a config from `pairs`, standing in for the process environment.
fn config_from(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
    let vars: HashMap<String, String> = pairs
        .iter()
        .map(|&(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    Config::from_vars(|name| vars.get(name).cloned())
}

/// Builds a config that must be accepted, failing the test otherwise.
fn config_ok(pairs: &[(&str, &str)]) -> Config {
    match config_from(pairs) {
        Ok(config) => config,
        Err(error) => panic!("expected {pairs:?} to be accepted, got: {error}"),
    }
}

/// Returns the error for `pairs`, failing the test if the config is accepted.
fn config_err(pairs: &[(&str, &str)]) -> ConfigError {
    match config_from(pairs) {
        Ok(_) => panic!("expected {pairs:?} to be rejected"),
        Err(error) => error,
    }
}

/// The `Invalid` error for variable `name` holding `value`, with `reason`.
fn invalid(name: &'static str, value: &str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        name,
        value: value.to_owned(),
        reason: reason.to_owned(),
    }
}

/// Asserts that `path` is refused as the live database, reporting the path as given.
fn assert_refused(path: &str) {
    assert_eq!(
        ensure_scratch_path(Path::new(path)),
        Err(ConfigError::LiveDatabase(PathBuf::from(path))),
        "{path} should be refused"
    );
}

/// Asserts that `path` is accepted as a scratch database.
fn assert_allowed(path: &str) {
    assert_eq!(
        ensure_scratch_path(Path::new(path)),
        Ok(()),
        "{path} should be allowed"
    );
}

#[test]
fn default_listen_is_loopback_port_8421() {
    assert_eq!(DEFAULT_LISTEN, "127.0.0.1:8421");
}

#[test]
fn live_database_root_is_mnt_database() {
    assert_eq!(LIVE_DB_ROOT, "/mnt/database");
}

#[test]
fn only_database_path_gives_default_listen_and_no_ntfy() {
    let config = config_ok(&[DB]);
    assert_eq!(config.db_path, PathBuf::from("/var/lib/haggle/haggle.db"));
    assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 8421)));
    assert!(config.ntfy.is_none());
}

#[test]
fn missing_database_variable_is_reported_by_name() {
    assert_eq!(config_err(&[]), ConfigError::Missing("HAGGLE_DB"));
}

#[test]
fn empty_database_variable_counts_as_missing() {
    assert_eq!(
        config_err(&[("HAGGLE_DB", "")]),
        ConfigError::Missing("HAGGLE_DB")
    );
}

#[test]
fn whitespace_only_database_variable_counts_as_missing() {
    assert_eq!(
        config_err(&[("HAGGLE_DB", "  ")]),
        ConfigError::Missing("HAGGLE_DB")
    );
}

#[test]
fn listen_variable_sets_the_bind_address() {
    let config = config_ok(&[DB, ("HAGGLE_LISTEN", "192.168.5.21:8421")]);
    assert_eq!(config.listen, SocketAddr::from(([192, 168, 5, 21], 8421)));
}

#[test]
fn empty_listen_variable_falls_back_to_default() {
    let config = config_ok(&[DB, ("HAGGLE_LISTEN", "")]);
    assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 8421)));
}

#[test]
fn whitespace_only_listen_variable_falls_back_to_default() {
    let config = config_ok(&[DB, ("HAGGLE_LISTEN", "   ")]);
    assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 8421)));
}

#[test]
fn listen_that_is_not_a_socket_address_is_invalid() {
    assert_eq!(
        config_err(&[DB, ("HAGGLE_LISTEN", "nope")]),
        invalid("HAGGLE_LISTEN", "nope", "not a socket address")
    );
}

#[test]
fn listen_without_a_port_is_invalid() {
    assert_eq!(
        config_err(&[DB, ("HAGGLE_LISTEN", "127.0.0.1")]),
        invalid("HAGGLE_LISTEN", "127.0.0.1", "not a socket address")
    );
}

#[test]
fn listen_with_a_port_above_65535_is_invalid() {
    // 70000 is outside the 0..=65535 port range, so it is not a socket address.
    assert_eq!(
        config_err(&[DB, ("HAGGLE_LISTEN", "127.0.0.1:70000")]),
        invalid("HAGGLE_LISTEN", "127.0.0.1:70000", "not a socket address")
    );
}

#[test]
fn listen_with_a_host_name_is_invalid() {
    // A socket address needs an IP literal; a host name such as localhost is not one.
    assert_eq!(
        config_err(&[DB, ("HAGGLE_LISTEN", "localhost:8421")]),
        invalid("HAGGLE_LISTEN", "localhost:8421", "not a socket address")
    );
}

#[test]
fn https_ntfy_url_and_token_are_both_kept() {
    let config = config_ok(&[
        DB,
        ("HAGGLE_NTFY_URL", NTFY_URL),
        ("HAGGLE_NTFY_TOKEN", "tok-abc"),
    ]);
    let ntfy = config.ntfy.expect("ntfy is configured");
    assert_eq!(ntfy.url.as_str(), NTFY_URL);
    assert_eq!(ntfy.token.as_deref(), Some("tok-abc"));
}

#[test]
fn http_ntfy_url_is_accepted() {
    let config = config_ok(&[DB, ("HAGGLE_NTFY_URL", "http://127.0.0.1:8080/haggle")]);
    let ntfy = config.ntfy.expect("ntfy is configured");
    assert_eq!(ntfy.url.as_str(), "http://127.0.0.1:8080/haggle");
    assert!(ntfy.token.is_none());
}

#[test]
fn ntfy_url_with_ftp_scheme_is_invalid() {
    assert_eq!(
        config_err(&[DB, ("HAGGLE_NTFY_URL", "ftp://ntfy.example.com/haggle")]),
        invalid(
            "HAGGLE_NTFY_URL",
            "ftp://ntfy.example.com/haggle",
            "not an http(s) URL"
        )
    );
}

#[test]
fn ntfy_url_without_a_scheme_is_invalid() {
    assert_eq!(
        config_err(&[DB, ("HAGGLE_NTFY_URL", "ntfy.example.com/haggle")]),
        invalid(
            "HAGGLE_NTFY_URL",
            "ntfy.example.com/haggle",
            "not an http(s) URL"
        )
    );
}

#[test]
fn empty_ntfy_url_leaves_ntfy_unset() {
    assert!(config_ok(&[DB, ("HAGGLE_NTFY_URL", "")]).ntfy.is_none());
}

#[test]
fn whitespace_only_ntfy_url_leaves_ntfy_unset() {
    assert!(config_ok(&[DB, ("HAGGLE_NTFY_URL", "   ")]).ntfy.is_none());
}

#[test]
fn token_without_url_leaves_ntfy_unset() {
    assert!(
        config_ok(&[DB, ("HAGGLE_NTFY_TOKEN", "tok-abc")])
            .ntfy
            .is_none()
    );
}

#[test]
fn empty_token_with_url_leaves_token_unset() {
    let config = config_ok(&[DB, ("HAGGLE_NTFY_URL", NTFY_URL), ("HAGGLE_NTFY_TOKEN", "")]);
    let ntfy = config.ntfy.expect("ntfy is configured");
    assert!(ntfy.token.is_none());
}

#[test]
fn whitespace_only_token_with_url_leaves_token_unset() {
    let config = config_ok(&[
        DB,
        ("HAGGLE_NTFY_URL", NTFY_URL),
        ("HAGGLE_NTFY_TOKEN", "  "),
    ]);
    let ntfy = config.ntfy.expect("ntfy is configured");
    assert!(ntfy.token.is_none());
}

#[test]
fn debug_output_of_config_does_not_show_the_token() {
    let config = config_ok(&[
        DB,
        ("HAGGLE_NTFY_URL", NTFY_URL),
        ("HAGGLE_NTFY_TOKEN", "s3cret"),
    ]);
    let shown = format!("{config:?}");
    assert!(!shown.contains("s3cret"), "token leaked into {shown}");
}

#[test]
fn debug_output_of_ntfy_shows_the_token_as_stars() {
    let config = config_ok(&[
        DB,
        ("HAGGLE_NTFY_URL", NTFY_URL),
        ("HAGGLE_NTFY_TOKEN", "s3cret"),
    ]);
    let ntfy = config.ntfy.expect("ntfy is configured");
    assert!(
        format!("{ntfy:?}").contains(r#"token: Some("***")"#),
        "token should be masked"
    );
}

#[test]
fn invalid_ntfy_error_does_not_show_the_token() {
    let error = config_err(&[
        DB,
        ("HAGGLE_NTFY_URL", "ftp://ntfy.example.com/haggle"),
        ("HAGGLE_NTFY_TOKEN", "s3cret"),
    ]);
    assert!(!error.to_string().contains("s3cret"));
}

#[test]
fn missing_database_error_message_names_the_variable() {
    assert_eq!(config_err(&[]).to_string(), "HAGGLE_DB is not set");
}

#[test]
fn invalid_listen_error_message_shows_name_value_and_reason() {
    assert_eq!(
        config_err(&[DB, ("HAGGLE_LISTEN", "nope")]).to_string(),
        "HAGGLE_LISTEN=nope: not a socket address"
    );
}

#[test]
fn live_database_error_message_shows_the_path() {
    assert_eq!(
        ConfigError::LiveDatabase(PathBuf::from("/mnt/database")).to_string(),
        "refusing to use the live database at /mnt/database"
    );
}

#[test]
fn live_root_itself_is_refused() {
    assert_refused("/mnt/database");
}

#[test]
fn file_under_live_root_is_refused() {
    assert_refused("/mnt/database/haggle/haggle.db");
}

#[test]
fn dotdot_that_lands_in_live_root_is_refused() {
    // Normalised: /mnt/x/../database/a.db -> /mnt/database/a.db, which is under the live root.
    assert_refused("/mnt/x/../database/a.db");
}

#[test]
fn dot_segment_inside_live_root_path_is_refused() {
    // Normalised: /mnt/./database/a.db -> /mnt/database/a.db.
    assert_refused("/mnt/./database/a.db");
}

#[test]
fn sibling_that_shares_a_name_prefix_is_allowed() {
    // Components are compared whole: "databases" is not "database".
    assert_allowed("/mnt/databases/x");
}

#[test]
fn database_directory_under_another_parent_is_allowed() {
    assert_allowed("/tmp/mnt/database");
}

#[test]
fn dotdot_that_leaves_live_root_is_allowed() {
    // Normalised: /mnt/database/../databases/x.db -> /mnt/databases/x.db, which is outside.
    assert_allowed("/mnt/database/../databases/x.db");
}

#[test]
fn relative_path_is_allowed() {
    assert_allowed("target/test.db");
}
