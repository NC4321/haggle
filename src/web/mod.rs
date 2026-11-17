//! The web interface: shared state, router, server and error type.

pub mod add_page;
pub mod chart;
pub mod pages;
pub mod watch_page;

use std::future::Future;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use time::OffsetDateTime;
use time::macros::format_description;

use crate::db::{DbError, EventKind};
use crate::fetch::Fetcher;

const STYLESHEET: &str = include_str!("../../static/style.css");
const STYLESHEET_CONTENT_TYPE: &str = "text/css; charset=utf-8";

/// State shared by every request handler.
#[derive(Clone)]
pub struct AppState {
    /// The database connection, guarded for use from handlers.
    pub db: Arc<Mutex<rusqlite::Connection>>,
    /// The fetcher used to check pages when a watch is added.
    pub fetcher: Arc<Fetcher>,
}

/// Errors a handler can return; each maps to an HTTP response.
#[derive(Debug, thiserror::Error)]
pub enum WebError {
    /// The database layer failed.
    #[error("database error: {0}")]
    Db(#[from] DbError),
    /// The requested resource does not exist.
    #[error("not found")]
    NotFound,
    /// The request was invalid; the message tells the user why.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A template failed to render.
    #[error("render error: {0}")]
    Render(#[from] askama::Error),
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        match self {
            WebError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            WebError::BadRequest(message) => (StatusCode::BAD_REQUEST, message).into_response(),
            error @ (WebError::Db(_) | WebError::Render(_)) => {
                tracing::error!(%error, "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
            }
        }
    }
}

/// Builds the router with all pages and the stylesheet.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(pages::routes())
        .merge(watch_page::routes())
        .merge(add_page::routes())
        .route("/static/style.css", get(stylesheet))
        .with_state(state)
}

async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, STYLESHEET_CONTENT_TYPE)],
        STYLESHEET,
    )
}

/// Serves the web interface on `listener` until `shutdown` completes.
///
/// Returns an error if the server fails on I/O.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

/// Returns the label shown to the user for an event kind.
pub fn event_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::PriceDrop => "Price drop",
        EventKind::PriceRise => "Price rise",
        EventKind::TargetHit => "Target hit",
        EventKind::LowestEver => "Lowest ever",
        EventKind::BackInStock => "Back in stock",
        EventKind::OutOfStock => "Out of stock",
        EventKind::InflatedSale => "Inflated sale",
    }
}

/// Formats Unix seconds as UTC `YYYY-MM-DD HH:MM`.
///
/// Returns an empty string for timestamps outside the representable range.
pub fn format_time(unix_seconds: i64) -> String {
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]");
    OffsetDateTime::from_unix_timestamp(unix_seconds)
        .ok()
        .and_then(|moment| moment.format(&format).ok())
        .unwrap_or_default()
}
