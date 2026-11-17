//! Page handlers for a single watch: the detail page, editing, pausing and deleting.

use std::sync::PoisonError;

use askama::Template;
use axum::Router;
use axum::extract::{Form, Path, State};
use axum::response::{Html, Redirect};
use axum::routing::{get, post};
use serde::Deserialize;

use super::{AppState, WebError, chart, event_label, format_time};
use crate::db::{events, observations, watches};
use crate::money::{self, Currency, Money};

/// The shortest allowed check interval, in seconds.
const MIN_INTERVAL_SECONDS: i64 = 300;

/// The currency assumed for a target when the watch has neither a target nor an observation.
const DEFAULT_TARGET_CURRENCY: Currency = Currency::Eur;

const CHART_WIDTH: u32 = 640;
const CHART_HEIGHT: u32 = 240;

const SECONDS_PER_DAY: i64 = 86_400;
const SECONDS_PER_HOUR: i64 = 3600;
const SECONDS_PER_MINUTE: i64 = 60;

/// The fields of the edit form, as submitted.
#[derive(Debug, Deserialize)]
struct EditForm {
    #[serde(default)]
    target: String,
    every: String,
}

/// One row of the events list.
struct EventRow {
    time: String,
    label: &'static str,
    /// The old and new price, when the event compares two prices.
    change: Option<(String, String)>,
}

/// The watch detail page.
#[derive(Template)]
#[template(path = "watch.html")]
struct WatchTemplate {
    id: i64,
    heading: String,
    url: String,
    status: &'static str,
    enabled: bool,
    latest_price: Option<String>,
    chart: String,
    events: Vec<EventRow>,
    target: String,
    every: String,
}

/// The routes of a single watch.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/watches/{id}", get(show_watch).post(edit_watch))
        .route("/watches/{id}/pause", post(toggle_pause))
        .route("/watches/{id}/delete", post(delete_watch))
}

/// Shows the watch with its price chart and events.
///
/// Returns `WebError::BadRequest` for a non-integer id, `WebError::NotFound` for an unknown
/// watch, and `WebError::Db` or `WebError::Render` on failure.
async fn show_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Html<String>, WebError> {
    let id = parse_id(&id)?;
    let (watch, history, stored_events) = {
        let conn = state.db.lock().unwrap_or_else(PoisonError::into_inner);
        let watch = watches::get(&conn, id)?.ok_or(WebError::NotFound)?;
        let history = observations::history(&conn, id)?;
        let stored_events = events::for_watch(&conn, id)?;
        (watch, history, stored_events)
    };
    let page = WatchTemplate {
        id,
        heading: watch.name.clone().unwrap_or_else(|| watch.url.clone()),
        url: watch.url.clone(),
        status: watch.status.as_str(),
        enabled: watch.enabled,
        latest_price: history
            .last()
            .map(|observation| observation.price.to_string()),
        chart: chart::price_chart(&history, watch.target.as_ref(), CHART_WIDTH, CHART_HEIGHT),
        events: stored_events
            .iter()
            .rev()
            .map(|stored| EventRow {
                time: format_time(stored.at),
                label: event_label(stored.event.kind),
                change: stored
                    .event
                    .old
                    .zip(stored.event.new)
                    .map(|(old, new)| (old.to_string(), new.to_string())),
            })
            .collect(),
        target: watch.target.map(target_number).unwrap_or_default(),
        every: format_interval(watch.interval_s),
    };
    Ok(Html(page.render()?))
}

