use haggle::db::observations::{history, insert, latest};
use haggle::db::{DbError, NewObservation, Observation, open};
use haggle::extract::{Availability, Source};
use haggle::money::{Currency, Money};
use rusqlite::{Connection, params};
use std::path::PathBuf;

/// A database file under the temp dir. The file and its WAL side files are removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Names the file after this process and `test`, and clears any leftover from an earlier run.
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "haggle-observations-{}-{test}.sqlite",
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

/// A migrated database. `conn` is declared before `_scratch`, so it closes before the file is removed.
struct Db {
    conn: Connection,
    _scratch: Scratch,
}

/// Opens a fresh migrated database for the test named `test`.
fn open_db(test: &str) -> Db {
    let scratch = Scratch::new(test);
    let conn = match open(&scratch.path) {
        Ok(conn) => conn,
        Err(error) => panic!("expected open to succeed, got {error}"),
    };
    Db {
        conn,
        _scratch: scratch,
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

/// Inserts an observation with the given stored codes, bypassing `insert`.
fn insert_raw(conn: &Connection, watch_id: i64, availability: &str, source: &str) {
    conn.execute(
        "INSERT INTO observations (watch_id, at, price_minor, currency, availability, source) \
         VALUES (?1, 0, 1999, 'EUR', ?2, ?3)",
        params![watch_id, availability, source],
    )
    .expect("insert raw observation");
}

/// Returns the text stored in `column` of the observation row `id`.
fn stored_text(conn: &Connection, column: &str, id: i64) -> String {
    conn.query_row(
        &format!("SELECT {column} FROM observations WHERE id = ?1"),
        [id],
        |row| row.get(0),
    )
    .expect("read stored text")
}

/// An amount of `minor` units in euro, which has exponent 2: 1999 is EUR 19.99.
fn eur(minor: i64) -> Money {
    Money {
        minor,
        currency: Currency::Eur,
    }
}

/// An in-stock EUR observation for `watch_id` taken at `at`, found by JSON-LD, with no list price.
fn sample(watch_id: i64, at: i64) -> NewObservation {
    NewObservation {
        watch_id,
        at,
        price: eur(1999),
        list_price: None,
        availability: Availability::InStock,
        source: Source::JsonLd,
    }
}

/// The row `insert` should return for `new` once stored under `id`.
fn stored(new: &NewObservation, id: i64) -> Observation {
    Observation {
        id,
        watch_id: new.watch_id,
        at: new.at,
        price: new.price,
        list_price: new.list_price,
        availability: new.availability,
        source: new.source,
    }
}

/// Inserts `new` and returns the row `history` reads back for it.
fn read_back(conn: &Connection, new: &NewObservation) -> Observation {
    let inserted = insert(conn, new).expect("insert observation");
    let rows = history(conn, new.watch_id).expect("history");
    rows.into_iter()
        .find(|row| row.id == inserted.id)
        .expect("inserted row is in history")
}

/// The `at` values of `rows`, in order.
fn ats(rows: &[Observation]) -> Vec<i64> {
    rows.iter().map(|row| row.at).collect()
}

/// The ids of `rows`, in order.
fn ids(rows: &[Observation]) -> Vec<i64> {
    rows.iter().map(|row| row.id).collect()
}

#[test]
fn insert_returns_the_stored_row_with_a_positive_id() {
    let db = open_db("insert_returns_stored_row");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    let new = sample(watch_id, 1_700_000_000);

    let row = insert(&db.conn, &new).expect("insert observation");

    assert!(row.id > 0, "expected a positive row id, got {}", row.id);
    assert_eq!(row, stored(&new, row.id));
    let rows = history(&db.conn, watch_id).expect("history");
    assert_eq!(rows, [row]);
}

#[test]
fn history_is_empty_for_a_watch_with_no_rows() {
    let db = open_db("history_empty");
    let watch_id = insert_watch(&db.conn, "https://shop.example/empty");

    let rows = history(&db.conn, watch_id).expect("history");

    assert!(rows.is_empty(), "expected no rows, got {}", rows.len());
}

#[test]
fn history_lists_rows_oldest_first() {
    let db = open_db("history_oldest_first");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    // Inserted out of time order on purpose.
    for at in [300, 100, 200] {
        insert(&db.conn, &sample(watch_id, at)).expect("insert observation");
    }

    let rows = history(&db.conn, watch_id).expect("history");

    assert_eq!(ats(&rows), [100, 200, 300]);
}

#[test]
fn history_keeps_insertion_order_for_rows_with_the_same_at() {
    let db = open_db("history_same_at");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    let inserted: Vec<i64> = (0..3)
        .map(|_| {
            insert(&db.conn, &sample(watch_id, 500))
                .expect("insert observation")
                .id
        })
        .collect();

    let rows = history(&db.conn, watch_id).expect("history");

    assert_eq!(ids(&rows), inserted);
}

#[test]
fn history_only_returns_rows_of_the_given_watch() {
    let db = open_db("history_one_watch");
    let kettle = insert_watch(&db.conn, "https://shop.example/kettle");
    let toaster = insert_watch(&db.conn, "https://shop.example/toaster");
    insert(&db.conn, &sample(kettle, 100)).expect("insert observation");
    insert(&db.conn, &sample(toaster, 50)).expect("insert observation");
    insert(&db.conn, &sample(kettle, 200)).expect("insert observation");

    let rows = history(&db.conn, kettle).expect("history");

    assert_eq!(ats(&rows), [100, 200]);
    assert!(rows.iter().all(|row| row.watch_id == kettle));
}

#[test]
fn latest_is_the_last_row_of_history() {
    let db = open_db("latest_last_of_history");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    // Inserted newest first; history ends with at 300, so that is the latest.
    let newest = insert(&db.conn, &sample(watch_id, 300)).expect("insert observation");
    insert(&db.conn, &sample(watch_id, 100)).expect("insert observation");
    insert(&db.conn, &sample(watch_id, 200)).expect("insert observation");

    let row = latest(&db.conn, watch_id)
        .expect("latest")
        .expect("a row for the watch");

    assert_eq!(row, newest);
}

#[test]
fn latest_is_the_later_insert_when_rows_share_an_at() {
    let db = open_db("latest_same_at");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    insert(&db.conn, &sample(watch_id, 400)).expect("insert observation");
    let second = insert(&db.conn, &sample(watch_id, 400)).expect("insert observation");

    let row = latest(&db.conn, watch_id)
        .expect("latest")
        .expect("a row for the watch");

    assert_eq!(row, second);
}

#[test]
fn latest_is_none_for_a_watch_with_no_rows() {
    let db = open_db("latest_empty");
    let watch_id = insert_watch(&db.conn, "https://shop.example/empty");

    let row = latest(&db.conn, watch_id).expect("latest");

    assert_eq!(row, None);
}

#[test]
fn every_availability_round_trips() {
    let db = open_db("availability_round_trip");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    let availabilities = [
        Availability::InStock,
        Availability::OutOfStock,
        Availability::Preorder,
        Availability::Unknown,
    ];

    for (at, availability) in (1_i64..).zip(availabilities) {
        let new = NewObservation {
            availability,
            ..sample(watch_id, at)
        };
        let row = read_back(&db.conn, &new);
        assert_eq!(row, stored(&new, row.id), "round trip of {availability}");
    }
}

#[test]
fn availability_is_stored_under_its_spec_code() {
    let db = open_db("availability_codes");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    let cases = [
        (Availability::InStock, "in_stock"),
        (Availability::OutOfStock, "out_of_stock"),
        (Availability::Preorder, "preorder"),
        (Availability::Unknown, "unknown"),
    ];

    for (at, (availability, code)) in (1_i64..).zip(cases) {
        let new = NewObservation {
            availability,
            ..sample(watch_id, at)
        };
        let row = insert(&db.conn, &new).expect("insert observation");
        assert_eq!(stored_text(&db.conn, "availability", row.id), code);
    }
}

#[test]
fn every_source_round_trips() {
    let db = open_db("source_round_trip");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    let sources = [
        Source::JsonLd,
        Source::Meta,
        Source::Microdata,
        Source::Selector,
    ];

    for (at, source) in (1_i64..).zip(sources) {
        let new = NewObservation {
            source,
            ..sample(watch_id, at)
        };
        let row = read_back(&db.conn, &new);
        assert_eq!(row, stored(&new, row.id), "round trip of {source}");
    }
}

#[test]
fn source_is_stored_under_its_spec_code() {
    let db = open_db("source_codes");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    let cases = [
        (Source::JsonLd, "json_ld"),
        (Source::Meta, "meta"),
        (Source::Microdata, "microdata"),
        (Source::Selector, "selector"),
    ];

    for (at, (source, code)) in (1_i64..).zip(cases) {
        let new = NewObservation {
            source,
            ..sample(watch_id, at)
        };
        let row = insert(&db.conn, &new).expect("insert observation");
        assert_eq!(stored_text(&db.conn, "source", row.id), code);
    }
}

#[test]
fn list_price_none_round_trips() {
    let db = open_db("list_price_none");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    let new = sample(watch_id, 100);

    let row = read_back(&db.conn, &new);

    assert_eq!(row.list_price, None);
    assert_eq!(row, stored(&new, row.id));
}

#[test]
fn list_price_in_another_currency_is_stored_as_none() {
    let db = open_db("list_price_other_currency");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    // The price is EUR, so a USD list price cannot share its currency column and is stored as NULL.
    let new = NewObservation {
        list_price: Some(Money {
            minor: 2499,
            currency: Currency::Usd,
        }),
        ..sample(watch_id, 100)
    };

    let row = read_back(&db.conn, &new);

    assert_eq!(row.list_price, None);
    assert_eq!(row.price, eur(1999));
}

#[test]
fn eur_amounts_round_trip() {
    let db = open_db("eur_round_trip");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    // 1999 is EUR 19.99 and 2499 is EUR 24.99, with exponent 2.
    let new = NewObservation {
        list_price: Some(eur(2499)),
        ..sample(watch_id, 100)
    };

    let row = read_back(&db.conn, &new);

    assert_eq!(row, stored(&new, row.id));
}

#[test]
fn jpy_amounts_round_trip() {
    let db = open_db("jpy_round_trip");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    // JPY has exponent 0, so 1980 is 1980 yen itself and 2500 is 2500 yen.
    let new = NewObservation {
        price: Money {
            minor: 1980,
            currency: Currency::Jpy,
        },
        list_price: Some(Money {
            minor: 2500,
            currency: Currency::Jpy,
        }),
        ..sample(watch_id, 100)
    };

    let row = read_back(&db.conn, &new);

    assert_eq!(row, stored(&new, row.id));
}

#[test]
fn history_fails_with_corrupt_for_an_unknown_availability_code() {
    let db = open_db("corrupt_availability");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    insert_raw(&db.conn, watch_id, "bogus", "json_ld");

    let error = history(&db.conn, watch_id).expect_err("expected an error for the bogus code");

    assert!(
        matches!(error, DbError::Corrupt(_)),
        "expected DbError::Corrupt, got {error}"
    );
}

#[test]
fn history_fails_with_corrupt_for_an_unknown_source_code() {
    let db = open_db("corrupt_source");
    let watch_id = insert_watch(&db.conn, "https://shop.example/kettle");
    insert_raw(&db.conn, watch_id, "in_stock", "bogus");

    let error = history(&db.conn, watch_id).expect_err("expected an error for the bogus code");

    assert!(
        matches!(error, DbError::Corrupt(_)),
        "expected DbError::Corrupt, got {error}"
    );
}
