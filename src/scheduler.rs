//! Picks due watches and runs fetch, extract, store, rules and notify for each.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::db::{DbError, NewObservation, Observation, StoredEvent, Watch, WatchStatus};
use crate::db::{events, observations, watches};
use crate::extract::{self, ExtractError};
use crate::fetch::{self, FetchError, Fetched, Fetcher, Validators};
use crate::notify::{self, Notifier};
use crate::rules;

/// How often `run` looks for due watches.
pub const TICK: Duration = Duration::from_secs(30);
/// Most watches checked in one tick.
pub const BATCH: usize = 50;

/// Largest relative move jitter applies to a check interval.
const JITTER_FRACTION: f64 = 0.1;

/// What one tick did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    /// Watches checked.
    pub checked: usize,
    /// Checks that failed.
    pub failed: usize,
    /// Events recorded.
    pub events: usize,
    /// Events delivered to the notifier.
    pub notified: usize,
}

/// Why a page check found no price.
#[derive(Debug, thiserror::Error)]
enum PageFailure {
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
}

/// The ways checking one watch can stop early.
enum CheckError {
    /// The database failed; the whole tick stops.
    Db(DbError),
    /// This watch's page could not be read; the tick goes on.
    Page(PageFailure),
}

impl From<DbError> for CheckError {
    fn from(error: DbError) -> CheckError {
        CheckError::Db(error)
    }
}

impl<T: Into<PageFailure>> From<T> for CheckError {
    fn from(failure: T) -> CheckError {
        CheckError::Page(failure.into())
    }
}

/// Returns `interval_s` moved by up to 10 % according to `unit`.
///
/// `unit` is clamped to [-1, 1]; the result is rounded and never below 1.
pub fn jittered(interval_s: i64, unit: f64) -> i64 {
    let offset = interval_s as f64 * JITTER_FRACTION * unit.clamp(-1.0, 1.0);
    (interval_s as f64 + offset).round().max(1.0) as i64
}

/// Returns a pseudo-random number in [-1, 1] derived from `watch_id` and `now`.
///
/// `RandomState` is seeded per process, which is all the spread jitter needs.
fn random_unit(watch_id: i64, now: i64) -> f64 {
    let hash = RandomState::new().hash_one((watch_id, now));
    hash as f64 / u64::MAX as f64 * 2.0 - 1.0
}

/// Runs one tick at `now` (unix seconds): checks the due watches, then sends pending notifications.
///
/// A failing watch is recorded and logged, and the tick goes on.
/// Returns `DbError` when a database operation fails.
pub async fn tick(
    conn: &Connection,
    fetcher: &Fetcher,
    notifier: Option<&Notifier>,
    now: i64,
) -> Result<TickReport, DbError> {
    let mut report = TickReport::default();
    for watch in watches::due(conn, now, BATCH)? {
        report.checked += 1;
        match check_watch(conn, fetcher, &watch, now).await {
            Ok(recorded) => report.events += recorded,
            Err(CheckError::Db(error)) => return Err(error),
            Err(CheckError::Page(failure)) => {
                report.failed += 1;
                record_page_failure(conn, fetcher, &watch, &failure, now).await?;
            }
        }
    }
    if let Some(notifier) = notifier {
        report.notified = notify_pending(conn, notifier, now).await?;
    }
    Ok(report)
}

/// Runs ticks every `TICK` until `shutdown` completes; the check in progress finishes first.
///
/// Returns `DbError` when a tick fails on the database.
pub async fn run(
    conn: &Connection,
    fetcher: &Fetcher,
    notifier: Option<&Notifier>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), DbError> {
    tokio::pin!(shutdown);
    loop {
        let report = tick(conn, fetcher, notifier, unix_now()).await?;
        tracing::debug!(?report, "tick finished");
        tokio::select! {
            biased;
            () = &mut shutdown => return Ok(()),
            () = tokio::time::sleep(TICK) => {}
        }
    }
}

/// Returns the current time in unix seconds.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or_default()
}

/// Fetches and stores one watch, and returns how many events it recorded.
async fn check_watch(
    conn: &Connection,
    fetcher: &Fetcher,
    watch: &Watch,
    now: i64,
) -> Result<usize, CheckError> {
    let url = url::Url::parse(&watch.url).map_err(PageFailure::from)?;
    let validators = Validators {
        etag: watch.etag.clone(),
        last_modified: watch.last_modified.clone(),
    };
    let fetched = fetch::fetch_page(fetcher, &url, Some(&validators))
        .await
        .map_err(PageFailure::from)?;
    let (recorded, validators) = match fetched {
        Fetched::Body { html, validators } => {
            let extraction =
                extract::extract(&html, watch.selector.as_deref()).map_err(PageFailure::from)?;
            let observation = NewObservation {
                watch_id: watch.id,
                at: now,
                price: extraction.price,
                list_price: extraction.list_price,
                availability: extraction.availability,
                source: extraction.source,
            };
            (
                record_observation(conn, watch, &observation, now)?,
                validators,
            )
        }
        Fetched::NotModified => {
            let recorded = match observations::latest(conn, watch.id)? {
                Some(latest) => record_observation(conn, watch, &repeat_at(&latest, now), now)?,
                None => 0,
            };
            (recorded, validators)
        }
    };
    let next_check_at = now + jittered(watch.interval_s, random_unit(watch.id, now));
    watches::record_success(
        conn,
        watch.id,
        validators.etag.as_deref(),
        validators.last_modified.as_deref(),
        next_check_at,
    )?;
    Ok(recorded)
}

