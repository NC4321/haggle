//! Page handlers for the add-watch form: the form itself, the preview and the insert.

use std::sync::PoisonError;
use std::time::{SystemTime, UNIX_EPOCH};

use askama::Template;
use axum::Router;
use axum::extract::{Form, State};
use axum::response::{Html, Redirect};
use axum::routing::{get, post};
use serde::Deserialize;

use super::{AppState, WebError};
use crate::db::NewWatch;
use crate::db::watches;
use crate::extract::{self, Extraction};
use crate::fetch::{self, Fetched};
use crate::money::{self, Currency, Money, MoneyError};

/// The check interval pre-filled in the form.
const DEFAULT_EVERY: &str = "6h";

/// The shortest allowed check interval, in seconds.
const MIN_INTERVAL_SECONDS: i64 = 300;

/// The currency assumed for a target price written without one.
const DEFAULT_TARGET_CURRENCY: Currency = Currency::Eur;

/// The fields of the add-watch form, as submitted.
#[derive(Debug, Deserialize)]
struct AddForm {
    url: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    selector: String,
    #[serde(default)]
    target: String,
    #[serde(default = "default_every")]
    every: String,
}

fn default_every() -> String {
    DEFAULT_EVERY.to_string()
}

/// What a successful extraction looks like in the preview box.
struct Preview {
    price: String,
    list_price: Option<String>,
    availability: String,
    title: Option<String>,
    source: String,
}

impl From<Extraction> for Preview {
    fn from(extraction: Extraction) -> Preview {
        Preview {
            price: extraction.price.to_string(),
            list_price: extraction.list_price.map(|price| price.to_string()),
            availability: extraction.availability.to_string(),
            title: extraction.title,
            source: extraction.source.to_string(),
        }
    }
}

/// The add-watch page: the form with its current values and an optional preview.
#[derive(Template)]
#[template(path = "add.html")]
struct AddTemplate {
    url: String,
    name: String,
    selector: String,
    target: String,
    every: String,
    preview: Option<Preview>,
    preview_error: Option<String>,
}

impl AddTemplate {
    fn from_form(form: AddForm) -> AddTemplate {
        AddTemplate {
            url: form.url,
            name: form.name,
            selector: form.selector,
            target: form.target,
            every: form.every,
            preview: None,
            preview_error: None,
        }
    }
}

/// The routes of the add-watch form.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/add", get(show_form).post(add_watch))
        .route("/add/preview", post(preview))
}

async fn show_form() -> Result<Html<String>, WebError> {
    let page = AddTemplate::from_form(AddForm {
        url: String::new(),
        name: String::new(),
        selector: String::new(),
        target: String::new(),
        every: default_every(),
    });
    Ok(Html(page.render()?))
}

/// Fetches and extracts the page of the submitted URL and shows the result without storing anything.
///
/// Returns `WebError::BadRequest` if the URL is not an http or https URL; fetch and
/// extraction failures are shown in the preview box instead.
async fn preview(
    State(state): State<AppState>,
    Form(form): Form<AddForm>,
) -> Result<Html<String>, WebError> {
    let url = parse_url(&form.url)?;
    let selector = non_empty(&form.selector);
    let outcome = match fetch::fetch(&state.fetcher, &url, None).await {
        Ok(Fetched::Body { html, .. }) => {
            extract::extract(&html, selector.as_deref()).map_err(|error| error.to_string())
        }
        Ok(Fetched::NotModified) => Err("the page was not modified".to_string()),
        Err(error) => Err(error.to_string()),
    };
    let mut page = AddTemplate::from_form(form);
    match outcome {
        Ok(extraction) => page.preview = Some(extraction.into()),
        Err(message) => page.preview_error = Some(message),
    }
    Ok(Html(page.render()?))
}

/// Validates the form, stores a new watch and redirects to its page.
///
/// Returns `WebError::BadRequest` for an invalid URL, interval or target, or a URL that
/// is already watched, and `WebError::Db` if the database fails.
async fn add_watch(
    State(state): State<AppState>,
    Form(form): Form<AddForm>,
) -> Result<Redirect, WebError> {
    parse_url(&form.url)?;
    let new = NewWatch {
        url: form.url.trim().to_string(),
        name: non_empty(&form.name),
        selector: non_empty(&form.selector),
        target: parse_target(&form.target)?,
        interval_s: parse_interval(&form.every)?,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let conn = state.db.lock().unwrap_or_else(PoisonError::into_inner);
    if watches::find_by_url(&conn, &new.url)?.is_some() {
        return Err(WebError::BadRequest("already watched".to_string()));
    }
    let id = watches::insert(&conn, &new, now)?;
    Ok(Redirect::to(&format!("/watches/{id}")))
}

/// Parses `text` as an http or https URL.
fn parse_url(text: &str) -> Result<url::Url, WebError> {
    let url = url::Url::parse(text.trim())
        .map_err(|error| WebError::BadRequest(format!("invalid URL: {error}")))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        _ => Err(WebError::BadRequest(
            "the URL must start with http:// or https://".to_string(),
        )),
    }
}

/// Returns the trimmed text, or `None` if it is blank.
fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Parses an interval such as `6h` (units s, m, h, d) into seconds, at least the minimum.
fn parse_interval(text: &str) -> Result<i64, WebError> {
    let invalid = || WebError::BadRequest(format!("invalid interval: {}", text.trim()));
    let text = text.trim();
    let unit = text.chars().next_back().ok_or_else(invalid)?;
    let unit_seconds: i64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
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

/// Parses the target price; blank is no target and a price without currency is in the default one.
fn parse_target(text: &str) -> Result<Option<Money>, WebError> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let parsed = match money::parse_price(text, None) {
        Err(MoneyError::NoCurrency) => money::parse_price(text, Some(DEFAULT_TARGET_CURRENCY)),
        other => other,
    };
    parsed
        .map(Some)
        .map_err(|error| WebError::BadRequest(format!("invalid target price: {error}")))
}
