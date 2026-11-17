//! Tests for the queries on the `watches` table in `haggle::db::watches`.

use haggle::db::watches;
use haggle::db::{DbError, NewWatch, Watch, WatchStatus, open};
use haggle::money::{Currency, Money};
use rusqlite::{Connection, params};
use std::path::PathBuf;

/// The insert time, in Unix seconds, used by most tests.
const NOW: i64 = 1_700_000_000;
/// One hour in seconds, the interval `new_watch` uses.
const HOUR_S: i64 = 3600;
/// A validator value as a server sends it in an ETag header.
const ETAG: &str = "\"abc123\"";
/// A validator value as a server sends it in a Last-Modified header.
const LAST_MODIFIED: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

/// A database file under the temp dir. The file and its WAL side files are removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Names the file after this process and `test`, and clears any leftover from an earlier run.
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "haggle-watches-{}-{test}.sqlite",
            std::process::id()
        ));
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

/// Opens a fresh migrated database named after `test`, failing the test if the open is rejected.
fn open_scratch(test: &str) -> (Connection, Scratch) {
    let scratch = Scratch::new(test);
    match open(&scratch.path) {
        Ok(conn) => (conn, scratch),
        Err(error) => panic!("expected open to succeed, got {error}"),
    }
}

/// A watch named "Kettle" with a one-hour interval and no selector or target.
fn new_watch(url: &str) -> NewWatch {
    NewWatch {
        url: url.to_string(),
        name: Some("Kettle".to_string()),
        selector: None,
        target: None,
        interval_s: HOUR_S,
    }
}

/// Inserts a `new_watch` for `url` at time `now` and returns its id.
fn insert_at(conn: &Connection, url: &str, now: i64) -> i64 {
    watches::insert(conn, &new_watch(url), now).expect("insert watch")
}

/// Reads the watch with `id`, failing the test if it is missing or the read fails.
fn get_watch(conn: &Connection, id: i64) -> Watch {
    match watches::get(conn, id) {
        Ok(Some(watch)) => watch,
        Ok(None) => panic!("expected watch {id} to exist"),
        Err(error) => panic!("expected get to succeed, got {error}"),
    }
}

/// Returns the ids of `rows` in the order given.
fn ids(rows: &[Watch]) -> Vec<i64> {
    rows.iter().map(|watch| watch.id).collect()
}

/// Inserts a row with raw SQL, so a test can store a value the queries must refuse.
/// Returns the new row id.
fn insert_raw(
    conn: &Connection,
    url: &str,
    target_minor: Option<i64>,
    currency: Option<&str>,
    status: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO watches (url, target_minor, currency, interval_s, status, next_check_at, created_at) \
         VALUES (?1, ?2, ?3, 3600, ?4, 0, 0)",
        params![url, target_minor, currency, status],
    )
    .expect("raw insert");
    conn.last_insert_rowid()
}

#[test]
fn insert_then_get_returns_every_field_as_given() {
    let (conn, _scratch) = open_scratch("insert_then_get_returns_every_field_as_given");
    let new = NewWatch {
        url: "https://shop.example/kettle".to_string(),
        name: Some("Kettle".to_string()),
        selector: Some(".price-now".to_string()),
        target: Some(Money {
            minor: 1299,
            currency: Currency::Eur,
        }),
        interval_s: HOUR_S,
    };
    let id = watches::insert(&conn, &new, NOW).expect("insert watch");

    // Only the fields the caller supplies come from `new`; the rest follow the spec's defaults:
    // status ok, enabled, no validators, no failures, next check and creation at `NOW`.
    assert_eq!(
        get_watch(&conn, id),
        Watch {
            id,
            url: "https://shop.example/kettle".to_string(),
            name: Some("Kettle".to_string()),
            selector: Some(".price-now".to_string()),
            target: Some(Money {
                minor: 1299,
                currency: Currency::Eur,
            }),
            interval_s: HOUR_S,
            enabled: true,
            status: WatchStatus::Ok,
            etag: None,
            last_modified: None,
            next_check_at: NOW,
            failures: 0,
            created_at: NOW,
        }
    );
}

