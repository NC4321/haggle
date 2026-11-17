//! Queries on the `observations` table.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::{DbError, NewObservation, Observation};
use crate::extract::{Availability, Source};
use crate::money::{Currency, Money};

/// Stores a new observation and returns it with its row id.
///
/// A `list_price` in a different currency than `price` is stored as NULL.
/// Returns `DbError::Sqlite` if the insert fails, for example when the watch does not exist.
pub fn insert(conn: &Connection, new: &NewObservation) -> Result<Observation, DbError> {
    let list_price = list_price_in_price_currency(new);
    conn.execute(
        INSERT_SQL,
        params![
            new.watch_id,
            new.at,
            new.price.minor,
            list_price.map(|money| money.minor),
            new.price.currency.code(),
            availability_code(new.availability),
            source_code(new.source),
        ],
    )?;
    Ok(Observation {
        id: conn.last_insert_rowid(),
        watch_id: new.watch_id,
        at: new.at,
        price: new.price,
        list_price,
        availability: new.availability,
        source: new.source,
    })
}

/// Returns every observation of a watch, oldest first (by `at`, then by id).
///
/// Returns `DbError::Corrupt` if a stored currency, availability or source code is unknown.
pub fn history(conn: &Connection, watch_id: i64) -> Result<Vec<Observation>, DbError> {
    let mut statement = conn.prepare(HISTORY_SQL)?;
    let rows = statement.query_map(params![watch_id], read_raw)?;
    rows.map(|row| row.map_err(DbError::from).and_then(into_observation))
        .collect()
}

/// Returns the newest observation of a watch, or `None` if it has none.
///
/// Returns `DbError::Corrupt` if a stored currency, availability or source code is unknown.
pub fn latest(conn: &Connection, watch_id: i64) -> Result<Option<Observation>, DbError> {
    let row = conn
        .query_row(LATEST_SQL, params![watch_id], read_raw)
        .optional()?;
    row.map(into_observation).transpose()
}

/// Inserts a row into `observations`; the placeholders follow the column order.
const INSERT_SQL: &str = "INSERT INTO observations \
    (watch_id, at, price_minor, list_price_minor, currency, availability, source) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

/// Selects one watch's observations, oldest first.
const HISTORY_SQL: &str = "SELECT id, watch_id, at, price_minor, list_price_minor, currency, \
    availability, source FROM observations WHERE watch_id = ?1 ORDER BY at, id";

/// Selects one watch's newest observation.
const LATEST_SQL: &str = "SELECT id, watch_id, at, price_minor, list_price_minor, currency, \
    availability, source FROM observations WHERE watch_id = ?1 ORDER BY at DESC, id DESC LIMIT 1";

/// A row of `observations` with its codes still as stored text.
struct RawObservation {
    id: i64,
    watch_id: i64,
    at: i64,
    price_minor: i64,
    list_price_minor: Option<i64>,
    currency: String,
    availability: String,
    source: String,
}

/// Reads one `observations` row, leaving the codes unparsed.
///
/// Returns `rusqlite::Error` if a column is missing or has the wrong type.
fn read_raw(row: &Row<'_>) -> rusqlite::Result<RawObservation> {
    Ok(RawObservation {
        id: row.get("id")?,
        watch_id: row.get("watch_id")?,
        at: row.get("at")?,
        price_minor: row.get("price_minor")?,
        list_price_minor: row.get("list_price_minor")?,
        currency: row.get("currency")?,
        availability: row.get("availability")?,
        source: row.get("source")?,
    })
}

/// Turns a stored row into an `Observation`, parsing its codes.
///
/// Returns `DbError::Corrupt` if the currency, availability or source code is unknown.
fn into_observation(raw: RawObservation) -> Result<Observation, DbError> {
    let currency = currency_from_code(&raw.currency)?;
    Ok(Observation {
        id: raw.id,
        watch_id: raw.watch_id,
        at: raw.at,
        price: Money {
            minor: raw.price_minor,
            currency,
        },
        list_price: raw.list_price_minor.map(|minor| Money { minor, currency }),
        availability: availability_from_code(&raw.availability)?,
        source: source_from_code(&raw.source)?,
    })
}

/// Returns the list price if it is in the same currency as the price, otherwise `None`.
fn list_price_in_price_currency(new: &NewObservation) -> Option<Money> {
    new.list_price
        .filter(|list_price| list_price.currency == new.price.currency)
}

/// Parses a stored ISO 4217 currency code.
///
/// Returns `DbError::Corrupt` if `code` is not a supported currency code.
fn currency_from_code(code: &str) -> Result<Currency, DbError> {
    Currency::from_code(code)
        .ok_or_else(|| DbError::Corrupt(format!("unknown currency code {code}")))
}

/// Returns the code an availability is stored under.
fn availability_code(availability: Availability) -> &'static str {
    match availability {
        Availability::InStock => "in_stock",
        Availability::OutOfStock => "out_of_stock",
        Availability::Preorder => "preorder",
        Availability::Unknown => "unknown",
    }
}

/// Parses a stored availability code.
///
/// Returns `DbError::Corrupt` if `code` is not a known availability code.
fn availability_from_code(code: &str) -> Result<Availability, DbError> {
    match code {
        "in_stock" => Ok(Availability::InStock),
        "out_of_stock" => Ok(Availability::OutOfStock),
        "preorder" => Ok(Availability::Preorder),
        "unknown" => Ok(Availability::Unknown),
        _ => Err(DbError::Corrupt(format!(
            "unknown availability code {code}"
        ))),
    }
}

/// Returns the code a source is stored under.
fn source_code(source: Source) -> &'static str {
    match source {
        Source::JsonLd => "json_ld",
        Source::Meta => "meta",
        Source::Microdata => "microdata",
        Source::Selector => "selector",
    }
}

/// Parses a stored source code.
///
/// Returns `DbError::Corrupt` if `code` is not a known source code.
fn source_from_code(code: &str) -> Result<Source, DbError> {
    match code {
        "json_ld" => Ok(Source::JsonLd),
        "meta" => Ok(Source::Meta),
        "microdata" => Ok(Source::Microdata),
        "selector" => Ok(Source::Selector),
        _ => Err(DbError::Corrupt(format!("unknown source code {code}"))),
    }
}
