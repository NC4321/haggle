use std::env;
use std::fs;
use std::future::pending;
use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::get;
use haggle::db::{Watch, open, watches};
use haggle::fetch::{Fetcher, Politeness};
use haggle::money::{Currency, Money};
use haggle::web::{AppState, serve};
use rusqlite::Connection;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// How long a test waits for the product page to be requested.
const DEADLINE: Duration = Duration::from_secs(5);

/// A shop URL for the add tests that do not need the page to be fetched.
const SHOP_URL: &str = "https://shop.example/kettle";

/// A product page with a JSON-LD Product and one Offer: price 49.99 EUR, in stock.
const PRODUCT_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Test kettle</title>
  <script type="application/ld+json">
  {
    "@context": "https://schema.org/",
    "@type": "Product",
    "name": "Test kettle",
    "offers": {
      "@type": "Offer",
      "price": "49.99",
      "priceCurrency": "EUR",
      "availability": "https://schema.org/InStock"
    }
  }
  </script>
</head>
<body><h1>Test kettle</h1></body>
</html>"#;

/// A page with no price in any form the extractors read.
const NO_PRICE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head><title>Kettle</title></head>
<body><h1>Test kettle</h1><p>Sold in pairs.</p></body>
</html>"#;

/// The robots.txt that lets the fetcher read every path on the product server.
const ROBOTS_TXT: &str = "User-agent: *\nAllow: /\n";

/// A database path in the system temp directory, unique to one test. The file and SQLite's
/// sidecar files are removed when the value is created and again when it is dropped.
struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let path = env::temp_dir().join(format!("haggle-web-add-{}-{name}.db", process::id()));
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

/// The app on 127.0.0.1 with a fresh database. The connection is shared with the test so it
/// can read what the handlers wrote. Dropping it aborts the task.
struct Server {
    base: String,
    conn: Arc<Mutex<Connection>>,
    task: JoinHandle<io::Result<()>>,
}