#[test]
fn insert_starts_enabled_with_status_ok_and_no_failures() {
    let (conn, _scratch) = open_scratch("insert_starts_enabled_with_status_ok_and_no_failures");
    let id = insert_at(&conn, "https://shop.example/a", NOW);

    let watch = get_watch(&conn, id);

    assert!(watch.enabled);
    assert_eq!(watch.status, WatchStatus::Ok);
    assert_eq!(watch.failures, 0);
    assert_eq!(watch.etag, None);
    assert_eq!(watch.last_modified, None);
}

#[test]
fn insert_sets_next_check_at_and_created_at_to_now() {
    let (conn, _scratch) = open_scratch("insert_sets_next_check_at_and_created_at_to_now");
    let id = insert_at(&conn, "https://shop.example/a", NOW);

    let watch = get_watch(&conn, id);

    assert_eq!(watch.next_check_at, NOW);
    assert_eq!(watch.created_at, NOW);
}

#[test]
fn insert_with_no_name_selector_or_target_round_trips_none() {
    let (conn, _scratch) = open_scratch("insert_with_no_name_selector_or_target_round_trips_none");
    let new = NewWatch {
        url: "https://shop.example/bare".to_string(),
        name: None,
        selector: None,
        target: None,
        interval_s: HOUR_S,
    };
    let id = watches::insert(&conn, &new, NOW).expect("insert watch");

    let watch = get_watch(&conn, id);

    assert_eq!(watch.name, None);
    assert_eq!(watch.selector, None);
    assert_eq!(watch.target, None);
}

#[test]
fn insert_keeps_the_currency_of_a_zero_decimal_target() {
    let (conn, _scratch) = open_scratch("insert_keeps_the_currency_of_a_zero_decimal_target");
    let new = NewWatch {
        target: Some(Money {
            minor: 15000,
            currency: Currency::Jpy,
        }),
        ..new_watch("https://shop.example/yen")
    };
    let id = watches::insert(&conn, &new, NOW).expect("insert watch");

    // 15000 yen is stored as minor units; JPY has no decimal places, so the minor value is the amount.
    assert_eq!(
        get_watch(&conn, id).target,
        Some(Money {
            minor: 15000,
            currency: Currency::Jpy,
        })
    );
}

#[test]
fn insert_with_a_url_already_watched_returns_duplicate_url_with_that_url() {
    let (conn, _scratch) = open_scratch("insert_with_a_url_already_watched_returns_duplicate_url");
    let url = "https://shop.example/kettle";
    insert_at(&conn, url, NOW);

    match watches::insert(&conn, &new_watch(url), NOW) {
        Err(DbError::DuplicateUrl(reported)) => assert_eq!(reported, url),
        other => panic!("expected DbError::DuplicateUrl, got {other:?}"),
    }
}

#[test]
fn insert_with_a_duplicate_url_leaves_one_row() {
    let (conn, _scratch) = open_scratch("insert_with_a_duplicate_url_leaves_one_row");
    let url = "https://shop.example/kettle";
    insert_at(&conn, url, NOW);

    let _ = watches::insert(&conn, &new_watch(url), NOW);

    let rows = watches::list(&conn).expect("list watches");
    assert_eq!(rows.len(), 1);
}

#[test]
fn find_by_url_returns_the_watch_with_that_url() {
    let (conn, _scratch) = open_scratch("find_by_url_returns_the_watch_with_that_url");
    let url = "https://shop.example/kettle";
    let id = insert_at(&conn, url, NOW);

    let found = watches::find_by_url(&conn, url).expect("find watch");

    assert_eq!(found.map(|watch| watch.id), Some(id));
}

#[test]
fn find_by_url_returns_none_for_a_url_not_watched() {
    let (conn, _scratch) = open_scratch("find_by_url_returns_none_for_a_url_not_watched");
    insert_at(&conn, "https://shop.example/kettle", NOW);

    let found = watches::find_by_url(&conn, "https://shop.example/toaster").expect("find watch");

    assert_eq!(found, None);
}

#[test]
fn get_of_an_unknown_id_returns_none() {
    let (conn, _scratch) = open_scratch("get_of_an_unknown_id_returns_none");
    insert_at(&conn, "https://shop.example/kettle", NOW);

    let found = watches::get(&conn, 9999).expect("get watch");

    assert_eq!(found, None);
}

