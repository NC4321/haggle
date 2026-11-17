use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use askama::Error as AskamaError;
use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use haggle::db::{DbError, EventKind, open};
use haggle::fetch::Fetcher;
use haggle::web::{AppState, WebError, event_label, format_time, serve};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// How long a test waits for serve to return after its shutdown signal.
const DEADLINE: Duration = Duration::from_secs(5);

/// The path the router serves the stylesheet on.
const STYLE_PATH: &str = "/static/style.css";

/// A database path in the system temp directory, unique to one test. The file and SQLite's
/// sidecar files are removed when the value is created and again when it is dropped.
struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let path = env::temp_dir().join(format!("haggle-web-router-{}-{name}.db", process::id()));
        let db = Self { path };
        db.remove_files();
        db
    }

    fn remove_files(&self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            fs::remove_file(name).ok();
        }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        self.remove_files();
    }
}

/// A `serve` task on 127.0.0.1 with a fresh database. Dropping it aborts the task.
struct Server {
    base: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}

impl Server {
    async fn start(db: &TempDb) -> Self {
        let conn = open(&db.path).expect("open the test database");
        let fetcher = Fetcher::new(Duration::from_secs(5), 1_000_000).expect("build the fetcher");
        let state = AppState {
            db: Arc::new(Mutex::new(conn)),
            fetcher: Arc::new(fetcher),
        };
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let address = listener.local_addr().expect("read the bound address");
        let (shutdown, signal) = oneshot::channel::<()>();
        let task = tokio::spawn(serve(listener, state, async move {
            signal.await.ok();
        }));
        Self {
            base: format!("http://{address}"),
            shutdown: Some(shutdown),
            task,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Fires the shutdown signal and waits up to 5 s for serve to return its result.
    async fn shut_down(&mut self) -> io::Result<()> {
        let shutdown = self
            .shutdown
            .take()
            .expect("the shutdown signal is sent once");
        shutdown
            .send(())
            .expect("serve is still waiting for the shutdown signal");
        timeout(DEADLINE, &mut self.task)
            .await
            .expect("serve did not return within 5 s")
            .expect("the serve task did not panic")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Sends a GET request for `path` to the running server.
async fn get(server: &Server, path: &str) -> reqwest::Response {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build the HTTP client")
        .get(server.url(path))
        .send()
        .await
        .expect("send the request")
}

/// Reads a response body as UTF-8 text.
async fn body_text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 1024)
        .await
        .expect("read the response body");
    String::from_utf8(bytes.to_vec()).expect("body is UTF-8")
}

#[tokio::test]
async fn style_css_answers_200_ok() {
    let db = TempDb::new("style-status");
    let server = Server::start(&db).await;

    let response = get(&server, STYLE_PATH).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn style_css_content_type_starts_with_text_css() {
    let db = TempDb::new("style-type-prefix");
    let server = Server::start(&db).await;

    let response = get(&server, STYLE_PATH).await;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .expect("the stylesheet has a content-type header")
        .to_str()
        .expect("content-type is ASCII");
    assert!(
        content_type.starts_with("text/css"),
        "content-type: {content_type}"
    );
}

#[tokio::test]
async fn style_css_content_type_is_text_css_with_utf8_charset() {
    let db = TempDb::new("style-type-exact");
    let server = Server::start(&db).await;

    let response = get(&server, STYLE_PATH).await;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .expect("the stylesheet has a content-type header")
        .to_str()
        .expect("content-type is ASCII");
    assert_eq!(content_type, "text/css; charset=utf-8");
}

#[tokio::test]
async fn style_css_body_is_the_bundled_stylesheet() {
    let db = TempDb::new("style-body");
    let server = Server::start(&db).await;

    let response = get(&server, STYLE_PATH).await;

    let body = response.text().await.expect("read the stylesheet body");
    assert_eq!(body, include_str!("../static/style.css"));
}

#[tokio::test]
async fn unknown_path_answers_404() {
    let db = TempDb::new("unknown-path");
    let server = Server::start(&db).await;

    let response = get(&server, "/no-such-page").await;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn serve_returns_ok_after_the_shutdown_signal() {
    let db = TempDb::new("shutdown");
    let mut server = Server::start(&db).await;

    let result = server.shut_down().await;

    assert!(result.is_ok(), "serve returned {result:?}");
}

#[test]
fn event_label_for_price_drop_is_price_drop() {
    assert_eq!(event_label(EventKind::PriceDrop), "Price drop");
}

#[test]
fn event_label_for_price_rise_is_price_rise() {
    assert_eq!(event_label(EventKind::PriceRise), "Price rise");
}

#[test]
fn event_label_for_target_hit_is_target_hit() {
    assert_eq!(event_label(EventKind::TargetHit), "Target hit");
}

#[test]
fn event_label_for_lowest_ever_is_lowest_ever() {
    assert_eq!(event_label(EventKind::LowestEver), "Lowest ever");
}

#[test]
fn event_label_for_back_in_stock_is_back_in_stock() {
    assert_eq!(event_label(EventKind::BackInStock), "Back in stock");
}

#[test]
fn event_label_for_out_of_stock_is_out_of_stock() {
    assert_eq!(event_label(EventKind::OutOfStock), "Out of stock");
}

#[test]
fn event_label_for_inflated_sale_is_inflated_sale() {
    assert_eq!(event_label(EventKind::InflatedSale), "Inflated sale");
}

#[test]
fn format_time_of_zero_is_the_unix_epoch() {
    assert_eq!(format_time(0), "1970-01-01 00:00");
}

#[test]
fn format_time_of_one_day_one_hour_one_minute_is_the_second_day() {
    // 86_400 s is one day, 3_600 s one hour and 60 s one minute.
    assert_eq!(format_time(86_400 + 3_600 + 60), "1970-01-02 01:01");
}

#[test]
fn format_time_drops_the_seconds() {
    // 59 s is still inside minute 00 of the epoch day.
    assert_eq!(format_time(59), "1970-01-01 00:00");
}

#[test]
fn format_time_of_a_recent_timestamp_gives_the_utc_date_and_time() {
    // 1_700_000_000 is 2023-11-14 22:13:20 UTC: day 19_675 after the epoch is 14 November 2023,
    // and the 80_000 s left over in that day is 22 h 13 min 20 s.
    assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
}

#[test]
fn not_found_error_responds_with_status_404() {
    let response = WebError::NotFound.into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn bad_request_error_responds_with_status_400() {
    let response = WebError::BadRequest("x".into()).into_response();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn db_error_responds_with_status_500() {
    let response = WebError::Db(DbError::Corrupt("test".into())).into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn render_error_responds_with_status_500() {
    let response = WebError::Render(AskamaError::Fmt).into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn not_found_error_body_says_not_found() {
    let response = WebError::NotFound.into_response();

    assert_eq!(body_text(response).await, "not found");
}

#[tokio::test]
async fn bad_request_error_body_is_the_message() {
    let response = WebError::BadRequest("the url is not valid".into()).into_response();

    assert_eq!(body_text(response).await, "the url is not valid");
}

#[test]
fn db_error_converts_into_web_error_db() {
    let error = WebError::from(DbError::Corrupt("test".into()));

    assert!(matches!(error, WebError::Db(_)));
}

#[test]
fn askama_error_converts_into_web_error_render() {
    let error = WebError::from(AskamaError::Fmt);

    assert!(matches!(error, WebError::Render(_)));
}
