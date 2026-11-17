//! Page handlers for the watch list.

use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::response::Html;
use axum::routing::get;

use super::chart::sparkline;
use super::{AppState, WebError};
use crate::db::{self, Observation, Watch};

const SPARKLINE_WIDTH: u32 = 120;
const SPARKLINE_HEIGHT: u32 = 30;
const NO_PRICE: &str = "—";

/// The routes of the watch list.
pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(dashboard))
}

/// The price change shown beside a watch's latest price.
struct PriceChange {
    class: &'static str,
    text: String,
}

/// One table row of the dashboard.
struct WatchRow {
    id: i64,
    title: String,
    latest_price: String,
    change: Option<PriceChange>,
    status_code: &'static str,
    paused: bool,
    sparkline: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    rows: Vec<WatchRow>,
}

async fn dashboard(State(state): State<AppState>) -> Result<Html<String>, WebError> {
    let rows = {
        let conn = state
            .db
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        db::watches::list(&conn)?
            .into_iter()
            .map(|watch| {
                let history = db::observations::history(&conn, watch.id)?;
                Ok(watch_row(watch, &history))
            })
            .collect::<Result<Vec<_>, WebError>>()?
    };
    Ok(Html(DashboardTemplate { rows }.render()?))
}

/// Builds the dashboard row for a watch from its history, oldest first.
fn watch_row(watch: Watch, history: &[Observation]) -> WatchRow {
    let latest = history.last();
    WatchRow {
        id: watch.id,
        title: watch.name.unwrap_or(watch.url),
        latest_price: latest.map_or_else(|| NO_PRICE.to_string(), |last| last.price.to_string()),
        change: price_change(history),
        status_code: watch.status.as_str(),
        paused: !watch.enabled,
        sparkline: sparkline(history, SPARKLINE_WIDTH, SPARKLINE_HEIGHT),
    }
}

/// Compares the latest price with the previous observation in the same currency.
///
/// Returns `None` when there is no such observation or the prices are equal.
fn price_change(history: &[Observation]) -> Option<PriceChange> {
    let (latest, earlier) = history.split_last()?;
    let previous = earlier
        .iter()
        .rev()
        .find(|observation| observation.price.currency == latest.price.currency)?;
    let difference = latest.price.minor - previous.price.minor;
    let (class, arrow) = match difference {
        0 => return None,
        below if below < 0 => ("down", "▼"),
        _ => ("up", "▲"),
    };
    let amount = crate::money::Money {
        minor: difference.abs(),
        currency: latest.price.currency,
    };
    Some(PriceChange {
        class,
        text: format!("{arrow} {amount}"),
    })
}