#[test]
fn list_on_an_empty_database_is_empty() {
    let (conn, _scratch) = open_scratch("list_on_an_empty_database_is_empty");

    let rows = watches::list(&conn).expect("list watches");

    assert!(rows.is_empty());
}

#[test]
fn list_returns_watches_ordered_by_id() {
    let (conn, _scratch) = open_scratch("list_returns_watches_ordered_by_id");
    // The next checks are deliberately out of id order, so the result cannot be sorted by time.
    let first = insert_at(&conn, "https://shop.example/a", 300);
    let second = insert_at(&conn, "https://shop.example/b", 100);
    let third = insert_at(&conn, "https://shop.example/c", 200);

    let rows = watches::list(&conn).expect("list watches");

    assert_eq!(ids(&rows), vec![first, second, third]);
}

#[test]
fn due_includes_a_watch_scheduled_exactly_at_now_and_excludes_later_ones() {
    let (conn, _scratch) = open_scratch("due_includes_exactly_at_now_and_excludes_later");
    let early = insert_at(&conn, "https://shop.example/a", 100);
    let on_time = insert_at(&conn, "https://shop.example/b", 200);
    insert_at(&conn, "https://shop.example/c", 201);

    let rows = watches::due(&conn, 200, 10).expect("due watches");

    assert_eq!(ids(&rows), vec![early, on_time]);
}

#[test]
fn due_orders_by_next_check_at_then_id() {
    let (conn, _scratch) = open_scratch("due_orders_by_next_check_at_then_id");
    let latest = insert_at(&conn, "https://shop.example/a", 300);
    // `earlier` and `tied` share a next check at 100; the lower id must come first.
    let earlier = insert_at(&conn, "https://shop.example/b", 100);
    let tied = insert_at(&conn, "https://shop.example/c", 100);
    let middle = insert_at(&conn, "https://shop.example/d", 200);

    let rows = watches::due(&conn, 300, 10).expect("due watches");

    assert_eq!(ids(&rows), vec![earlier, tied, middle, latest]);
}

#[test]
fn due_returns_at_most_limit_rows() {
    let (conn, _scratch) = open_scratch("due_returns_at_most_limit_rows");
    let first = insert_at(&conn, "https://shop.example/a", 100);
    let second = insert_at(&conn, "https://shop.example/b", 200);
    insert_at(&conn, "https://shop.example/c", 300);

    let rows = watches::due(&conn, 300, 2).expect("due watches");

    assert_eq!(ids(&rows), vec![first, second]);
}

#[test]
fn due_with_limit_zero_returns_no_rows() {
    let (conn, _scratch) = open_scratch("due_with_limit_zero_returns_no_rows");
    insert_at(&conn, "https://shop.example/a", 100);

    let rows = watches::due(&conn, 300, 0).expect("due watches");

    assert!(rows.is_empty());
}

#[test]
fn due_skips_a_watch_disabled_in_the_database() {
    let (conn, _scratch) = open_scratch("due_skips_a_watch_disabled_in_the_database");
    let disabled = insert_at(&conn, "https://shop.example/a", 100);
    let enabled = insert_at(&conn, "https://shop.example/b", 100);
    conn.execute(
        "UPDATE watches SET enabled = 0 WHERE id = ?1",
        params![disabled],
    )
    .expect("disable watch");

    let rows = watches::due(&conn, 100, 10).expect("due watches");

    assert_eq!(ids(&rows), vec![enabled]);
}

#[test]
fn record_success_stores_the_etag_and_last_modified() {
    let (conn, _scratch) = open_scratch("record_success_stores_the_etag_and_last_modified");
    let id = insert_at(&conn, "https://shop.example/a", NOW);

    watches::record_success(&conn, id, Some(ETAG), Some(LAST_MODIFIED), NOW + HOUR_S)
        .expect("record success");

    let watch = get_watch(&conn, id);
    assert_eq!(watch.etag.as_deref(), Some(ETAG));
    assert_eq!(watch.last_modified.as_deref(), Some(LAST_MODIFIED));
}

