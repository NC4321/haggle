use haggle::db::{DbError, open};
use rusqlite::{Connection, Params};
use std::path::{Path, PathBuf};

/// A database file under the temp dir. The file and its WAL side files are removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Names the file after this process and `test`, and clears any leftover from an earlier run.
    fn new(test: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("haggle-db-{}-{test}.sqlite", std::process::id()));
        let scratch = Scratch { path };
        scratch.remove_files();
        scratch
    }

    fn remove_files(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(name));
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove_files();
    }
}

/// Opens the database at `path`, failing the test if the open is rejected.
fn open_ok(path: &Path) -> Connection {
    match open(path) {
        Ok(conn) => conn,
        Err(error) => panic!("expected open to succeed, got {error}"),
    }
}

/// Runs a single-integer query with `params` and returns the value.
fn count<P: Params>(conn: &Connection, sql: &str, params: P) -> i64 {
    conn.query_row(sql, params, |row| row.get(0))
        .expect("count query")
}

/// Inserts a watch with only the required columns and returns its id.
fn insert_watch(conn: &Connection, url: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO watches (url, interval_s, next_check_at, created_at) VALUES (?1, 3600, 0, 0)",
        [url],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Inserts an observation for `watch_id` with only the required columns.
fn insert_observation(conn: &Connection, watch_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO observations (watch_id, at, price_minor, currency, availability, source) \
         VALUES (?1, 0, 1999, 'EUR', 'in_stock', 'json_ld')",
        [watch_id],
    )
}

/// Inserts an event for `watch_id` with only the required columns.
fn insert_event(conn: &Connection, watch_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO events (watch_id, at, kind, old_minor, new_minor, currency) \
         VALUES (?1, 0, 'price_drop', 2000, 1500, 'EUR')",
        [watch_id],
    )
}

/// Returns true when a table named `name` exists in the database.
fn table_exists(conn: &Connection, name: &str) -> bool {
    count(
        conn,
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
    ) == 1
}

/// Returns true when `table` has an index whose key columns are exactly `columns`, in order.
fn has_index(conn: &Connection, table: &str, columns: &[&str]) -> bool {
    let index_names: Vec<String> = conn
        .prepare(&format!("PRAGMA index_list({table})"))
        .expect("prepare index_list")
        .query_map([], |row| row.get(1))
        .expect("query index_list")
        .map(|name| name.expect("index name"))
        .collect();
    index_names.iter().any(|index| {
        let key_columns: Vec<String> = conn
            .prepare(&format!("PRAGMA index_info({index})"))
            .expect("prepare index_info")
            .query_map([], |row| row.get(2))
            .expect("query index_info")
            .map(|name| name.expect("column name"))
            .collect();
        key_columns
            .iter()
            .map(String::as_str)
            .eq(columns.iter().copied())
    })
}

#[test]
fn open_creates_the_database_file() {
    let scratch = Scratch::new("open_creates_the_database_file");
    assert!(
        !scratch.path.exists(),
        "the file should not exist before open"
    );

    let _conn = open_ok(&scratch.path);

    assert!(scratch.path.exists(), "open should create the file");
}

#[test]
fn open_sets_journal_mode_to_wal() {
    let scratch = Scratch::new("open_sets_journal_mode_to_wal");
    let conn = open_ok(&scratch.path);

    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal_mode");

    assert_eq!(mode, "wal");
}

#[test]
fn open_turns_foreign_keys_on() {
    let scratch = Scratch::new("open_turns_foreign_keys_on");
    let conn = open_ok(&scratch.path);

    let enabled: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("foreign_keys");

    assert_eq!(enabled, 1);
}

#[test]
fn open_sets_busy_timeout_to_5000_ms() {
    let scratch = Scratch::new("open_sets_busy_timeout_to_5000_ms");
    let conn = open_ok(&scratch.path);

    let timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("busy_timeout");

    assert_eq!(timeout, 5000);
}

#[test]
fn new_database_has_schema_version_one() {
    let scratch = Scratch::new("new_database_has_schema_version_one");
    let conn = open_ok(&scratch.path);

    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .expect("schema_version");

    assert_eq!(version, 1);
}

#[test]
fn open_creates_watches_observations_and_events_tables() {
    let scratch = Scratch::new("open_creates_watches_observations_and_events_tables");
    let conn = open_ok(&scratch.path);

    assert!(table_exists(&conn, "watches"), "watches table");
    assert!(table_exists(&conn, "observations"), "observations table");
    assert!(table_exists(&conn, "events"), "events table");
}

