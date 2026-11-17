use haggle::db::{DbError, Event, EventKind, StoredEvent, events, open};
use haggle::money::{Currency, Money};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

/// Every event kind, so each stored code is exercised.
const ALL_KINDS: [EventKind; 7] = [
    EventKind::PriceDrop,
    EventKind::PriceRise,
    EventKind::TargetHit,
    EventKind::LowestEver,
    EventKind::BackInStock,
    EventKind::OutOfStock,
    EventKind::InflatedSale,
];

/// Unix seconds used as the time an event is recorded.
const RECORDED_AT: i64 = 1_700_000_000;

/// Unix seconds used as the time notifications are sent.
const NOTIFIED_AT: i64 = 1_700_000_500;

/// A database file under the temp dir. The file and its WAL side files are removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Names the file after this process and `test`, and clears any leftover from an earlier run.
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "haggle-db-events-{}-{test}.sqlite",
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

/// Opens the database at `path`, failing the test if the open is rejected.
fn open_ok(path: &Path) -> Connection {
    match open(path) {
        Ok(conn) => conn,
        Err(error) => panic!("expected open to succeed, got {error}"),
    }
}

/// Inserts a watch with only the required columns and returns its id.
fn insert_watch(conn: &Connection, url: &str) -> i64 {
    conn.execute(
        "INSERT INTO watches (url, interval_s, next_check_at, created_at) VALUES (?1, 3600, 0, 0)",
        [url],
    )
    .expect("insert watch");
    conn.last_insert_rowid()
}

/// Builds an amount in `currency`.
fn money(minor: i64, currency: Currency) -> Money {
    Money { minor, currency }
}

/// Builds an event of `kind` for `watch_id`.
fn event_for(watch_id: i64, kind: EventKind, old: Option<Money>, new: Option<Money>) -> Event {
    Event {
        watch_id,
        kind,
        old,
        new,
    }
}

/// A price drop from 20.00 to 15.00 USD for `watch_id`.
fn price_drop(watch_id: i64) -> Event {
    event_for(
        watch_id,
        EventKind::PriceDrop,
        Some(money(2000, Currency::Usd)),
        Some(money(1500, Currency::Usd)),
    )
}

/// Stores a price drop for `watch_id` recorded at `at` and returns its id.
fn insert_price_drop(conn: &Connection, watch_id: i64, at: i64) -> i64 {
    events::insert(conn, &price_drop(watch_id), at).expect("insert event")
}

/// Stores `event`, reads back the events of its watch, and returns the single one stored.
fn only_stored_event(conn: &Connection, event: &Event) -> StoredEvent {
    events::insert(conn, event, RECORDED_AT).expect("insert event");
    let mut stored = events::for_watch(conn, event.watch_id).expect("for_watch");
    assert_eq!(stored.len(), 1, "expected exactly one stored event");
    stored.remove(0)
}

/// Returns the ids of `stored`, in the order given.
fn ids(stored: &[StoredEvent]) -> Vec<i64> {
    stored.iter().map(|event| event.id).collect()
}

/// Inserts a row into `events` with SQL, bypassing `events::insert`.
fn insert_raw_event(
    conn: &Connection,
    watch_id: i64,
    kind: &str,
    old_minor: Option<i64>,
    new_minor: Option<i64>,
    currency: Option<&str>,
) {
    conn.execute(
        "INSERT INTO events (watch_id, at, kind, old_minor, new_minor, currency) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![watch_id, RECORDED_AT, kind, old_minor, new_minor, currency],
    )
    .expect("insert raw event");
}

#[test]
fn insert_then_unnotified_returns_the_event_as_pending() {
    let scratch = Scratch::new("insert_then_unnotified");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let expected = price_drop(watch_id);

    let id = events::insert(&conn, &expected, RECORDED_AT).expect("insert event");
    let pending = events::unnotified(&conn).expect("unnotified");

    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, id);
    assert_eq!(pending[0].at, RECORDED_AT);
    assert_eq!(pending[0].notified_at, None);
    assert_eq!(pending[0].event, expected);
}