#[test]
fn record_success_sets_next_check_at() {
    let (conn, _scratch) = open_scratch("record_success_sets_next_check_at");
    let id = insert_at(&conn, "https://shop.example/a", NOW);

    watches::record_success(&conn, id, None, None, NOW + HOUR_S).expect("record success");

    assert_eq!(get_watch(&conn, id).next_check_at, NOW + HOUR_S);
}

#[test]
fn record_success_after_a_failure_resets_status_to_ok_and_failures_to_zero() {
    let (conn, _scratch) =
        open_scratch("record_success_after_a_failure_resets_status_and_failures");
    let id = insert_at(&conn, "https://shop.example/a", NOW);
    watches::record_failure(&conn, id, WatchStatus::Blocked, NOW + 60).expect("record failure");
    watches::record_failure(&conn, id, WatchStatus::Blocked, NOW + 120).expect("record failure");

    watches::record_success(&conn, id, None, None, NOW + HOUR_S).expect("record success");

    let watch = get_watch(&conn, id);
    assert_eq!(watch.status, WatchStatus::Ok);
    assert_eq!(watch.failures, 0);
}

#[test]
fn record_failure_twice_counts_two_failures_and_stores_the_status() {
    let (conn, _scratch) = open_scratch("record_failure_twice_counts_two_failures");
    let id = insert_at(&conn, "https://shop.example/a", NOW);

    watches::record_failure(&conn, id, WatchStatus::Blocked, NOW + 60).expect("record failure");
    watches::record_failure(&conn, id, WatchStatus::Blocked, NOW + 120).expect("record failure");

    let watch = get_watch(&conn, id);
    assert_eq!(watch.failures, 2);
    assert_eq!(watch.status, WatchStatus::Blocked);
    assert_eq!(watch.next_check_at, NOW + 120);
}

#[test]
fn record_failure_keeps_the_etag_and_last_modified_from_the_last_success() {
    let (conn, _scratch) = open_scratch("record_failure_keeps_the_validators");
    let id = insert_at(&conn, "https://shop.example/a", NOW);
    watches::record_success(&conn, id, Some(ETAG), Some(LAST_MODIFIED), NOW + HOUR_S)
        .expect("record success");

    watches::record_failure(&conn, id, WatchStatus::Blocked, NOW + 60).expect("record failure");
    watches::record_failure(&conn, id, WatchStatus::Blocked, NOW + 120).expect("record failure");

    let watch = get_watch(&conn, id);
    assert_eq!(watch.etag.as_deref(), Some(ETAG));
    assert_eq!(watch.last_modified.as_deref(), Some(LAST_MODIFIED));
}

#[test]
fn delete_of_an_existing_watch_returns_true() {
    let (conn, _scratch) = open_scratch("delete_of_an_existing_watch_returns_true");
    let id = insert_at(&conn, "https://shop.example/a", NOW);

    let deleted = watches::delete(&conn, id).expect("delete watch");

    assert!(deleted);
}

#[test]
fn delete_of_the_same_watch_twice_returns_false_the_second_time() {
    let (conn, _scratch) = open_scratch("delete_of_the_same_watch_twice_returns_false");
    let id = insert_at(&conn, "https://shop.example/a", NOW);
    watches::delete(&conn, id).expect("delete watch");

    let deleted_again = watches::delete(&conn, id).expect("delete watch");

    assert!(!deleted_again);
}

#[test]
fn delete_of_an_unknown_id_returns_false() {
    let (conn, _scratch) = open_scratch("delete_of_an_unknown_id_returns_false");

    let deleted = watches::delete(&conn, 9999).expect("delete watch");

    assert!(!deleted);
}

#[test]
fn delete_removes_the_watch_so_get_returns_none() {
    let (conn, _scratch) = open_scratch("delete_removes_the_watch_so_get_returns_none");
    let id = insert_at(&conn, "https://shop.example/a", NOW);

    watches::delete(&conn, id).expect("delete watch");

    assert_eq!(watches::get(&conn, id).expect("get watch"), None);
}

#[test]
fn get_of_a_row_with_status_bogus_returns_corrupt() {
    let (conn, _scratch) = open_scratch("get_of_a_row_with_status_bogus_returns_corrupt");
    let id = insert_raw(&conn, "https://shop.example/bogus", None, None, "bogus");

    match watches::get(&conn, id) {
        Err(DbError::Corrupt(_)) => {}
        other => panic!("expected DbError::Corrupt, got {other:?}"),
    }
}

