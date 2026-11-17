//! Page handlers for a single watch.

use axum::Router;

use super::AppState;

/// The routes of a single watch.
pub fn routes() -> Router<AppState> {
    Router::new()
}
