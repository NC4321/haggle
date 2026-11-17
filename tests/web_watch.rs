//! Tests for the watch detail page and its edit, pause and delete forms in `haggle::web`.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use haggle::db::{
    Event, EventKind, NewObservation, NewWatch, Watch, events, observations, open, watches,
};
use haggle::extract::{Availability, Source};
use haggle::fetch::Fetcher;
use haggle::money::{Currency, Money};
use haggle::web::{AppState, serve};
use rusqlite::Connection;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The product page every test watch points at.
const URL: &str = "https://shop.example/item/42";

/// The time, in Unix seconds, that test watches are created and first observed.
const NOW: i64 = 1_700_000_000;

/// One hour in seconds.
const HOUR_S: i64 = 3_600;

/// One day in seconds.
const DAY_S: i64 = 86_400;

/// A database path in the system temp directory, unique to one test. The file and SQLite's
/// sidecar files are removed when the value is created and again when it is dropped.
struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let path = env::temp_dir().join(format!("haggle-web-watch-{}-{name}.db", process::id()));
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

/// A `serve` task on 127.0.0.1 with the given database. Dropping it aborts the task.
struct Server {
    base: String,
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
        let task = tokio::spawn(serve(listener, state, std::future::pending::<()>()));
        Self {
            base: format!("http://{address}"),
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

/// An HTTP client that does not follow redirects, so a 303 can be inspected.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build the HTTP client")
}

/// Sends a GET request for `path` to the running server.
async fn get(server: &Server, path: &str) -> reqwest::Response {
    client()
        .get(server.url(path))
        .send()
        .await
        .expect("send the request")
}

/// Sends a POST request with an empty body to `path`.
async fn post(server: &Server, path: &str) -> reqwest::Response {
    client()
        .post(server.url(path))
        .send()
        .await
        .expect("send the request")
}

/// Sends a POST request with the URL-encoded `body` to `path`, as a browser form does.
async fn post_form(server: &Server, path: &str, body: &str) -> reqwest::Response {
    client()
        .post(server.url(path))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body.to_string())
        .send()
        .await
        .expect("send the request")
}

/// The body of the edit form for a watch, as the page's form would send it.
fn edit_body(target: &str, every: &str) -> String {
    format!("target={target}&every={every}")
}

/// The path of a watch's detail page.
fn watch_path(id: i64) -> String {
    format!("/watches/{id}")
}

/// The `Location` header of a redirect response.
fn location(response: &reqwest::Response) -> &str {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("the redirect has a Location header")
        .to_str()
        .expect("Location is ASCII")
}

/// The text of the first `<h1>` heading on a page, without the surrounding tags.
fn heading_text(page: &str) -> &str {
    let start = page.find("<h1").expect("the page has an h1 heading");
    let text_start = start + page[start..].find('>').expect("the h1 tag closes") + 1;
    let text_end = text_start
        + page[text_start..]
            .find("</h1>")
            .expect("the h1 heading closes");
    page[text_start..text_end].trim()
}

/// The whole `<input>` tag whose `name` attribute is `name`, so its other attributes can be read.
fn input_tag<'a>(page: &'a str, name: &str) -> &'a str {
    let field = page
        .find(&format!("name=\"{name}\""))
        .unwrap_or_else(|| panic!("the page has an input named {name}"));
    let start = page[..field].rfind('<').expect("the field is inside a tag");
    let end = field + page[field..].find('>').expect("the tag closes");
    &page[start..=end]
}

/// Opens the test database on a connection of the test's own, separate from the server's.
fn open_db(db: &TempDb) -> Connection {
    open(&db.path).expect("open the test database")
}

/// A watch for `URL` with the given name, target and interval.
fn new_watch(name: Option<&str>, target: Option<Money>, interval_s: i64) -> NewWatch {
    NewWatch {
        url: URL.to_string(),
        name: name.map(str::to_string),
        selector: None,
        target,
        interval_s,
    }
}

/// Inserts `new` at `NOW` and returns its id.
fn seed_watch(db: &TempDb, new: &NewWatch) -> i64 {
    let conn = open_db(db);
    watches::insert(&conn, new, NOW).expect("insert the watch")
}

/// Reads the watch with `id`, or `None` if it is not stored.
fn stored_watch(db: &TempDb, id: i64) -> Option<Watch> {
    let conn = open_db(db);
    watches::get(&conn, id).expect("read the watch")
}