#[test]
fn get_of_a_row_with_an_unknown_currency_returns_corrupt() {
    let (conn, _scratch) = open_scratch("get_of_a_row_with_an_unknown_currency_returns_corrupt");
    // 'XXX' is not a supported ISO 4217 code, so the stored target cannot be read back.
    let id = insert_raw(
        &conn,
        "https://shop.example/xxx",
        Some(1999),
        Some("XXX"),
        "ok",
    );

    match watches::get(&conn, id) {
        Err(DbError::Corrupt(_)) => {}
        other => panic!("expected DbError::Corrupt, got {other:?}"),
    }
}

/// A watch with a name, selector, a 1299 EUR target and a 15-minute interval, so each setter
/// has a stored value to change and fields to leave alone.
fn detailed_watch(url: &str) -> NewWatch {
    NewWatch {
        url: url.to_string(),
        name: Some("Kettle".to_string()),
        selector: Some(".price-now".to_string()),
        target: Some(Money {
            minor: 1299,
            currency: Currency::Eur,
        }),
        interval_s: 900,
    }
}

/// Inserts a `detailed_watch` for `https://shop.example/a` at `NOW` and returns its id.
fn insert_detailed(conn: &Connection) -> i64 {
    watches::insert(conn, &detailed_watch("https://shop.example/a"), NOW).expect("insert watch")
}