impl Server {
    async fn start(db: &TempDb) -> Self {
        let conn = Arc::new(Mutex::new(open(&db.path).expect("open the test database")));
        // The politeness is zeroed so the preview's fetches of the product server are not paced.
        let fetcher = Fetcher::new(Duration::from_secs(5), 1_000_000)
            .expect("build the fetcher")
            .with_politeness(Politeness {
                host_interval: Duration::ZERO,
                ..Default::default()
            });
        let state = AppState {
            db: Arc::clone(&conn),
            fetcher: Arc::new(fetcher),
        };
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let address = listener.local_addr().expect("read the bound address");
        let task = tokio::spawn(serve(listener, state, pending::<()>()));
        Self {
            base: format!("http://{address}"),
            conn,
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

/// Lets a test hold the `/held` page open until it has checked the database lock.
#[derive(Default)]
struct Gate {
    /// Told when `/held` is requested.
    requested: Mutex<Option<oneshot::Sender<()>>>,
    /// Awaited by `/held` before it answers.
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

/// Answers the product page once the gate releases it, or at once when the gate has no channels.
async fn held(State(gate): State<Arc<Gate>>) -> Html<&'static str> {
    let requested = gate.requested.lock().expect("lock the gate").take();
    if let Some(sender) = requested {
        sender.send(()).ok();
    }
    let release = gate.release.lock().expect("lock the gate").take();
    if let Some(receiver) = release {
        receiver.await.ok();
    }
    Html(PRODUCT_HTML)
}

/// A product site on 127.0.0.1 that the preview fetches. Dropping it aborts the task.
struct PageServer {
    base: String,
    task: JoinHandle<io::Result<()>>,
}

impl PageServer {
    async fn start() -> Self {
        Self::with_gate(Arc::new(Gate::default())).await
    }

    async fn with_gate(gate: Arc<Gate>) -> Self {
        let app = Router::new()
            .route("/robots.txt", get(|| async { ROBOTS_TXT }))
            .route("/kettle", get(|| async { Html(PRODUCT_HTML) }))
            .route("/no-price", get(|| async { Html(NO_PRICE_HTML) }))
            .route(
                "/missing",
                get(|| async { (StatusCode::NOT_FOUND, "no such page") }),
            )
            .route("/held", get(held))
            .with_state(gate);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let address = listener.local_addr().expect("read the bound address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await });
        Self {
            base: format!("http://{address}"),
            task,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

impl Drop for PageServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A client that does not follow redirects, so a 303 from the add form is seen as it is.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build the HTTP client")
}

/// Sends a GET request for `path` to the running server.
async fn get_page(server: &Server, path: &str) -> reqwest::Response {
    client()
        .get(server.url(path))
        .send()
        .await
        .expect("send the request")
}

/// Posts `fields` to `path` as an HTML form would.
async fn post_form(server: &Server, path: &str, fields: &[(&str, &str)]) -> reqwest::Response {
    let mut body = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in fields {
        body.append_pair(name, value);
    }
    client()
        .post(server.url(path))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body.finish())
        .send()
        .await
        .expect("send the request")
}

/// The fields a browser sends for the add form: every field present, empty except `url` and
/// `every`, which hold their form defaults. `overrides` replaces the fields it names.
fn with_form_fields<'a>(url: &'a str, overrides: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut fields = vec![
        ("url", url),
        ("name", ""),
        ("selector", ""),
        ("target", ""),
        ("every", "6h"),
    ];
    for &(name, value) in overrides {
        fields.retain(|&(existing, _)| existing != name);
        fields.push((name, value));
    }
    fields
}

/// Reads a response body as UTF-8 text.
async fn text(response: reqwest::Response) -> String {
    response.text().await.expect("read the response body")
}

/// Parses an HTML body so tests can read attributes of the form.
fn parse(body: &str) -> scraper::Html {
    scraper::Html::parse_document(body)
}

/// Returns the value of `attribute` on the first element matching `css`, if there is one.
fn attribute(document: &scraper::Html, css: &str, name: &str) -> Option<String> {
    let selector = scraper::Selector::parse(css).expect("valid CSS selector");
    document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr(name).map(str::to_string))
}

/// Whether any element matches `css`.
fn has_element(document: &scraper::Html, css: &str) -> bool {
    let selector = scraper::Selector::parse(css).expect("valid CSS selector");
    document.select(&selector).next().is_some()
}

/// Whether some button on the page has exactly `label` as its text.
fn has_button_labelled(document: &scraper::Html, label: &str) -> bool {
    let selector = scraper::Selector::parse("button").expect("valid CSS selector");
    document
        .select(&selector)
        .any(|button| button.text().collect::<String>().trim() == label)
}

/// Reads the watch with `id`, failing the test if it is missing.
fn watch(server: &Server, id: i64) -> Watch {
    let conn = server.conn.lock().expect("lock the database");
    watches::get(&conn, id)
        .expect("read the watch")
        .expect("the watch exists")
}

/// How many watches the database holds.
fn watch_count(server: &Server) -> usize {
    let conn = server.conn.lock().expect("lock the database");
    watches::list(&conn).expect("list the watches").len()
}

/// Fetches the add form and parses it.
async fn form(server: &Server) -> scraper::Html {
    parse(&text(get_page(server, "/add").await).await)
}

#[tokio::test]
async fn get_add_answers_200_ok() {
    let db = TempDb::new("get-status");
    let server = Server::start(&db).await;

    let response = get_page(&server, "/add").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn get_add_form_posts_to_add() {
    let db = TempDb::new("get-form-action");
    let server = Server::start(&db).await;

    let document = form(&server).await;

    assert!(
        has_element(&document, r#"form[action="/add"][method="post"]"#),
        "no form posts to /add"
    );
}

#[tokio::test]
async fn get_add_form_has_a_url_input() {
    let db = TempDb::new("get-url-input");
    let server = Server::start(&db).await;

    let document = form(&server).await;

    assert!(has_element(&document, r#"input[name="url"]"#));
}

#[tokio::test]
async fn get_add_form_has_name_selector_and_target_inputs() {
    let db = TempDb::new("get-optional-inputs");
    let server = Server::start(&db).await;

    let document = form(&server).await;

    for field in ["name", "selector", "target"] {
        assert!(
            has_element(&document, &format!(r#"input[name="{field}"]"#)),
            "no input named {field}"
        );
    }
}

#[tokio::test]
async fn get_add_form_defaults_every_to_6h() {
    let db = TempDb::new("get-every-default");
    let server = Server::start(&db).await;

    let document = form(&server).await;

    assert_eq!(
        attribute(&document, r#"input[name="every"]"#, "value").as_deref(),
        Some("6h")
    );
}

#[tokio::test]
async fn get_add_form_has_a_preview_button_posting_to_add_preview() {
    let db = TempDb::new("get-preview-button");
    let server = Server::start(&db).await;

    let document = form(&server).await;

    assert!(has_button_labelled(&document, "Preview"));
    assert!(has_element(
        &document,
        r#"button[formaction="/add/preview"]"#
    ));
}

#[tokio::test]
async fn get_add_form_has_an_add_button() {
    let db = TempDb::new("get-add-button");
    let server = Server::start(&db).await;

    let document = form(&server).await;

    assert!(has_button_labelled(&document, "Add"));
}

#[tokio::test]
async fn get_add_page_links_the_shared_stylesheet() {
    let db = TempDb::new("get-layout");
    let server = Server::start(&db).await;

    let body = text(get_page(&server, "/add").await).await;

    assert!(
        body.contains(r#"href="/static/style.css""#),
        "the add page does not use the shared layout"
    );
}

#[tokio::test]
async fn preview_of_a_product_page_answers_200_ok() {
    let db = TempDb::new("preview-status");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let response = post_form(
        &server,
        "/add/preview",
        &with_form_fields(&page.url("/kettle"), &[]),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn preview_shows_the_price_with_its_currency() {
    let db = TempDb::new("preview-price");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let body = text(
        post_form(
            &server,
            "/add/preview",
            &with_form_fields(&page.url("/kettle"), &[]),
        )
        .await,
    )
    .await;

    // The price 49.99 EUR is 4999 minor units; Money's Display writes it as "49.99 EUR".
    assert!(body.contains("49.99 EUR"), "body: {body}");
}

#[tokio::test]
async fn preview_shows_the_availability_as_in_stock() {
    let db = TempDb::new("preview-availability");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let body = text(
        post_form(
            &server,
            "/add/preview",
            &with_form_fields(&page.url("/kettle"), &[]),
        )
        .await,
    )
    .await;

    // The offer's availability is https://schema.org/InStock, which is Availability::InStock.
    assert!(body.contains("in stock"), "body: {body}");
}

#[tokio::test]
async fn preview_shows_the_product_title() {
    let db = TempDb::new("preview-title");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let body = text(
        post_form(
            &server,
            "/add/preview",
            &with_form_fields(&page.url("/kettle"), &[]),
        )
        .await,
    )
    .await;

    assert!(body.contains("Test kettle"), "body: {body}");
}

#[tokio::test]
async fn preview_shows_the_source_of_the_price() {
    let db = TempDb::new("preview-source");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let body = text(
        post_form(
            &server,
            "/add/preview",
            &with_form_fields(&page.url("/kettle"), &[]),
        )
        .await,
    )
    .await;

    // The price comes from the JSON-LD block, and Source::JsonLd displays as "json-ld".
    assert!(body.contains("json-ld"), "body: {body}");
}

#[tokio::test]
async fn preview_prefills_the_url_with_the_page_url() {
    let db = TempDb::new("preview-prefill-url");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;
    let page_url = page.url("/kettle");

    let response = post_form(&server, "/add/preview", &with_form_fields(&page_url, &[])).await;

    let document = parse(&text(response).await);
    assert_eq!(
        attribute(&document, r#"input[name="url"]"#, "value"),
        Some(page_url)
    );
}

#[tokio::test]
async fn preview_keeps_the_fields_the_user_entered() {
    let db = TempDb::new("preview-keeps-fields");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let response = post_form(
        &server,
        "/add/preview",
        &with_form_fields(
            &page.url("/kettle"),
            &[
                ("name", "Kettle"),
                ("selector", ".price-now"),
                ("target", "40.00"),
                ("every", "12h"),
            ],
        ),
    )
    .await;

    let document = parse(&text(response).await);
    assert_eq!(
        attribute(&document, r#"input[name="name"]"#, "value").as_deref(),
        Some("Kettle")
    );
    assert_eq!(
        attribute(&document, r#"input[name="selector"]"#, "value").as_deref(),
        Some(".price-now")
    );
    assert_eq!(
        attribute(&document, r#"input[name="target"]"#, "value").as_deref(),
        Some("40.00")
    );
    assert_eq!(
        attribute(&document, r#"input[name="every"]"#, "value").as_deref(),
        Some("12h")
    );
}

#[tokio::test]
async fn preview_of_a_page_without_a_price_shows_the_extract_error_with_status_200() {
    let db = TempDb::new("preview-no-price");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let response = post_form(
        &server,
        "/add/preview",
        &with_form_fields(&page.url("/no-price"), &[]),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("no price found on the page"), "body: {body}");
}

#[tokio::test]
async fn preview_of_a_page_answering_404_shows_the_fetch_error_with_status_200() {
    let db = TempDb::new("preview-fetch-error");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let response = post_form(
        &server,
        "/add/preview",
        &with_form_fields(&page.url("/missing"), &[]),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = text(response).await;
    // FetchError::Http(404) displays as "HTTP status 404".
    assert!(body.contains("HTTP status 404"), "body: {body}");
}

#[tokio::test]
async fn preview_with_an_ftp_url_answers_400() {
    let db = TempDb::new("preview-ftp");
    let server = Server::start(&db).await;

    let response = post_form(&server, "/add/preview", &with_form_fields("ftp://x", &[])).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_with_an_empty_url_answers_400() {
    let db = TempDb::new("preview-empty-url");
    let server = Server::start(&db).await;

    let response = post_form(&server, "/add/preview", &with_form_fields("", &[])).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_stores_no_watch() {
    let db = TempDb::new("preview-stores-nothing");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let response = post_form(
        &server,
        "/add/preview",
        &with_form_fields(&page.url("/kettle"), &[("name", "Kettle")]),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(watch_count(&server), 0);
}

#[tokio::test]
async fn preview_does_not_hold_the_database_lock_during_the_fetch() {
    let db = TempDb::new("preview-lock");
    let server = Server::start(&db).await;
    let (requested_tx, requested_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let gate = Arc::new(Gate {
        requested: Mutex::new(Some(requested_tx)),
        release: Mutex::new(Some(release_rx)),
    });
    let page = PageServer::with_gate(gate).await;
    let page_url = page.url("/held");
    let fields = with_form_fields(&page_url, &[]);

    // The page holds its answer until the test has tried the lock, so the preview is still in its
    // fetch when the check runs.
    let request = post_form(&server, "/add/preview", &fields);
    let check = async {
        timeout(DEADLINE, requested_rx)
            .await
            .expect("the product page was requested within 5 s")
            .expect("the product page sent its request signal");
        let lock_is_free = server.conn.try_lock().is_ok();
        release_tx.send(()).ok();
        lock_is_free
    };
    let (response, lock_is_free) = tokio::join!(request, check);

    assert!(lock_is_free, "the database lock was held during the fetch");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn add_redirects_with_303_to_the_new_watch_page() {
    let db = TempDb::new("add-redirect");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(
            &page.url("/kettle"),
            &[("name", "Kettle"), ("target", "40.00"), ("every", "6h")],
        ),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("the redirect has a Location header")
        .to_str()
        .expect("Location is ASCII");
    assert_eq!(location, "/watches/1");
}

#[tokio::test]
async fn add_stores_the_url_given() {
    let db = TempDb::new("add-url");
    let server = Server::start(&db).await;
    let page = PageServer::start().await;
    let page_url = page.url("/kettle");

    post_form(&server, "/add", &with_form_fields(&page_url, &[])).await;

    assert_eq!(watch(&server, 1).url, page_url);
}

#[tokio::test]
async fn add_stores_the_name_given() {
    let db = TempDb::new("add-name");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("name", "Kettle")]),
    )
    .await;

    assert_eq!(watch(&server, 1).name.as_deref(), Some("Kettle"));
}

#[tokio::test]
async fn add_stores_no_name_when_the_name_is_empty() {
    let db = TempDb::new("add-empty-name");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("name", "")]),
    )
    .await;

    assert_eq!(watch(&server, 1).name, None);
}

#[tokio::test]
async fn add_stores_the_selector_given() {
    let db = TempDb::new("add-selector");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("selector", ".price-now")]),
    )
    .await;

    assert_eq!(watch(&server, 1).selector.as_deref(), Some(".price-now"));
}

#[tokio::test]
async fn add_stores_no_selector_when_the_selector_is_empty() {
    let db = TempDb::new("add-empty-selector");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("selector", "")]),
    )
    .await;

    assert_eq!(watch(&server, 1).selector, None);
}

#[tokio::test]
async fn add_stores_40_00_without_a_currency_as_40_euros() {
    let db = TempDb::new("add-target-bare");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("target", "40.00")]),
    )
    .await;

    // A bare number has no currency, so it is taken as EUR. 40.00 EUR is 4000 cents.
    assert_eq!(
        watch(&server, 1).target,
        Some(Money {
            minor: 4000,
            currency: Currency::Eur,
        })
    );
}

#[tokio::test]
async fn add_keeps_the_currency_the_target_names() {
    let db = TempDb::new("add-target-suffix");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("target", "40.00 USD")]),
    )
    .await;

    // 40.00 USD is 4000 cents, and the stored currency is the one the text names.
    assert_eq!(
        watch(&server, 1).target,
        Some(Money {
            minor: 4000,
            currency: Currency::Usd,
        })
    );
}

#[tokio::test]
async fn add_stores_no_target_when_the_target_is_empty() {
    let db = TempDb::new("add-empty-target");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("target", "")]),
    )
    .await;

    assert_eq!(watch(&server, 1).target, None);
}

#[tokio::test]
async fn add_stores_6h_as_21600_seconds() {
    let db = TempDb::new("add-every-6h");
    let server = Server::start(&db).await;

    post_form(&server, "/add", &with_form_fields(SHOP_URL, &[])).await;

    // 6 hours is 6 × 3600 s.
    assert_eq!(watch(&server, 1).interval_s, 21_600);
}

#[tokio::test]
async fn add_stores_1d_as_86400_seconds() {
    let db = TempDb::new("add-every-1d");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "1d")]),
    )
    .await;

    // One day is 24 × 3600 s.
    assert_eq!(watch(&server, 1).interval_s, 86_400);
}