/// Records an in-stock observation of `price` for watch `watch_id` at time `at`.
fn seed_observation(db: &TempDb, watch_id: i64, price: Money, at: i64) {
    let conn = open_db(db);
    let new = NewObservation {
        watch_id,
        at,
        price,
        list_price: None,
        availability: Availability::InStock,
        source: Source::JsonLd,
    };
    observations::insert(&conn, &new).expect("insert the observation");
}

/// Stores an event of `kind` for watch `watch_id`, recorded at time `at`.
fn seed_event(
    db: &TempDb,
    watch_id: i64,
    kind: EventKind,
    old: Option<Money>,
    new: Option<Money>,
    at: i64,
) {
    let conn = open_db(db);
    let event = Event {
        watch_id,
        kind,
        old,
        new,
    };
    events::insert(&conn, &event, at).expect("insert the event");
}

/// Sets the enabled flag of watch `id` directly in the database.
fn set_enabled(db: &TempDb, id: i64, enabled: bool) {
    let conn = open_db(db);
    watches::set_enabled(&conn, id, enabled).expect("set the enabled flag");
}

/// A price in minor units of `currency`.
fn money(minor: i64, currency: Currency) -> Money {
    Money { minor, currency }
}

#[tokio::test]
async fn get_of_an_unknown_watch_answers_404() {
    let db = TempDb::new("get-unknown");
    let server = Server::start(&db).await;

    let response = get(&server, "/watches/999").await;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_with_a_non_integer_id_answers_400() {
    let db = TempDb::new("get-bad-id");
    let server = Server::start(&db).await;

    let response = get(&server, "/watches/abc").await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn detail_page_of_a_watch_without_observations_answers_200() {
    let db = TempDb::new("detail-empty-status");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = get(&server, &watch_path(id)).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn detail_page_heading_is_the_watch_name() {
    let db = TempDb::new("detail-heading-name");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert_eq!(heading_text(&page), "Kettle");
}

#[tokio::test]
async fn detail_page_heading_is_the_url_when_the_watch_has_no_name() {
    let db = TempDb::new("detail-heading-url");
    let id = seed_watch(&db, &new_watch(None, None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert_eq!(heading_text(&page), URL);
}

#[tokio::test]
async fn detail_page_links_the_watch_url() {
    let db = TempDb::new("detail-url-link");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        page.contains(&format!("href=\"{URL}\"")),
        "the page links the watch URL"
    );
}

#[tokio::test]
async fn detail_page_escapes_the_watch_name() {
    let db = TempDb::new("detail-escape-name");
    let id = seed_watch(&db, &new_watch(Some("<i>Kettle</i>"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        !page.contains("<i>Kettle</i>"),
        "the name is escaped, not written as markup"
    );
}

#[tokio::test]
async fn detail_page_without_observations_says_no_data_yet() {
    let db = TempDb::new("detail-no-data");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(page.contains("no data yet"), "the page says no data yet");
}

#[tokio::test]
async fn detail_page_without_events_says_no_events_yet() {
    let db = TempDb::new("detail-no-events");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        page.contains("No events yet"),
        "the page says no events yet"
    );
}

#[tokio::test]
async fn detail_page_shows_the_latest_price_of_the_watch() {
    let db = TempDb::new("detail-latest-price");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_observation(&db, id, money(19_999, Currency::Eur), NOW);
    // 149.50 EUR is the newer of the two observations, so it is the latest price.
    seed_observation(&db, id, money(14_950, Currency::Eur), NOW + HOUR_S);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(page.contains("149.50"), "the page shows the latest price");
}

#[tokio::test]
async fn detail_page_draws_the_price_history_as_a_polyline() {
    let db = TempDb::new("detail-polyline");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_observation(&db, id, money(19_999, Currency::Eur), NOW);
    seed_observation(&db, id, money(14_950, Currency::Eur), NOW + HOUR_S);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(page.contains("<polyline"), "the page draws the price chart");
}

#[tokio::test]
async fn detail_page_with_a_target_draws_the_target_line() {
    let db = TempDb::new("detail-target-line");
    let target = Some(money(15_000, Currency::Eur));
    let id = seed_watch(&db, &new_watch(Some("Kettle"), target, HOUR_S));
    seed_observation(&db, id, money(19_999, Currency::Eur), NOW);
    seed_observation(&db, id, money(14_950, Currency::Eur), NOW + HOUR_S);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        page.contains("class=\"target\""),
        "the chart draws the target line"
    );
}

#[tokio::test]
async fn detail_page_without_a_target_draws_no_target_line() {
    let db = TempDb::new("detail-no-target-line");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_observation(&db, id, money(19_999, Currency::Eur), NOW);
    seed_observation(&db, id, money(14_950, Currency::Eur), NOW + HOUR_S);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        !page.contains("class=\"target\""),
        "the chart draws no target line without a target"
    );
}

#[tokio::test]
async fn detail_page_lists_the_labels_of_its_events() {
    let db = TempDb::new("detail-event-labels");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_event(
        &db,
        id,
        EventKind::PriceDrop,
        Some(money(19_999, Currency::Eur)),
        Some(money(14_950, Currency::Eur)),
        NOW,
    );
    seed_event(&db, id, EventKind::BackInStock, None, None, NOW + DAY_S);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(page.contains("Price drop"), "the page lists a price drop");
    assert!(page.contains("Back in stock"), "the page lists a restock");
}

#[tokio::test]
async fn detail_page_lists_the_newest_event_first() {
    let db = TempDb::new("detail-event-order");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_event(
        &db,
        id,
        EventKind::PriceDrop,
        Some(money(19_999, Currency::Eur)),
        Some(money(14_950, Currency::Eur)),
        NOW,
    );
    seed_event(&db, id, EventKind::BackInStock, None, None, NOW + DAY_S);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    let newer = page
        .find("Back in stock")
        .expect("the page lists the restock");
    let older = page
        .find("Price drop")
        .expect("the page lists the price drop");
    assert!(
        newer < older,
        "the restock, the newer event, is listed first"
    );
}

#[tokio::test]
async fn detail_page_shows_the_time_of_each_event() {
    let db = TempDb::new("detail-event-time");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_event(&db, id, EventKind::BackInStock, None, None, NOW);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    // 1_700_000_000 is 2023-11-14 22:13 UTC, the time format_time gives for it.
    assert!(
        page.contains("2023-11-14 22:13"),
        "the event shows its time"
    );
}

#[tokio::test]
async fn detail_page_shows_the_old_and_new_prices_of_an_event() {
    let db = TempDb::new("detail-event-prices");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_event(
        &db,
        id,
        EventKind::PriceDrop,
        Some(money(19_999, Currency::Eur)),
        Some(money(14_950, Currency::Eur)),
        NOW,
    );
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(page.contains("199.99"), "the event shows the old price");
    assert!(page.contains("149.50"), "the event shows the new price");
}

#[tokio::test]
async fn detail_page_offers_the_edit_form_to_post_to_the_watch() {
    let db = TempDb::new("detail-edit-action");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        page.contains(&format!("action=\"/watches/{id}\"")),
        "the edit form posts to the watch"
    );
}

#[tokio::test]
async fn edit_form_prefills_the_target_with_its_amount() {
    let db = TempDb::new("edit-prefill-target");
    let target = Some(money(15_000, Currency::Eur));
    let id = seed_watch(&db, &new_watch(Some("Kettle"), target, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    let tag = input_tag(&page, "target");
    assert!(
        tag.contains("value=\"150.00\""),
        "the target field shows 150.00: {tag}"
    );
}

#[tokio::test]
async fn edit_form_target_is_empty_when_the_watch_has_no_target() {
    let db = TempDb::new("edit-prefill-no-target");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    let tag = input_tag(&page, "target");
    assert!(
        !tag.contains("value=\"") || tag.contains("value=\"\""),
        "the target field is empty: {tag}"
    );
}

#[tokio::test]
async fn edit_form_prefills_every_in_hours() {
    let db = TempDb::new("edit-every-hours");
    // 21_600 s is six hours, and it is not a whole number of days.
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, 6 * HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    let tag = input_tag(&page, "every");
    assert!(
        tag.contains("value=\"6h\""),
        "the interval field shows 6h: {tag}"
    );
}

#[tokio::test]
async fn edit_form_prefills_every_in_days_for_whole_days() {
    let db = TempDb::new("edit-every-days");
    // 172_800 s is two whole days, which is shown in days rather than hours.
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, 2 * DAY_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    let tag = input_tag(&page, "every");
    assert!(
        tag.contains("value=\"2d\""),
        "the interval field shows 2d: {tag}"
    );
}

#[tokio::test]
async fn edit_form_prefills_every_in_minutes() {
    let db = TempDb::new("edit-every-minutes");
    // 900 s is fifteen minutes, which is not a whole number of hours.
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, 900));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    let tag = input_tag(&page, "every");
    assert!(
        tag.contains("value=\"15m\""),
        "the interval field shows 15m: {tag}"
    );
}

#[tokio::test]
async fn edit_form_prefills_every_in_seconds_when_not_a_whole_minute() {
    let db = TempDb::new("edit-every-seconds");
    // 3_601 s is an hour and a second, which is not a whole number of minutes.
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, 3_601));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    let tag = input_tag(&page, "every");
    assert!(
        tag.contains("value=\"3601s\""),
        "the interval field shows 3601s: {tag}"
    );
}