#[test]
fn every_event_kind_round_trips_through_for_watch() {
    let scratch = Scratch::new("every_kind_round_trips");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let expected: Vec<Event> = ALL_KINDS
        .iter()
        .map(|&kind| {
            event_for(
                watch_id,
                kind,
                Some(money(2000, Currency::Usd)),
                Some(money(1500, Currency::Usd)),
            )
        })
        .collect();
    for event in &expected {
        events::insert(&conn, event, RECORDED_AT).expect("insert event");
    }

    let read: Vec<Event> = events::for_watch(&conn, watch_id)
        .expect("for_watch")
        .into_iter()
        .map(|stored| stored.event)
        .collect();

    assert_eq!(read, expected);
}

#[test]
fn event_with_old_price_none_and_new_price_some_round_trips() {
    let scratch = Scratch::new("old_none_new_some");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let expected = event_for(
        watch_id,
        EventKind::TargetHit,
        None,
        Some(money(1500, Currency::Eur)),
    );

    let stored = only_stored_event(&conn, &expected);

    assert_eq!(stored.event, expected);
}

#[test]
fn event_with_old_price_some_and_new_price_none_round_trips() {
    let scratch = Scratch::new("old_some_new_none");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let expected = event_for(
        watch_id,
        EventKind::LowestEver,
        Some(money(900, Currency::Gbp)),
        None,
    );

    let stored = only_stored_event(&conn, &expected);

    assert_eq!(stored.event, expected);
}

#[test]
fn event_with_no_prices_round_trips_with_both_prices_none() {
    let scratch = Scratch::new("both_none_round_trips");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let expected = event_for(watch_id, EventKind::OutOfStock, None, None);

    let stored = only_stored_event(&conn, &expected);

    assert_eq!(stored.event, expected);
}