#[tokio::test]
async fn add_accepts_every_5m_as_its_300_second_minimum() {
    let db = TempDb::new("add-every-5m");
    let server = Server::start(&db).await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "5m")]),
    )
    .await;

    // 5 minutes is 300 s, exactly the smallest interval the form accepts.
    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(watch(&server, 1).interval_s, 300);
}

#[tokio::test]
async fn add_with_every_1m_answers_400() {
    let db = TempDb::new("add-every-1m");
    let server = Server::start(&db).await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "1m")]),
    )
    .await;

    // 1 minute is 60 s, below the 300 s minimum.
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_with_every_1m_stores_no_watch() {
    let db = TempDb::new("add-every-1m-stores-nothing");
    let server = Server::start(&db).await;

    post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "1m")]),
    )
    .await;

    assert_eq!(watch_count(&server), 0);
}

#[tokio::test]
async fn add_with_every_299s_answers_400() {
    let db = TempDb::new("add-every-299s");
    let server = Server::start(&db).await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "299s")]),
    )
    .await;

    // 299 s is one second below the 300 s minimum.
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_with_a_negative_every_answers_400() {
    let db = TempDb::new("add-every-negative");
    let server = Server::start(&db).await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "-1h")]),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_with_every_without_a_unit_answers_400() {
    let db = TempDb::new("add-every-no-unit");
    let server = Server::start(&db).await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "6")]),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_with_every_in_an_unknown_unit_answers_400() {
    let db = TempDb::new("add-every-unknown-unit");
    let server = Server::start(&db).await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("every", "6w")]),
    )
    .await;

    // The units are s, m, h and d only.
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_with_a_target_that_is_not_a_price_answers_400() {
    let db = TempDb::new("add-target-invalid");
    let server = Server::start(&db).await;

    let response = post_form(
        &server,
        "/add",
        &with_form_fields(SHOP_URL, &[("target", "abc")]),
    )
    .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_with_an_ftp_url_answers_400() {
    let db = TempDb::new("add-ftp");
    let server = Server::start(&db).await;

    let response = post_form(&server, "/add", &with_form_fields("ftp://x", &[])).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_with_an_empty_url_answers_400() {
    let db = TempDb::new("add-empty-url");
    let server = Server::start(&db).await;

    let response = post_form(&server, "/add", &with_form_fields("", &[])).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_of_a_url_already_watched_answers_400() {
    let db = TempDb::new("add-duplicate-status");
    let server = Server::start(&db).await;
    post_form(&server, "/add", &with_form_fields(SHOP_URL, &[])).await;

    let response = post_form(&server, "/add", &with_form_fields(SHOP_URL, &[])).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_of_a_url_already_watched_says_already_watched() {
    let db = TempDb::new("add-duplicate-message");
    let server = Server::start(&db).await;
    post_form(&server, "/add", &with_form_fields(SHOP_URL, &[])).await;

    let response = post_form(&server, "/add", &with_form_fields(SHOP_URL, &[])).await;

    let body = text(response).await;
    assert!(body.contains("already watched"), "body: {body}");
}

#[tokio::test]
async fn add_of_a_url_already_watched_leaves_one_watch() {
    let db = TempDb::new("add-duplicate-count");
    let server = Server::start(&db).await;
    post_form(&server, "/add", &with_form_fields(SHOP_URL, &[])).await;

    post_form(&server, "/add", &with_form_fields(SHOP_URL, &[])).await;

    assert_eq!(watch_count(&server), 1);
}
