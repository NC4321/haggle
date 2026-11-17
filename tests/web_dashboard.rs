//! Tests for the dashboard served on `GET /` by `haggle::web::pages`.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use haggle::db::{NewObservation, NewWatch, observations, open, watches};
use haggle::extract::{Availability, Source};
use haggle::fetch::Fetcher;
use haggle::money::{Currency, Money};
use haggle::web::{AppState, serve};
use rusqlite::Connection;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// The insert time, in Unix seconds, of every watch the tests create.
const NOW: i64 = 1_700_000_000;
/// One hour in seconds: the watch interval and the gap between the observations the tests record.
const HOUR_S: i64 = 3600;
/// The URL of the watch the tests name "Kettle".
const KETTLE_URL: &str = "https://shop.example/kettle";
/// The URL of a watch that has no name, used to check the URL is shown in its place.
const BARE_URL: &str = "https://shop.example/bare-toaster";

/// A database path in the system temp directory, unique to one test. The file and SQLite's
/// sidecar files are removed when the value is created and again when it is dropped.
struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let path =
            env::temp_dir().join(format!("haggle-web-dashboard-{}-{name}.db", process::id()));
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

/// A `serve` task on 127.0.0.1 over the database in `db`. Dropping it aborts the task.
struct Server {
    base: String,
    /// Kept alive for the server's lifetime: serve stops once the sender is dropped.
    _shutdown: oneshot::Sender<()>,
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
            _shutdown: shutdown,
            task,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
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

/// Fetches `GET /` and returns the body as text.
async fn dashboard(server: &Server) -> String {
    get(server, "/")
        .await
        .text()
        .await
        .expect("read the dashboard body")
}

/// Opens the test database at `db`, runs `build` on it and closes it again, so that a server
/// started afterwards opens the same data. Returns what `build` returns.
fn seed<T>(db: &TempDb, build: impl FnOnce(&Connection) -> T) -> T {
    let conn = open(&db.path).expect("open the test database");
    build(&conn)
}

/// A watch with the given URL and optional name, a one-hour interval and no selector or target.
fn new_watch(url: &str, name: Option<&str>) -> NewWatch {
    NewWatch {
        url: url.to_string(),
        name: name.map(str::to_string),
        selector: None,
        target: None,
        interval_s: HOUR_S,
    }
}

/// Records an in-stock EUR observation of `minor` minor units for `watch_id` taken at `at`.
fn record(conn: &Connection, watch_id: i64, at: i64, minor: i64) {
    let new = NewObservation {
        watch_id,
        at,
        price: Money {
            minor,
            currency: Currency::Eur,
        },
        list_price: None,
        availability: Availability::InStock,
        source: Source::JsonLd,
    };
    observations::insert(conn, &new).expect("insert observation");
}

/// Inserts a watch and records one observation per entry of `prices`, in order, one hour apart.
/// Returns the watch id.
fn seed_watch(conn: &Connection, url: &str, name: Option<&str>, prices: &[i64]) -> i64 {
    let id = watches::insert(conn, &new_watch(url, name), NOW).expect("insert watch");
    for (index, minor) in prices.iter().enumerate() {
        record(conn, id, NOW + HOUR_S * index as i64, *minor);
    }
    id
}

/// The table row of the dashboard that contains `needle`, from its `<tr` to its `</tr>`.
/// Panics if no row contains it.
fn row_for<'a>(body: &'a str, needle: &str) -> &'a str {
    let at = body
        .find(needle)
        .unwrap_or_else(|| panic!("no row of the dashboard shows {needle:?}"));
    let start = body[..at]
        .rfind("<tr")
        .expect("the text sits inside a table row");
    let end = at + body[at..].find("</tr>").expect("the table row is closed");
    &body[start..end]
}