#[test]
fn event_with_no_prices_stores_a_null_currency() {
    let scratch = Scratch::new("both_none_null_currency");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let id = events::insert(
        &conn,
        &event_for(watch_id, EventKind::OutOfStock, None, None),
        RECORDED_AT,
    )
    .expect("insert event");

    let currency: Option<String> = conn
        .query_row("SELECT currency FROM events WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .expect("currency query");

    assert_eq!(currency, None);
}

#[test]
fn mark_notified_removes_the_given_events_from_unnotified() {
    let scratch = Scratch::new("mark_removes_from_unnotified");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let first = insert_price_drop(&conn, watch_id, RECORDED_AT);
    let second = insert_price_drop(&conn, watch_id, RECORDED_AT);
    let third = insert_price_drop(&conn, watch_id, RECORDED_AT);

    events::mark_notified(&conn, &[first, third], NOTIFIED_AT).expect("mark_notified");
    let pending = events::unnotified(&conn).expect("unnotified");

    assert_eq!(ids(&pending), [second]);
}

#[test]
fn mark_notified_sets_notified_at_on_the_given_events_only() {
    let scratch = Scratch::new("mark_sets_notified_at");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let first = insert_price_drop(&conn, watch_id, RECORDED_AT);
    insert_price_drop(&conn, watch_id, RECORDED_AT);
    let third = insert_price_drop(&conn, watch_id, RECORDED_AT);

    events::mark_notified(&conn, &[first, third], NOTIFIED_AT).expect("mark_notified");
    let stored = events::for_watch(&conn, watch_id).expect("for_watch");

    let notified: Vec<Option<i64>> = stored.iter().map(|event| event.notified_at).collect();
    assert_eq!(notified, [Some(NOTIFIED_AT), None, Some(NOTIFIED_AT)]);
}

#[test]
fn mark_notified_with_an_empty_slice_changes_nothing() {
    let scratch = Scratch::new("mark_empty_slice");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let id = insert_price_drop(&conn, watch_id, RECORDED_AT);

    events::mark_notified(&conn, &[], NOTIFIED_AT).expect("mark_notified with no ids");
    let pending = events::unnotified(&conn).expect("unnotified");

    assert_eq!(ids(&pending), [id]);
    assert_eq!(pending[0].notified_at, None);
}

#[test]
fn unnotified_is_ordered_by_id_not_by_recorded_time() {
    let scratch = Scratch::new("unnotified_order");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    // The recorded times run backwards from the insert order, so an order by `at` would differ.
    let first = insert_price_drop(&conn, watch_id, 300);
    let second = insert_price_drop(&conn, watch_id, 100);
    let third = insert_price_drop(&conn, watch_id, 200);

    let pending = events::unnotified(&conn).expect("unnotified");

    assert_eq!(ids(&pending), [first, second, third]);
}

#[test]
fn for_watch_is_ordered_by_id_and_keeps_notified_events() {
    let scratch = Scratch::new("for_watch_order");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let first = insert_price_drop(&conn, watch_id, 300);
    let second = insert_price_drop(&conn, watch_id, 100);
    let third = insert_price_drop(&conn, watch_id, 200);
    events::mark_notified(&conn, &[second], NOTIFIED_AT).expect("mark_notified");

    let stored = events::for_watch(&conn, watch_id).expect("for_watch");

    assert_eq!(ids(&stored), [first, second, third]);
}

#[test]
fn for_watch_returns_only_the_events_of_that_watch() {
    let scratch = Scratch::new("for_watch_filters_by_watch");
    let conn = open_ok(&scratch.path);
    let kettle = insert_watch(&conn, "https://shop.example/kettle");
    let toaster = insert_watch(&conn, "https://shop.example/toaster");
    let kettle_event = insert_price_drop(&conn, kettle, RECORDED_AT);
    insert_price_drop(&conn, toaster, RECORDED_AT);

    let stored = events::for_watch(&conn, kettle).expect("for_watch");

    assert_eq!(ids(&stored), [kettle_event]);
}

#[test]
fn notified_events_stay_notified_after_reopening_the_database() {
    let scratch = Scratch::new("notified_survives_reopen");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    let notified = insert_price_drop(&conn, watch_id, RECORDED_AT);
    let pending_id = insert_price_drop(&conn, watch_id, RECORDED_AT);
    events::mark_notified(&conn, &[notified], NOTIFIED_AT).expect("mark_notified");
    drop(conn);

    let reopened = open_ok(&scratch.path);
    let pending = events::unnotified(&reopened).expect("unnotified");
    let stored = events::for_watch(&reopened, watch_id).expect("for_watch");

    assert_eq!(ids(&pending), [pending_id]);
    let notified_at: Vec<Option<i64>> = stored.iter().map(|event| event.notified_at).collect();
    assert_eq!(notified_at, [Some(NOTIFIED_AT), None]);
}

#[test]
fn unnotified_with_unknown_kind_code_is_corrupt() {
    let scratch = Scratch::new("unknown_kind_unnotified");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    insert_raw_event(&conn, watch_id, "bogus", None, None, None);

    let error = events::unnotified(&conn).expect_err("unknown kind should fail");

    assert!(matches!(error, DbError::Corrupt(_)), "got {error}");
}

#[test]
fn for_watch_with_unknown_kind_code_is_corrupt() {
    let scratch = Scratch::new("unknown_kind_for_watch");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    insert_raw_event(&conn, watch_id, "bogus", None, None, None);

    let error = events::for_watch(&conn, watch_id).expect_err("unknown kind should fail");

    assert!(matches!(error, DbError::Corrupt(_)), "got {error}");
}

#[test]
fn for_watch_with_unknown_currency_code_is_corrupt() {
    let scratch = Scratch::new("unknown_currency_for_watch");
    let conn = open_ok(&scratch.path);
    let watch_id = insert_watch(&conn, "https://shop.example/kettle");
    insert_raw_event(
        &conn,
        watch_id,
        "price_drop",
        Some(2000),
        Some(1500),
        Some("XXX"),
    );

    let error = events::for_watch(&conn, watch_id).expect_err("unknown currency should fail");

    assert!(matches!(error, DbError::Corrupt(_)), "got {error}");
}