#[test]
fn open_creates_the_indexes_the_spec_names() {
    let scratch = Scratch::new("open_creates_the_indexes_the_spec_names");
    let conn = open_ok(&scratch.path);

    assert!(
        has_index(&conn, "observations", &["watch_id", "at"]),
        "index on observations(watch_id, at)"
    );
    assert!(
        has_index(&conn, "events", &["notified_at"]),
        "index on events(notified_at)"
    );
    assert!(
        has_index(&conn, "watches", &["next_check_at"]),
        "index on watches(next_check_at)"
    );
}

#[test]
fn new_watch_defaults_to_enabled_ok_with_no_failures() {
    let scratch = Scratch::new("new_watch_defaults_to_enabled_ok_with_no_failures");
    let conn = open_ok(&scratch.path);
    let id = insert_watch(&conn, "https://shop.example/item").expect("insert watch");

    let (enabled, status, failures): (i64, String, i64) = conn
        .query_row(
            "SELECT enabled, status, failures FROM watches WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("select defaults");

    // The spec gives the defaults: enabled 1, status 'ok', failures 0.
    assert_eq!(enabled, 1);
    assert_eq!(status, "ok");
    assert_eq!(failures, 0);
}

#[test]
fn watches_table_rejects_text_in_an_integer_column() {
    let scratch = Scratch::new("watches_table_rejects_text_in_an_integer_column");
    let conn = open_ok(&scratch.path);

    // watches is a STRICT table, so text cannot go into interval_s (INTEGER).
    let result = conn.execute(
        "INSERT INTO watches (url, interval_s, next_check_at, created_at) \
         VALUES ('https://shop.example/item', 'hourly', 0, 0)",
        [],
    );

    assert!(result.is_err());
}

#[test]
fn reopening_keeps_schema_version_one_with_one_row() {
    let scratch = Scratch::new("reopening_keeps_schema_version_one_with_one_row");
    drop(open_ok(&scratch.path));

    let conn = open_ok(&scratch.path);

    assert_eq!(count(&conn, "SELECT COUNT(*) FROM schema_version", []), 1);
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .expect("schema_version");
    assert_eq!(version, 1);
}

#[test]
fn insert_duplicate_watch_url_fails() {
    let scratch = Scratch::new("insert_duplicate_watch_url_fails");
    let conn = open_ok(&scratch.path);
    insert_watch(&conn, "https://shop.example/item").expect("first insert");

    let second = insert_watch(&conn, "https://shop.example/item");

    // url is UNIQUE, so a second watch with the same url is refused.
    assert!(second.is_err());
}

#[test]
fn insert_observation_for_missing_watch_fails() {
    let scratch = Scratch::new("insert_observation_for_missing_watch_fails");
    let conn = open_ok(&scratch.path);

    // No watch has id 9999, and foreign keys are on, so the insert is refused.
    let result = insert_observation(&conn, 9999);

    assert!(result.is_err());
}

#[test]
fn deleting_watch_deletes_its_observations() {
    let scratch = Scratch::new("deleting_watch_deletes_its_observations");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/item").expect("insert watch");
    insert_observation(&conn, watch_id).expect("first observation");
    insert_observation(&conn, watch_id).expect("second observation");
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM observations WHERE watch_id = ?1",
            [watch_id]
        ),
        2
    );

    conn.execute("DELETE FROM watches WHERE id = ?1", [watch_id])
        .expect("delete watch");

    // ON DELETE CASCADE removes the watch's observations with it.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM observations WHERE watch_id = ?1",
            [watch_id]
        ),
        0
    );
}

#[test]
fn deleting_watch_deletes_its_events() {
    let scratch = Scratch::new("deleting_watch_deletes_its_events");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/item").expect("insert watch");
    insert_event(&conn, watch_id).expect("insert event");

    conn.execute("DELETE FROM watches WHERE id = ?1", [watch_id])
        .expect("delete watch");

    // ON DELETE CASCADE removes the watch's events with it.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM events WHERE watch_id = ?1",
            [watch_id]
        ),
        0
    );
}

#[test]
fn open_rejects_schema_version_newer_than_binary() {
    let scratch = Scratch::new("open_rejects_schema_version_newer_than_binary");
    {
        let conn = open_ok(&scratch.path);
        conn.execute("UPDATE schema_version SET version = 99", [])
            .expect("set schema_version to 99");
    }

    match open(&scratch.path) {
        Ok(_) => panic!("expected open to reject schema version 99"),
        Err(DbError::Corrupt(message)) => {
            assert_eq!(message, "schema version 99 is newer than this binary");
        }
        Err(other) => panic!("expected DbError::Corrupt, got {other}"),
    }
}
