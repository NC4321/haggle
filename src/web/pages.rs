//! Page handlers for the watch list.

use axum::Router;

use super::AppState;

/// The routes of the watch list.
pub fn routes() -> Router<AppState> {
    Router::new()
}
