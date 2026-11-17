//! Queries on the `events` table.

use rusqlite::{Connection, Row, params};

use super::{DbError, Event, EventKind, StoredEvent};
use crate::money::{Currency, Money};

/// Column list shared by every select, in the order `stored_event` reads them.
const COLUMNS: &str = "id, watch_id, at, kind, old_minor, new_minor, currency, notified_at";

/// Inserts an event that has not been notified; returns its id.
///
/// The stored currency is taken from `event.old` if set, otherwise from `event.new`, and is
/// NULL when neither price is set. `at` is the Unix time the event was recorded.
///
/// Returns `DbError::Sqlite` if the insert fails.
pub fn insert(conn: &Connection, event: &Event, at: i64) -> Result<i64, DbError> {
    let currency = event.old.or(event.new).map(|money| money.currency.code());
    conn.execute(
        "INSERT INTO events (watch_id, at, kind, old_minor, new_minor, currency, notified_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        params![
            event.watch_id,
            at,
            event.kind.as_str(),
            event.old.map(|money| money.minor),
            event.new.map(|money| money.minor),
            currency,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Returns every event with no notification time, ordered by id.
///
/// Returns `DbError::Corrupt` if a stored row has an unknown kind or currency, or an amount
/// without a currency.
pub fn unnotified(conn: &Connection) -> Result<Vec<StoredEvent>, DbError> {
    query_all(conn, "WHERE notified_at IS NULL ORDER BY id", params![])
}

/// Sets the notification time of each event in `ids` to `at`.
///
/// Ids that match no event are ignored. Each update runs on its own, so a failure part way
/// through leaves the earlier ids marked.
///
/// Returns `DbError::Sqlite` if an update fails.
pub fn mark_notified(conn: &Connection, ids: &[i64], at: i64) -> Result<(), DbError> {
    let mut statement = conn.prepare("UPDATE events SET notified_at = ?1 WHERE id = ?2")?;
    for &id in ids {
        statement.execute(params![at, id])?;
    }
    Ok(())
}

/// Returns every event for `watch_id`, ordered by id.
///
/// Returns `DbError::Corrupt` if a stored row has an unknown kind or currency, or an amount
/// without a currency.
pub fn for_watch(conn: &Connection, watch_id: i64) -> Result<Vec<StoredEvent>, DbError> {
    query_all(conn, "WHERE watch_id = ?1 ORDER BY id", params![watch_id])
}

/// Runs a select on `events` with the given trailing clauses and returns every event.
///
/// Returns `DbError::Sqlite` if the query fails, and `DbError::Corrupt` for a bad stored row.
fn query_all(
    conn: &Connection,
    clauses: &str,
    parameters: impl rusqlite::Params,
) -> Result<Vec<StoredEvent>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM events {clauses}");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(parameters, |row| Ok(stored_event(row)))?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row??);
    }
    Ok(events)
}

/// Reads a `COLUMNS` row into a `StoredEvent`.
///
/// Returns `DbError::Sqlite` if a column cannot be read, and `DbError::Corrupt` for an
/// unknown kind or currency, or an amount without a currency.
fn stored_event(row: &Row) -> Result<StoredEvent, DbError> {
    let id: i64 = row.get(0)?;
    let kind_code: String = row.get(3)?;
    let kind = EventKind::from_code(&kind_code)
        .ok_or_else(|| DbError::Corrupt(format!("event {id} has unknown kind {kind_code:?}")))?;
    let currency_code: Option<String> = row.get(6)?;
    let currency = parse_currency(id, currency_code.as_deref())?;
    let old = parse_amount(id, row.get(4)?, currency)?;
    let new = parse_amount(id, row.get(5)?, currency)?;
    Ok(StoredEvent {
        id,
        at: row.get(2)?,
        notified_at: row.get(7)?,
        event: Event {
            watch_id: row.get(1)?,
            kind,
            old,
            new,
        },
    })
}

/// Parses the stored currency code, or returns `None` if the column is NULL.
///
/// Returns `DbError::Corrupt` if the code is not a known currency.
fn parse_currency(id: i64, code: Option<&str>) -> Result<Option<Currency>, DbError> {
    code.map(|code| {
        Currency::from_code(code)
            .ok_or_else(|| DbError::Corrupt(format!("event {id} has unknown currency {code:?}")))
    })
    .transpose()
}

/// Combines a stored amount with the event's currency, or returns `None` if there is no amount.
///
/// Returns `DbError::Corrupt` if the amount is set but the currency is not.
fn parse_amount(
    id: i64,
    minor: Option<i64>,
    currency: Option<Currency>,
) -> Result<Option<Money>, DbError> {
    match (minor, currency) {
        (None, _) => Ok(None),
        (Some(minor), Some(currency)) => Ok(Some(Money { minor, currency })),
        (Some(_), None) => Err(DbError::Corrupt(format!(
            "event {id} has an amount without a currency"
        ))),
    }
}
