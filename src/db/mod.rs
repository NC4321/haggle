//! Storage types shared by the database modules: row types, event kinds and errors.

pub mod events;
pub mod observations;
pub mod watches;

use crate::extract::{Availability, Source};
use crate::money::Money;

/// Errors from the database layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A watch for this URL is already stored.
    #[error("a watch for {0} already exists")]
    DuplicateUrl(String),
    /// A stored value could not be read back as the type it should be.
    #[error("bad stored value: {0}")]
    Corrupt(String),
}

/// Health of a watch after its most recent check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStatus {
    /// The last check found a price.
    Ok,
    /// The page has no price the extractor can find; a CSS selector is needed.
    NeedsSelector,
    /// The shop refused the check, through robots.txt or with a 403.
    Blocked,
    /// The last check failed for another reason.
    Error,
}

impl WatchStatus {
    /// Returns the code this status is stored under in the `watches` table.
    pub fn as_str(self) -> &'static str {
        match self {
            WatchStatus::Ok => "ok",
            WatchStatus::NeedsSelector => "needs-selector",
            WatchStatus::Blocked => "blocked",
            WatchStatus::Error => "error",
        }
    }

    /// Parses a stored status code.
    ///
    /// Returns `None` if `code` is not a known status code.
    pub fn from_code(code: &str) -> Option<WatchStatus> {
        match code {
            "ok" => Some(WatchStatus::Ok),
            "needs-selector" => Some(WatchStatus::NeedsSelector),
            "blocked" => Some(WatchStatus::Blocked),
            "error" => Some(WatchStatus::Error),
            _ => None,
        }
    }
}

/// A watched product page, as stored in the `watches` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    /// Row id.
    pub id: i64,
    /// Product page URL; unique across watches.
    pub url: String,
    /// Name the user gave the watch, if any.
    pub name: Option<String>,
    /// CSS selector for the price, if the user supplied one.
    pub selector: Option<String>,
    /// The price the user wants to pay, if set.
    pub target: Option<Money>,
    /// Seconds between checks.
    pub interval_s: i64,
    /// Whether the scheduler checks this watch.
    pub enabled: bool,
    /// Health after the most recent check.
    pub status: WatchStatus,
    /// ETag of the last successful fetch, sent back for a conditional GET.
    pub etag: Option<String>,
    /// Last-Modified value of the last successful fetch, sent back for a conditional GET.
    pub last_modified: Option<String>,
    /// Unix seconds of the next due check.
    pub next_check_at: i64,
    /// Checks that have failed in a row.
    pub failures: i64,
    /// Unix seconds when the watch was added.
    pub created_at: i64,
}

/// Fields needed to add a watch; the database fills in the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWatch {
    /// Product page URL; must not already be watched.
    pub url: String,
    /// Name the user gave the watch, if any.
    pub name: Option<String>,
    /// CSS selector for the price, if the user supplied one.
    pub selector: Option<String>,
    /// The price the user wants to pay, if set.
    pub target: Option<Money>,
    /// Seconds between checks.
    pub interval_s: i64,
}

/// One successful check of a watch, before it has a row id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewObservation {
    /// The watch that was checked.
    pub watch_id: i64,
    /// Unix seconds when the check ran.
    pub at: i64,
    /// The price found on the page.
    pub price: Money,
    /// The shop's own "was" price, if the page states one.
    pub list_price: Option<Money>,
    /// Stock state found on the page.
    pub availability: Availability,
    /// Which extraction strategy found the price.
    pub source: Source,
}

/// One successful check of a watch, as stored in the `observations` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// Row id.
    pub id: i64,
    /// The watch that was checked.
    pub watch_id: i64,
    /// Unix seconds when the check ran.
    pub at: i64,
    /// The price found on the page.
    pub price: Money,
    /// The shop's own "was" price, if the page states one.
    pub list_price: Option<Money>,
    /// Stock state found on the page.
    pub availability: Availability,
    /// Which extraction strategy found the price.
    pub source: Source,
}

/// What a new observation means for a watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// The price fell since the previous observation.
    PriceDrop,
    /// The price rose since the previous observation.
    PriceRise,
    /// The price reached the watch's target.
    TargetHit,
    /// The price is the lowest the watch has seen.
    LowestEver,
    /// The item came back into stock.
    BackInStock,
    /// The item went out of stock.
    OutOfStock,
    /// The shop's "was" price is inflated relative to the observed history.
    InflatedSale,
}

impl EventKind {
    /// Returns the code this kind is stored under in the `events` table.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::PriceDrop => "price_drop",
            EventKind::PriceRise => "price_rise",
            EventKind::TargetHit => "target_hit",
            EventKind::LowestEver => "lowest_ever",
            EventKind::BackInStock => "back_in_stock",
            EventKind::OutOfStock => "out_of_stock",
            EventKind::InflatedSale => "inflated_sale",
        }
    }

    /// Parses a stored event code.
    ///
    /// Returns `None` if `code` is not a known event code.
    pub fn from_code(code: &str) -> Option<EventKind> {
        match code {
            "price_drop" => Some(EventKind::PriceDrop),
            "price_rise" => Some(EventKind::PriceRise),
            "target_hit" => Some(EventKind::TargetHit),
            "lowest_ever" => Some(EventKind::LowestEver),
            "back_in_stock" => Some(EventKind::BackInStock),
            "out_of_stock" => Some(EventKind::OutOfStock),
            "inflated_sale" => Some(EventKind::InflatedSale),
            _ => None,
        }
    }
}

/// An event produced by the rules for a watch, before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The watch the event belongs to.
    pub watch_id: i64,
    /// What happened.
    pub kind: EventKind,
    /// The price before the change, if the event compares two prices.
    pub old: Option<Money>,
    /// The price after the change, if the event compares two prices.
    pub new: Option<Money>,
}

/// An event as stored in the `events` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    /// Row id.
    pub id: i64,
    /// Unix seconds when the event was recorded.
    pub at: i64,
    /// Unix seconds when the notification was sent, or `None` while it is pending.
    pub notified_at: Option<i64>,
    /// The event itself.
    pub event: Event,
}