#[tokio::test]
async fn detail_page_of_an_enabled_watch_offers_pause() {
    let db = TempDb::new("detail-pause-button");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        page.contains(&format!("/watches/{id}/pause")),
        "the pause button posts to the pause path"
    );
    assert!(page.contains("Pause"), "an enabled watch offers Pause");
    assert!(
        !page.contains("Resume"),
        "an enabled watch does not offer Resume"
    );
}

#[tokio::test]
async fn detail_page_of_a_paused_watch_offers_resume() {
    let db = TempDb::new("detail-resume-button");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    set_enabled(&db, id, false);
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(page.contains("Resume"), "a paused watch offers Resume");
}

#[tokio::test]
async fn detail_page_offers_delete_posting_to_the_delete_path() {
    let db = TempDb::new("detail-delete-button");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let page = get(&server, &watch_path(id))
        .await
        .text()
        .await
        .expect("read the page");

    assert!(
        page.contains(&format!("/watches/{id}/delete")),
        "the delete button posts to the delete path"
    );
}

#[tokio::test]
async fn post_edit_redirects_to_the_watch_page() {
    let db = TempDb::new("edit-redirect");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "2h")).await;

    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&response), watch_path(id));
}

#[tokio::test]
async fn post_edit_with_a_target_stores_it_in_minor_units() {
    let db = TempDb::new("edit-target-minor");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    post_form(&server, &watch_path(id), &edit_body("150.00", "2h")).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    // 150.00 is 15_000 minor units; with no target or observation to take a currency from, it is EUR.
    assert_eq!(watch.target, Some(money(15_000, Currency::Eur)));
}

