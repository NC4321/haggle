//! Queries on the `watches` table.

use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, params};

use super::{DbError, NewWatch, Watch, WatchStatus};
use crate::money::{Currency, Money};

/// Column list shared by every select, in the order `from_row` reads them.
const COLUMNS: &str = "id, url, name, selector, target_minor, currency, interval_s, enabled, \
                       status, etag, last_modified, next_check_at, failures, created_at";

/// Inserts a watch that is enabled, healthy and due for a check at `now`; returns its id.
///
/// Returns `DbError::DuplicateUrl` if the URL is already watched, and `DbError::Sqlite`
/// for any other failure.
pub fn insert(conn: &Connection, new: &NewWatch, now: i64) -> Result<i64, DbError> {
    let result = conn.execute(
        "INSERT INTO watches (url, name, selector, target_minor, currency, interval_s, \
         enabled, status, failures, next_check_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 0, ?8, ?8)",
        params![
            new.url,
            new.name,
            new.selector,
            new.target.map(|target| target.minor),
            new.target.map(|target| target.currency.code()),
            new.interval_s,
            WatchStatus::Ok.as_str(),
            now,
        ],
    );
    match result {
        Ok(_) => Ok(conn.last_insert_rowid()),
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == ErrorCode::ConstraintViolation
                && error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            Err(DbError::DuplicateUrl(new.url.clone()))
        }
        Err(error) => Err(error.into()),
    }
}

/// Returns the watch with row id `id`, or `None` if there is none.
///
/// Returns `DbError::Corrupt` if the stored row has an unknown status or currency.
pub fn get(conn: &Connection, id: i64) -> Result<Option<Watch>, DbError> {
    query_optional(conn, "WHERE id = ?1", params![id])
}

/// Returns the watch for `url`, or `None` if it is not watched.
///
/// Returns `DbError::Corrupt` if the stored row has an unknown status or currency.
pub fn find_by_url(conn: &Connection, url: &str) -> Result<Option<Watch>, DbError> {
    query_optional(conn, "WHERE url = ?1", params![url])
}

/// Returns every watch, ordered by id.
///
/// Returns `DbError::Corrupt` if a stored row has an unknown status or currency.
pub fn list(conn: &Connection) -> Result<Vec<Watch>, DbError> {
    query_all(conn, "ORDER BY id", params![])
}

/// Returns up to `limit` enabled watches whose next check is at or before `now`,
/// ordered by next check time then id.
///
/// Returns `DbError::Corrupt` if a stored row has an unknown status or currency.
pub fn due(conn: &Connection, now: i64, limit: usize) -> Result<Vec<Watch>, DbError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    query_all(
        conn,
        "WHERE enabled = 1 AND next_check_at <= ?1 ORDER BY next_check_at, id LIMIT ?2",
        params![now, limit],
    )
}

/// Records a successful check: status ok, failures reset, validators and next check stored.
///
/// Returns `DbError::Sqlite` if the update fails.
pub fn record_success(
    conn: &Connection,
    id: i64,
    etag: Option<&str>,
    last_modified: Option<&str>,
    next_check_at: i64,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE watches SET status = ?1, failures = 0, etag = ?2, last_modified = ?3, \
         next_check_at = ?4 WHERE id = ?5",
        params![
            WatchStatus::Ok.as_str(),
            etag,
            last_modified,
            next_check_at,
            id
        ],
    )?;
    Ok(())
}

/// Records a failed check: failures incremented, `status` and next check stored,
/// validators left as they were.
///
/// Returns `DbError::Sqlite` if the update fails.
pub fn record_failure(
    conn: &Connection,
    id: i64,
    status: WatchStatus,
    next_check_at: i64,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE watches SET failures = failures + 1, status = ?1, next_check_at = ?2 \
         WHERE id = ?3",
        params![status.as_str(), next_check_at, id],
    )?;
    Ok(())
}

/// Sets the target price of the watch with row id `id`; `None` clears the target and its currency.
/// Returns whether a row was updated.
///
/// Returns `DbError::Sqlite` if the update fails.
pub fn set_target(conn: &Connection, id: i64, target: Option<&Money>) -> Result<bool, DbError> {
    let updated = conn.execute(
        "UPDATE watches SET target_minor = ?1, currency = ?2 WHERE id = ?3",
        params![
            target.map(|target| target.minor),
            target.map(|target| target.currency.code()),
            id,
        ],
    )?;
    Ok(updated > 0)
}

