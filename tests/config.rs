use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use haggle::config::{
    Config, ConfigError, DEFAULT_LISTEN, LIVE_DB_ROOT, SMTP_HOST, SMTP_PORT, ensure_scratch_path,
};

const DB: (&str, &str) = ("HAGGLE_DB", "/var/lib/haggle/haggle.db");
const USER: (&str, &str) = ("GMAIL_USER", "haggle@example.com");
const APP_PASSWORD: (&str, &str) = ("GMAIL_APP_PASSWORD", SECRET);
const ALERT: (&str, &str) = ("ALERT_EMAIL", "me@example.com");

/// The Gmail app password used in the email tests. It must never appear in an error or a Debug
/// output.
const SECRET: &str = "s3cret-app-pass";

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
fn smtp_host_is_gmail() {
    assert_eq!(SMTP_HOST, "smtp.gmail.com");
}

#[test]
fn smtp_port_is_587() {
    assert_eq!(SMTP_PORT, 587);
}

#[test]
fn only_database_path_gives_default_listen_and_no_email() {
    let config = config_ok(&[DB]);
    assert_eq!(config.db_path, PathBuf::from("/var/lib/haggle/haggle.db"));
    assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 8421)));
    assert!(config.email.is_none());
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
fn all_three_email_variables_give_the_email_settings() {
    let config = config_ok(&[DB, USER, APP_PASSWORD, ALERT]);
    let email = config.email.expect("email is configured");
    assert_eq!(email.user, "haggle@example.com");
    assert_eq!(email.password, SECRET);
    assert_eq!(email.to, "me@example.com");
}

#[test]
fn email_values_are_trimmed() {
    let config = config_ok(&[
        DB,
        ("GMAIL_USER", " haggle@example.com "),
        ("GMAIL_APP_PASSWORD", " s3cret-app-pass\t"),
        ("ALERT_EMAIL", "me@example.com  "),
    ]);
    let email = config.email.expect("email is configured");
    assert_eq!(email.user, "haggle@example.com");
    assert_eq!(email.password, SECRET);
    assert_eq!(email.to, "me@example.com");
}

#[test]
fn missing_gmail_user_is_reported_by_name() {
    assert_eq!(
        config_err(&[DB, APP_PASSWORD, ALERT]),
        ConfigError::Missing("GMAIL_USER")
    );
}

#[test]
fn missing_app_password_is_reported_by_name() {
    assert_eq!(
        config_err(&[DB, USER, ALERT]),
        ConfigError::Missing("GMAIL_APP_PASSWORD")
    );
}

#[test]
fn missing_alert_email_is_reported_by_name() {
    assert_eq!(
        config_err(&[DB, USER, APP_PASSWORD]),
        ConfigError::Missing("ALERT_EMAIL")
    );
}

#[test]
fn when_several_email_variables_are_missing_the_first_in_order_is_reported() {
    // GMAIL_USER comes first in the order GMAIL_USER, GMAIL_APP_PASSWORD, ALERT_EMAIL.
    assert_eq!(config_err(&[DB, ALERT]), ConfigError::Missing("GMAIL_USER"));
}

#[test]
fn blank_email_variables_all_count_as_unset() {
    let config = config_ok(&[
        DB,
        ("GMAIL_USER", ""),
        ("GMAIL_APP_PASSWORD", "  "),
        ("ALERT_EMAIL", " "),
    ]);
    assert!(config.email.is_none());
}

#[test]
fn blank_app_password_beside_the_other_email_variables_is_missing() {
    assert_eq!(
        config_err(&[DB, USER, ("GMAIL_APP_PASSWORD", " "), ALERT]),
        ConfigError::Missing("GMAIL_APP_PASSWORD")
    );
}

#[test]
fn alert_email_without_an_at_sign_is_invalid() {
    assert_eq!(
        config_err(&[DB, USER, APP_PASSWORD, ("ALERT_EMAIL", "not-an-address")]),
        invalid("ALERT_EMAIL", "not-an-address", "not an email address")
    );
}

#[test]
fn gmail_user_with_whitespace_is_invalid() {
    assert_eq!(
        config_err(&[DB, ("GMAIL_USER", "a b@c"), APP_PASSWORD, ALERT]),
        invalid("GMAIL_USER", "a b@c", "not an email address")
    );
}

#[test]
fn alert_email_with_two_at_signs_is_invalid() {
    assert_eq!(
        config_err(&[DB, USER, APP_PASSWORD, ("ALERT_EMAIL", "a@b@example.com")]),
        invalid("ALERT_EMAIL", "a@b@example.com", "not an email address")
    );
}

#[test]
fn gmail_user_with_an_empty_local_part_is_invalid() {
    assert_eq!(
        config_err(&[DB, ("GMAIL_USER", "@example.com"), APP_PASSWORD, ALERT]),
        invalid("GMAIL_USER", "@example.com", "not an email address")
    );
}

#[test]
fn alert_email_with_an_empty_domain_is_invalid() {
    assert_eq!(
        config_err(&[DB, USER, APP_PASSWORD, ("ALERT_EMAIL", "me@")]),
        invalid("ALERT_EMAIL", "me@", "not an email address")
    );
}

#[test]
fn debug_output_of_config_does_not_show_the_password() {
    let config = config_ok(&[DB, USER, APP_PASSWORD, ALERT]);
    let shown = format!("{config:?}");
    assert!(!shown.contains(SECRET), "password leaked into {shown}");
}

#[test]
fn debug_output_of_email_shows_the_password_as_stars() {
    let email = config_ok(&[DB, USER, APP_PASSWORD, ALERT])
        .email
        .expect("email is configured");
    let shown = format!("{email:?}");
    assert!(
        shown.contains("***"),
        "password should be masked in {shown}"
    );
    assert!(!shown.contains(SECRET), "password leaked into {shown}");
}

#[test]
fn invalid_email_error_does_not_show_the_password() {
    let error = config_err(&[DB, ("GMAIL_USER", "not-an-address"), APP_PASSWORD, ALERT]);
    assert!(!error.to_string().contains(SECRET));
    assert!(!format!("{error:?}").contains(SECRET));
}

#[test]
fn missing_email_error_does_not_show_the_password() {
    let error = config_err(&[DB, APP_PASSWORD, ALERT]);
    assert!(!error.to_string().contains(SECRET));
    assert!(!format!("{error:?}").contains(SECRET));
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