#[tokio::test]
async fn post_edit_with_an_every_stores_the_interval_in_seconds() {
    let db = TempDb::new("edit-interval-seconds");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    post_form(&server, &watch_path(id), &edit_body("150.00", "2h")).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    // Two hours is 2 * 3_600 = 7_200 seconds.
    assert_eq!(watch.interval_s, 7_200);
}

#[tokio::test]
async fn post_edit_with_an_empty_target_clears_the_target() {
    let db = TempDb::new("edit-clear-target");
    let id = seed_watch(
        &db,
        &new_watch(Some("Kettle"), Some(money(15_000, Currency::Eur)), HOUR_S),
    );
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("", "2h")).await;

    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    let watch = stored_watch(&db, id).expect("the watch is still stored");
    assert_eq!(watch.target, None);
}

#[tokio::test]
async fn post_edit_target_takes_the_currency_of_the_watch_target() {
    let db = TempDb::new("edit-target-currency-target");
    let id = seed_watch(
        &db,
        &new_watch(Some("Kettle"), Some(money(10_000, Currency::Gbp)), HOUR_S),
    );
    let server = Server::start(&db).await;

    post_form(&server, &watch_path(id), &edit_body("150.00", "2h")).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    assert_eq!(watch.target, Some(money(15_000, Currency::Gbp)));
}

#[tokio::test]
async fn post_edit_target_takes_the_currency_of_the_latest_observation_without_a_target() {
    let db = TempDb::new("edit-target-currency-observation");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    seed_observation(&db, id, money(9_900, Currency::Usd), NOW);
    let server = Server::start(&db).await;

    post_form(&server, &watch_path(id), &edit_body("150.00", "2h")).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    assert_eq!(watch.target, Some(money(15_000, Currency::Usd)));
}

#[tokio::test]
async fn post_edit_with_an_invalid_target_answers_400() {
    let db = TempDb::new("edit-bad-target");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("abc", "2h")).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_edit_with_an_invalid_target_keeps_the_stored_target() {
    let db = TempDb::new("edit-bad-target-kept");
    let id = seed_watch(
        &db,
        &new_watch(Some("Kettle"), Some(money(15_000, Currency::Eur)), HOUR_S),
    );
    let server = Server::start(&db).await;

    post_form(&server, &watch_path(id), &edit_body("abc", "2h")).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    assert_eq!(watch.target, Some(money(15_000, Currency::Eur)));
}