/// Sets the check interval, in seconds, of the watch with row id `id`; returns whether a row was
/// updated.
///
/// Returns `DbError::Sqlite` if the update fails.
pub fn set_interval(conn: &Connection, id: i64, interval_s: i64) -> Result<bool, DbError> {
    let updated = conn.execute(
        "UPDATE watches SET interval_s = ?1 WHERE id = ?2",
        params![interval_s, id],
    )?;
    Ok(updated > 0)
}

/// Enables or disables the watch with row id `id`; returns whether a row was updated.
///
/// Returns `DbError::Sqlite` if the update fails.
pub fn set_enabled(conn: &Connection, id: i64, enabled: bool) -> Result<bool, DbError> {
    let updated = conn.execute(
        "UPDATE watches SET enabled = ?1 WHERE id = ?2",
        params![i64::from(enabled), id],
    )?;
    Ok(updated > 0)
}

/// Deletes the watch with row id `id`; returns whether a row was deleted.
///
/// Returns `DbError::Sqlite` if the delete fails.
pub fn delete(conn: &Connection, id: i64) -> Result<bool, DbError> {
    let deleted = conn.execute("DELETE FROM watches WHERE id = ?1", params![id])?;
    Ok(deleted > 0)
}

/// Runs a select on `watches` with the given trailing clauses and returns the first watch.
fn query_optional(
    conn: &Connection,
    clauses: &str,
    parameters: impl rusqlite::Params,
) -> Result<Option<Watch>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM watches {clauses}");
    let row = conn.query_row(&sql, parameters, from_row).optional()?;
    row.transpose()
}

/// Runs a select on `watches` with the given trailing clauses and returns every watch.
fn query_all(
    conn: &Connection,
    clauses: &str,
    parameters: impl rusqlite::Params,
) -> Result<Vec<Watch>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM watches {clauses}");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(parameters, from_row)?;
    let mut watches = Vec::new();
    for row in rows {
        watches.push(row??);
    }
    Ok(watches)
}

/// Reads a `COLUMNS` row into a `Watch`.
///
/// The outer error is a SQLite read failure; the inner one is `DbError::Corrupt` for an
/// unknown status or currency code.
fn from_row(row: &Row) -> rusqlite::Result<Result<Watch, DbError>> {
    let target_minor: Option<i64> = row.get(4)?;
    let currency: Option<String> = row.get(5)?;
    let status: String = row.get(8)?;
    let id: i64 = row.get(0)?;
    let target = match parse_target(id, target_minor, currency.as_deref()) {
        Ok(target) => target,
        Err(error) => return Ok(Err(error)),
    };
    let Some(status) = WatchStatus::from_code(&status) else {
        return Ok(Err(DbError::Corrupt(format!(
            "watch {id} has unknown status {status:?}"
        ))));
    };
    Ok(Ok(Watch {
        id,
        url: row.get(1)?,
        name: row.get(2)?,
        selector: row.get(3)?,
        target,
        interval_s: row.get(6)?,
        enabled: row.get::<_, i64>(7)? != 0,
        status,
        etag: row.get(9)?,
        last_modified: row.get(10)?,
        next_check_at: row.get(11)?,
        failures: row.get(12)?,
        created_at: row.get(13)?,
    }))
}

/// Combines the stored target columns into a `Money`, or `None` if no target is set.
///
/// Returns `DbError::Corrupt` if the currency code is unknown, or if only one column is set.
fn parse_target(
    id: i64,
    minor: Option<i64>,
    currency: Option<&str>,
) -> Result<Option<Money>, DbError> {
    match (minor, currency) {
        (None, None) => Ok(None),
        (Some(minor), Some(code)) => {
            let currency = Currency::from_code(code).ok_or_else(|| {
                DbError::Corrupt(format!("watch {id} has unknown currency {code:?}"))
            })?;
            Ok(Some(Money { minor, currency }))
        }
        _ => Err(DbError::Corrupt(format!(
            "watch {id} has a target amount without a currency, or the reverse"
        ))),
    }
}