/// Reads the `currency` column of the watch with `id` as stored, without the target decoding.
fn stored_currency(conn: &Connection, id: i64) -> Option<String> {
    conn.query_row(
        "SELECT currency FROM watches WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .expect("read currency column")
}

#[test]
fn set_target_stores_the_amount_in_minor_units() {
    let (conn, _scratch) = open_scratch("set_target_stores_the_amount_in_minor_units");
    let id = insert_at(&conn, "https://shop.example/a", NOW);
    // 199.99 EUR is 19999 minor units, since EUR has two decimal places.
    let target = Money {
        minor: 19999,
        currency: Currency::Eur,
    };

    watches::set_target(&conn, id, Some(&target)).expect("set target");

    assert_eq!(get_watch(&conn, id).target, Some(target));
}

#[test]
fn set_target_with_another_currency_replaces_the_stored_currency() {
    let (conn, _scratch) = open_scratch("set_target_with_another_currency_replaces_the_currency");
    let id = insert_detailed(&conn);
    let target = Money {
        minor: 15000,
        currency: Currency::Jpy,
    };

    watches::set_target(&conn, id, Some(&target)).expect("set target");

    assert_eq!(get_watch(&conn, id).target, Some(target));
    assert_eq!(stored_currency(&conn, id).as_deref(), Some("JPY"));
}

#[test]
fn set_target_none_clears_the_target() {
    let (conn, _scratch) = open_scratch("set_target_none_clears_the_target");
    let id = insert_detailed(&conn);

    watches::set_target(&conn, id, None).expect("set target");

    assert_eq!(get_watch(&conn, id).target, None);
}

#[test]
fn set_target_none_clears_the_currency_column() {
    let (conn, _scratch) = open_scratch("set_target_none_clears_the_currency_column");
    let new = NewWatch {
        target: Some(Money {
            minor: 15000,
            currency: Currency::Jpy,
        }),
        ..new_watch("https://shop.example/yen")
    };
    let id = watches::insert(&conn, &new, NOW).expect("insert watch");

    watches::set_target(&conn, id, None).expect("set target");

    // Clearing the target also clears its currency: a currency with no amount reads back as corrupt.
    assert_eq!(stored_currency(&conn, id), None);
}

#[test]
fn set_target_changes_no_other_field_of_the_watch() {
    let (conn, _scratch) = open_scratch("set_target_changes_no_other_field");
    let id = insert_detailed(&conn);
    let before = get_watch(&conn, id);
    let target = Money {
        minor: 19999,
        currency: Currency::Eur,
    };

    watches::set_target(&conn, id, Some(&target)).expect("set target");

    assert_eq!(
        get_watch(&conn, id),
        Watch {
            target: Some(target),
            ..before
        }
    );
}

#[test]
fn set_target_of_an_existing_watch_returns_true() {
    let (conn, _scratch) = open_scratch("set_target_of_an_existing_watch_returns_true");
    let id = insert_at(&conn, "https://shop.example/a", NOW);
    let target = Money {
        minor: 19999,
        currency: Currency::Eur,
    };

    let updated = watches::set_target(&conn, id, Some(&target)).expect("set target");

    assert!(updated);
}

#[test]
fn set_target_of_an_unknown_id_returns_false() {
    let (conn, _scratch) = open_scratch("set_target_of_an_unknown_id_returns_false");
    let target = Money {
        minor: 19999,
        currency: Currency::Eur,
    };

    let updated = watches::set_target(&conn, 9999, Some(&target)).expect("set target");

    assert!(!updated);
}

#[test]
fn set_target_of_an_unknown_id_leaves_existing_watches_unchanged() {
    let (conn, _scratch) = open_scratch("set_target_of_an_unknown_id_leaves_watches_unchanged");
    let id = insert_detailed(&conn);
    let before = get_watch(&conn, id);
    let target = Money {
        minor: 19999,
        currency: Currency::Eur,
    };

    watches::set_target(&conn, 9999, Some(&target)).expect("set target");

    assert_eq!(get_watch(&conn, id), before);
}

#[test]
fn set_interval_stores_the_new_interval() {
    let (conn, _scratch) = open_scratch("set_interval_stores_the_new_interval");
    let id = insert_detailed(&conn);

    watches::set_interval(&conn, id, HOUR_S).expect("set interval");

    assert_eq!(get_watch(&conn, id).interval_s, 3600);
}

#[test]
fn set_interval_changes_no_other_field_of_the_watch() {
    let (conn, _scratch) = open_scratch("set_interval_changes_no_other_field");
    let id = insert_detailed(&conn);
    let before = get_watch(&conn, id);

    watches::set_interval(&conn, id, HOUR_S).expect("set interval");

    assert_eq!(
        get_watch(&conn, id),
        Watch {
            interval_s: HOUR_S,
            ..before
        }
    );
}

#[test]
fn set_interval_of_an_existing_watch_returns_true() {
    let (conn, _scratch) = open_scratch("set_interval_of_an_existing_watch_returns_true");
    let id = insert_detailed(&conn);

    let updated = watches::set_interval(&conn, id, HOUR_S).expect("set interval");

    assert!(updated);
}

#[test]
fn set_interval_of_an_unknown_id_returns_false() {
    let (conn, _scratch) = open_scratch("set_interval_of_an_unknown_id_returns_false");

    let updated = watches::set_interval(&conn, 9999, HOUR_S).expect("set interval");

    assert!(!updated);
}

#[test]
fn set_enabled_false_then_true_toggles_enabled_both_ways() {
    let (conn, _scratch) = open_scratch("set_enabled_false_then_true_toggles_enabled");
    let id = insert_detailed(&conn);

    watches::set_enabled(&conn, id, false).expect("disable watch");
    assert!(!get_watch(&conn, id).enabled);

    watches::set_enabled(&conn, id, true).expect("enable watch");
    assert!(get_watch(&conn, id).enabled);
}

#[test]
fn set_enabled_changes_no_other_field_of_the_watch() {
    let (conn, _scratch) = open_scratch("set_enabled_changes_no_other_field");
    let id = insert_detailed(&conn);
    let before = get_watch(&conn, id);

    watches::set_enabled(&conn, id, false).expect("disable watch");

    assert_eq!(
        get_watch(&conn, id),
        Watch {
            enabled: false,
            ..before
        }
    );
}

#[test]
fn set_enabled_of_an_existing_watch_returns_true() {
    let (conn, _scratch) = open_scratch("set_enabled_of_an_existing_watch_returns_true");
    let id = insert_detailed(&conn);

    let updated = watches::set_enabled(&conn, id, false).expect("disable watch");

    assert!(updated);
}

#[test]
fn set_enabled_of_an_unknown_id_returns_false() {
    let (conn, _scratch) = open_scratch("set_enabled_of_an_unknown_id_returns_false");

    let updated = watches::set_enabled(&conn, 9999, false).expect("disable watch");

    assert!(!updated);
}