#[tokio::test]
async fn dashboard_answers_200_ok_when_there_are_no_watches() {
    let db = TempDb::new("empty-status");
    let server = Server::start(&db).await;

    let response = get(&server, "/").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn dashboard_content_type_is_html() {
    let db = TempDb::new("empty-type");
    let server = Server::start(&db).await;

    let response = get(&server, "/").await;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .expect("the dashboard has a content-type header")
        .to_str()
        .expect("content-type is ASCII");
    assert!(
        content_type.starts_with("text/html"),
        "content-type: {content_type}"
    );
}

#[tokio::test]
async fn empty_dashboard_says_no_watches_yet() {
    let db = TempDb::new("empty-message");
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(body.contains("No watches yet"), "body: {body}");
}

#[tokio::test]
async fn empty_dashboard_links_to_the_add_page() {
    let db = TempDb::new("empty-add-link");
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(body.contains("href=\"/add\""), "body: {body}");
}

#[tokio::test]
async fn empty_dashboard_has_no_watches_table() {
    let db = TempDb::new("empty-no-table");
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(!body.contains("<table"), "body: {body}");
}

#[tokio::test]
async fn dashboard_heading_is_watches() {
    let db = TempDb::new("heading");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(body.contains("<h1>Watches</h1>"), "body: {body}");
}

#[tokio::test]
async fn dashboard_with_watches_has_the_watches_table() {
    let db = TempDb::new("table");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(body.contains("<table class=\"watches\">"), "body: {body}");
}

#[tokio::test]
async fn watch_row_shows_the_watch_name() {
    let db = TempDb::new("row-name");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(row_for(&body, "Kettle").contains("Kettle"));
}

#[tokio::test]
async fn watch_row_shows_the_latest_price() {
    let db = TempDb::new("row-latest");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    // The latest observation is 39.99 EUR (3999 minor units), recorded after the 49.99 one.
    assert!(row_for(&body, "Kettle").contains("39.99 EUR"));
}

#[tokio::test]
async fn price_drop_shows_a_down_arrow_and_the_absolute_change() {
    let db = TempDb::new("drop-text");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    // 49.99 down to 39.99 is a drop of 10.00 EUR (1000 minor units). The arrow is U+25BC.
    assert!(
        row_for(&body, "Kettle").contains("▼ 10.00 EUR"),
        "body: {body}"
    );
}

#[tokio::test]
async fn price_drop_change_has_class_down() {
    let db = TempDb::new("drop-class");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    let row = row_for(&body, "Kettle");
    assert!(row.contains("class=\"down\""), "row: {row}");
    assert!(!row.contains("class=\"up\""), "row: {row}");
}

#[tokio::test]
async fn price_rise_shows_an_up_arrow_and_the_absolute_change() {
    let db = TempDb::new("rise-text");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[1000, 1200]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    // 10.00 up to 12.00 is a rise of 2.00 EUR (200 minor units). The arrow is U+25B2.
    assert!(
        row_for(&body, "Kettle").contains("▲ 2.00 EUR"),
        "body: {body}"
    );
}

#[tokio::test]
async fn price_rise_change_has_class_up() {
    let db = TempDb::new("rise-class");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[1000, 1200]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    let row = row_for(&body, "Kettle");
    assert!(row.contains("class=\"up\""), "row: {row}");
    assert!(!row.contains("class=\"down\""), "row: {row}");
}

#[tokio::test]
async fn equal_prices_show_no_change_arrow() {
    let db = TempDb::new("equal");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[3999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    let row = row_for(&body, "Kettle");
    assert!(!row.contains("▼"), "row: {row}");
    assert!(!row.contains("▲"), "row: {row}");
}

#[tokio::test]
async fn single_observation_shows_no_change_arrow() {
    let db = TempDb::new("single");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    let row = row_for(&body, "Kettle");
    assert!(row.contains("39.99 EUR"), "row: {row}");
    assert!(!row.contains("▼"), "row: {row}");
    assert!(!row.contains("▲"), "row: {row}");
}

#[tokio::test]
async fn latest_price_is_the_newest_observation_not_the_last_inserted() {
    let db = TempDb::new("newest");
    seed(&db, |conn| {
        let id = watches::insert(conn, &new_watch(KETTLE_URL, Some("Kettle")), NOW)
            .expect("insert watch");
        // The newer price goes in first: the latest is 39.99 EUR, the one before it 49.99 EUR.
        record(conn, id, NOW + HOUR_S, 3999);
        record(conn, id, NOW, 4999);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    // Dropping from 49.99 to 39.99 is a drop of 10.00 EUR; a rise would show ▲ instead.
    let row = row_for(&body, "Kettle");
    assert!(row.contains("39.99 EUR"), "row: {row}");
    assert!(row.contains("▼ 10.00 EUR"), "row: {row}");
}

#[tokio::test]
async fn watch_name_links_to_its_watch_page() {
    let db = TempDb::new("link");
    let id = seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999])
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    let expected = format!("href=\"/watches/{id}\"");
    assert!(row_for(&body, "Kettle").contains(&expected), "body: {body}");
}

#[tokio::test]
async fn watch_row_has_an_svg_sparkline() {
    let db = TempDb::new("sparkline");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(row_for(&body, "Kettle").contains("<svg"), "body: {body}");
}

#[tokio::test]
async fn sparkline_is_inserted_unescaped() {
    let db = TempDb::new("sparkline-unescaped");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    // The chart is our own SVG, so its markup must reach the page as markup, not escaped text.
    assert!(!body.contains("&lt;svg"), "body: {body}");
}

#[tokio::test]
async fn enabled_watch_row_shows_status_ok() {
    let db = TempDb::new("status-ok");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(
        row_for(&body, "Kettle").contains("<span class=\"status-ok\">ok</span>"),
        "body: {body}"
    );
}

#[tokio::test]
async fn unnamed_watch_row_shows_its_url_in_place_of_the_name() {
    let db = TempDb::new("unnamed-url");
    seed(&db, |conn| seed_watch(conn, BARE_URL, None, &[]));
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(row_for(&body, BARE_URL).contains(BARE_URL), "body: {body}");
}

#[tokio::test]
async fn watch_without_observations_shows_a_dash_for_the_price() {
    let db = TempDb::new("no-observations");
    seed(&db, |conn| seed_watch(conn, BARE_URL, None, &[]));
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    // The dash is an em dash, U+2014.
    assert!(row_for(&body, BARE_URL).contains("—"), "body: {body}");
}

#[tokio::test]
async fn watch_without_observations_shows_no_change_arrow() {
    let db = TempDb::new("no-observations-change");
    seed(&db, |conn| seed_watch(conn, BARE_URL, None, &[]));
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    let row = row_for(&body, BARE_URL);
    assert!(!row.contains("▼"), "row: {row}");
    assert!(!row.contains("▲"), "row: {row}");
}

#[tokio::test]
async fn disabled_watch_row_shows_paused() {
    let db = TempDb::new("paused");
    seed(&db, |conn| {
        let id = seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
        watches::set_enabled(conn, id, false).expect("disable the watch");
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(
        row_for(&body, "Kettle").contains("<span class=\"paused\">paused</span>"),
        "body: {body}"
    );
}

#[tokio::test]
async fn enabled_watch_row_shows_no_paused_marker() {
    let db = TempDb::new("not-paused");
    seed(&db, |conn| {
        seed_watch(conn, KETTLE_URL, Some("Kettle"), &[4999, 3999]);
    });
    let server = Server::start(&db).await;

    let body = dashboard(&server).await;

    assert!(
        !row_for(&body, "Kettle").contains("class=\"paused\""),
        "body: {body}"
    );
}