/// Copies `observation` as a new observation made at `at`.
fn repeat_at(observation: &Observation, at: i64) -> NewObservation {
    NewObservation {
        watch_id: observation.watch_id,
        at,
        price: observation.price,
        list_price: observation.list_price,
        availability: observation.availability,
        source: observation.source,
    }
}

/// Stores `new`, evaluates the rules against the earlier observations, and stores the events.
///
/// Returns the number of events stored.
fn record_observation(
    conn: &Connection,
    watch: &Watch,
    new: &NewObservation,
    now: i64,
) -> Result<usize, DbError> {
    let history = observations::history(conn, watch.id)?;
    let latest = observations::insert(conn, new)?;
    let triggered = rules::evaluate(watch, &history, &latest);
    for event in &triggered {
        events::insert(conn, event, now)?;
    }
    Ok(triggered.len())
}

/// Logs a failed check and stores the new status and the backed-off next check time.
async fn record_page_failure(
    conn: &Connection,
    fetcher: &Fetcher,
    watch: &Watch,
    failure: &PageFailure,
    now: i64,
) -> Result<(), DbError> {
    tracing::warn!(watch_id = watch.id, url = %watch.url, error = %failure, "check failed");
    let status = failure_status(watch, failure);
    let next_check_at = failure_next_check(fetcher, watch, failure, now).await;
    watches::record_failure(conn, watch.id, status, next_check_at)
}

/// Returns the status a watch gets after `failure`.
fn failure_status(watch: &Watch, failure: &PageFailure) -> WatchStatus {
    match failure {
        PageFailure::Fetch(FetchError::Blocked) => WatchStatus::Blocked,
        PageFailure::Extract(ExtractError::NoPrice | ExtractError::AmbiguousPrice)
            if watch.selector.is_none() =>
        {
            WatchStatus::NeedsSelector
        }
        _ => WatchStatus::Error,
    }
}

/// Returns when to retry after `failure`: the backed-off interval, and for a 429 not before the host's pause ends.
async fn failure_next_check(
    fetcher: &Fetcher,
    watch: &Watch,
    failure: &PageFailure,
    now: i64,
) -> i64 {
    let interval = Duration::from_secs(u64::try_from(watch.interval_s).unwrap_or_default());
    let failures = u32::try_from(watch.failures.saturating_add(1)).unwrap_or(u32::MAX);
    let delay = fetch::backoff_delay(interval, failures);
    let backoff_until = now.saturating_add(i64::try_from(delay.as_secs()).unwrap_or(i64::MAX));
    let pause_until = match failure {
        PageFailure::Fetch(FetchError::RateLimited) => pause_end(fetcher, watch, now).await,
        _ => None,
    };
    pause_until.map_or(backoff_until, |until| until.max(backoff_until))
}

/// Returns the unix time the watch's host stays paused until, if it is paused.
async fn pause_end(fetcher: &Fetcher, watch: &Watch, now: i64) -> Option<i64> {
    let url = url::Url::parse(&watch.url).ok()?;
    let until = fetcher.paused_until(&url).await?;
    let remaining = until.saturating_duration_since(Instant::now());
    Some(now.saturating_add(remaining.as_secs_f64().ceil() as i64))
}

/// Sends one message per (watch, check time) group of unnotified events and marks the sent ones.
///
/// A send failure is logged and the group stays pending for the next tick.
/// Returns the number of events marked as notified.
async fn notify_pending(
    conn: &Connection,
    notifier: &Notifier,
    now: i64,
) -> Result<usize, DbError> {
    let mut groups: BTreeMap<(i64, i64), Vec<StoredEvent>> = BTreeMap::new();
    for stored in events::unnotified(conn)? {
        groups
            .entry((stored.event.watch_id, stored.at))
            .or_default()
            .push(stored);
    }
    let mut notified = 0;
    for ((watch_id, _), group) in groups {
        let Some(watch) = watches::get(conn, watch_id)? else {
            continue;
        };
        let group_events: Vec<_> = group.iter().map(|stored| stored.event.clone()).collect();
        let message = notify::message_for(&watch, &group_events);
        match notifier.send(&message).await {
            Ok(()) => {
                let ids: Vec<i64> = group.iter().map(|stored| stored.id).collect();
                events::mark_notified(conn, &ids, now)?;
                notified += ids.len();
            }
            Err(error) => {
                tracing::warn!(watch_id, error = %error, "notification failed; will retry");
            }
        }
    }
    Ok(notified)
}