#[tokio::test]
async fn post_edit_with_one_minute_every_answers_400() {
    let db = TempDb::new("edit-every-1m");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "1m")).await;

    // 1 minute is 60 s, below the 300 s minimum.
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_edit_with_four_minutes_every_answers_400() {
    let db = TempDb::new("edit-every-4m");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "4m")).await;

    // 4 minutes is 240 s, below the 300 s minimum.
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_edit_with_five_minutes_every_is_accepted_at_the_minimum() {
    let db = TempDb::new("edit-every-5m");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "5m")).await;

    // 5 minutes is exactly 300 s, the minimum, so the edit is accepted.
    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    let watch = stored_watch(&db, id).expect("the watch is still stored");
    assert_eq!(watch.interval_s, 300);
}

#[tokio::test]
async fn post_edit_with_seconds_every_below_the_minimum_answers_400() {
    let db = TempDb::new("edit-every-299s");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "299s")).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_edit_with_one_day_every_stores_86400_seconds() {
    let db = TempDb::new("edit-every-1d");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    post_form(&server, &watch_path(id), &edit_body("150.00", "1d")).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    // One day is 86_400 seconds.
    assert_eq!(watch.interval_s, DAY_S);
}

#[tokio::test]
async fn post_edit_with_a_unit_other_than_smhd_answers_400() {
    let db = TempDb::new("edit-every-bad-unit");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "2w")).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_edit_with_a_non_numeric_every_answers_400() {
    let db = TempDb::new("edit-every-not-number");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "abc")).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_edit_with_an_empty_every_answers_400() {
    let db = TempDb::new("edit-every-empty");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post_form(&server, &watch_path(id), &edit_body("150.00", "")).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_edit_with_a_non_integer_id_answers_400() {
    let db = TempDb::new("edit-bad-id");
    let server = Server::start(&db).await;

    let response = post_form(&server, "/watches/abc", &edit_body("150.00", "2h")).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_pause_redirects_to_the_watch_page() {
    let db = TempDb::new("pause-redirect");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post(&server, &format!("{}/pause", watch_path(id))).await;

    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&response), watch_path(id));
}

#[tokio::test]
async fn post_pause_disables_an_enabled_watch() {
    let db = TempDb::new("pause-disables");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    post(&server, &format!("{}/pause", watch_path(id))).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    assert!(!watch.enabled, "the watch is paused");
}

#[tokio::test]
async fn post_pause_twice_enables_the_watch_again() {
    let db = TempDb::new("pause-twice");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;
    let path = format!("{}/pause", watch_path(id));

    post(&server, &path).await;
    post(&server, &path).await;

    let watch = stored_watch(&db, id).expect("the watch is still stored");
    assert!(watch.enabled, "the second pause resumes the watch");
}

#[tokio::test]
async fn post_pause_with_a_non_integer_id_answers_400() {
    let db = TempDb::new("pause-bad-id");
    let server = Server::start(&db).await;

    let response = post(&server, "/watches/abc/pause").await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_delete_redirects_to_the_home_page() {
    let db = TempDb::new("delete-redirect");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    let response = post(&server, &format!("{}/delete", watch_path(id))).await;

    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/");
}

#[tokio::test]
async fn post_delete_removes_the_watch() {
    let db = TempDb::new("delete-removes");
    let id = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    let server = Server::start(&db).await;

    post(&server, &format!("{}/delete", watch_path(id))).await;

    assert!(stored_watch(&db, id).is_none(), "the watch is gone");
}

#[tokio::test]
async fn post_delete_leaves_other_watches_in_place() {
    let db = TempDb::new("delete-leaves-others");
    let deleted = seed_watch(&db, &new_watch(Some("Kettle"), None, HOUR_S));
    // A second watch needs its own URL, since a URL may be watched only once.
    let other = NewWatch {
        url: "https://shop.example/item/43".to_string(),
        ..new_watch(Some("Toaster"), None, HOUR_S)
    };
    let kept = seed_watch(&db, &other);
    let server = Server::start(&db).await;

    post(&server, &format!("{}/delete", watch_path(deleted))).await;

    assert!(stored_watch(&db, kept).is_some(), "the other watch stays");
}

#[tokio::test]
async fn post_delete_of_an_unknown_watch_answers_404() {
    let db = TempDb::new("delete-unknown");
    let server = Server::start(&db).await;

    let response = post(&server, "/watches/999/delete").await;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_delete_with_a_non_integer_id_answers_400() {
    let db = TempDb::new("delete-bad-id");
    let server = Server::start(&db).await;

    let response = post(&server, "/watches/abc/delete").await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}