/// Updates the target and check interval, then redirects to the watch page.
///
/// Returns `WebError::BadRequest` for an invalid id, target or interval, and
/// `WebError::NotFound` for an unknown watch.
async fn edit_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<EditForm>,
) -> Result<Redirect, WebError> {
    let id = parse_id(&id)?;
    let interval_s = parse_interval(&form.every)?;
    let conn = state.db.lock().unwrap_or_else(PoisonError::into_inner);
    let watch = watches::get(&conn, id)?.ok_or(WebError::NotFound)?;
    let target = if form.target.trim().is_empty() {
        None
    } else {
        let currency = match watch.target {
            Some(target) => target.currency,
            None => observations::latest(&conn, id)?
                .map_or(DEFAULT_TARGET_CURRENCY, |observation| {
                    observation.price.currency
                }),
        };
        let price = money::parse_price(&form.target, Some(currency))
            .map_err(|error| WebError::BadRequest(format!("invalid target: {error}")))?;
        Some(price)
    };
    watches::set_target(&conn, id, target.as_ref())?;
    watches::set_interval(&conn, id, interval_s)?;
    Ok(see_watch(id))
}

/// Pauses a running watch or resumes a paused one, then redirects to the watch page.
///
/// Returns `WebError::BadRequest` for an invalid id and `WebError::NotFound` for an unknown watch.
async fn toggle_pause(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Redirect, WebError> {
    let id = parse_id(&id)?;
    let conn = state.db.lock().unwrap_or_else(PoisonError::into_inner);
    let watch = watches::get(&conn, id)?.ok_or(WebError::NotFound)?;
    watches::set_enabled(&conn, id, !watch.enabled)?;
    Ok(see_watch(id))
}

/// Deletes the watch, then redirects to the watch list.
///
/// Returns `WebError::BadRequest` for an invalid id and `WebError::NotFound` for an unknown watch.
async fn delete_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Redirect, WebError> {
    let id = parse_id(&id)?;
    let deleted = {
        let conn = state.db.lock().unwrap_or_else(PoisonError::into_inner);
        watches::delete(&conn, id)?
    };
    if deleted {
        Ok(Redirect::to("/"))
    } else {
        Err(WebError::NotFound)
    }
}

fn see_watch(id: i64) -> Redirect {
    Redirect::to(&format!("/watches/{id}"))
}

/// Parses the `{id}` path segment.
fn parse_id(text: &str) -> Result<i64, WebError> {
    text.parse()
        .map_err(|_| WebError::BadRequest(format!("invalid watch id: {text}")))
}

/// Returns the amount of a target without its currency code, such as `150.00`.
fn target_number(target: Money) -> String {
    let text = target.to_string();
    text.split_once(' ')
        .map_or(text.clone(), |(number, _code)| number.to_string())
}

/// Formats seconds as the largest whole unit among d, h, m, else s, such as `6h`.
fn format_interval(seconds: i64) -> String {
    [
        (SECONDS_PER_DAY, 'd'),
        (SECONDS_PER_HOUR, 'h'),
        (SECONDS_PER_MINUTE, 'm'),
    ]
    .into_iter()
    .find(|(unit_seconds, _)| seconds > 0 && seconds % unit_seconds == 0)
    .map_or_else(
        || format!("{seconds}s"),
        |(unit_seconds, unit)| format!("{}{unit}", seconds / unit_seconds),
    )
}

/// Parses an interval such as `6h` (units s, m, h, d) into seconds, at least the minimum.
fn parse_interval(text: &str) -> Result<i64, WebError> {
    let text = text.trim();
    let invalid = || WebError::BadRequest(format!("invalid interval: {text}"));
    let unit = text.chars().next_back().ok_or_else(invalid)?;
    let unit_seconds: i64 = match unit {
        's' => 1,
        'm' => SECONDS_PER_MINUTE,
        'h' => SECONDS_PER_HOUR,
        'd' => SECONDS_PER_DAY,
        _ => return Err(invalid()),
    };
    let count: i64 = text[..text.len() - 1].parse().map_err(|_| invalid())?;
    let seconds = count.checked_mul(unit_seconds).ok_or_else(invalid)?;
    if seconds < MIN_INTERVAL_SECONDS {
        return Err(WebError::BadRequest(format!(
            "the interval must be at least {MIN_INTERVAL_SECONDS} seconds"
        )));
    }
    Ok(seconds)
}
