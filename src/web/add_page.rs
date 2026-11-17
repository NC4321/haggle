//! Page handlers for the add-watch form.

use axum::Router;

use super::AppState;

/// The routes of the add-watch form.
pub fn routes() -> Router<AppState> {
    Router::new()
}
