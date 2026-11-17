//! Tests for `haggle::scheduler`: `jittered` on its own, and `tick` and `run` end to end against a
//! local shop and a fake SMTP server on 127.0.0.1. No test touches the internet or the live database.

#[path = "support/fake_smtp.rs"]
mod fake_smtp;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake_smtp::FakeSmtp;
use haggle::config::ensure_scratch_path;
use haggle::db::{EventKind, NewWatch, StoredEvent, events, open, watches};
use haggle::fetch::{Fetcher, Politeness};
use haggle::notify::Notifier;
use haggle::scheduler::{BATCH, TickReport, jittered, run, tick};
use rusqlite::{Connection, params};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

/// The insert time and the first tick, in Unix seconds.
const T0: i64 = 1_700_000_000;
/// The check interval of every watch in these tests: one hour, in seconds.
const INTERVAL_S: i64 = 3600;
/// Timeout for the fetcher, and the bound on a `run` that must return at once.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Price shown by the shop before a test changes it.
const PRICE: &str = "10.00";
/// Price that is a new low, so a drop from `PRICE` makes a PriceDrop and a LowestEver.
const DROPPED: &str = "9.00";
/// The sender of every email in these tests.
const SENDER: &str = "haggle@example.com";
/// The recipient of every email in these tests.
const RECIPIENT: &str = "kettle-alerts@example.com";
/// The reply code for a temporary failure: the server refuses the message now, and the sender may
/// try again later.
const TEMPORARY_FAILURE: u16 = 451;
/// The reply code that accepts a message at the end of DATA.
const ACCEPTED: u16 = 250;

/// A time at which every watch is due again: two intervals after `now`, which is past the
/// largest jitter (+10 %), so a watch checked at `now` is due by then.
fn later(now: i64) -> i64 {
    now + 2 * INTERVAL_S
}

/// A database file under the temp dir. The file and its WAL side files are removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Names the file after this process and `test`, and clears any leftover from an earlier run.
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "haggle-scheduler-{}-{test}.sqlite",
            std::process::id()
        ));
        let scratch = Scratch { path };
        scratch.remove_files();
        scratch
    }

    fn remove_files(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(name));
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove_files();
    }
}

/// How the shop answers, fixed when it starts.
#[derive(Clone, Copy)]
struct Behaviour {
    /// Body of `/robots.txt`; `None` answers 404.
    robots: Option<&'static str>,
    /// Status line of every product page. A status other than 200 sends no body.
    page_status: &'static str,
}

/// A shop that has no robots.txt and serves its product pages.
const OPEN_SHOP: Behaviour = Behaviour {
    robots: None,
    page_status: "200 OK",
};

/// A local shop serving `/robots.txt` and `/p/*`. The price on each page is read on every request.
struct Shop {
    url: Url,
    /// The price the next page shows, such as "10.00". Empty means the page shows no price.
    price: Arc<Mutex<String>>,
    /// Request targets in arrival order.
    requests: Arc<Mutex<Vec<String>>>,
}

impl Shop {
    /// The URL of product `n` on this shop.
    fn product_url(&self, n: usize) -> Url {
        self.url
            .join(&format!("/p/{n}"))
            .expect("valid product url")
    }

    /// How many requests for `target` the shop has seen.
    fn requests_to(&self, target: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.as_str() == target)
            .count()
    }

    /// How many requests of any kind the shop has seen.
    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// Sets the price the shop shows from now on.
fn set_price(shop: &Shop, price: &str) {
    *shop.price.lock().unwrap() = price.to_owned();
}

/// Everything one test needs: a scratch database holding one watch on the shop's `/p/1`, the shop,
/// the fake SMTP server, and the clients a tick uses.
struct Fixture {
    conn: Connection,
    scratch: Scratch,
    shop: Shop,
    smtp: FakeSmtp,
    fetcher: Fetcher,
    notifier: Notifier,
    watch_id: i64,
}

impl Fixture {
    /// Runs one tick at `now`, notifying through the fake SMTP server.
    async fn tick_at(&self, now: i64) -> TickReport {
        tick_with(&self.conn, &self.fetcher, Some(&self.notifier), now).await
    }
}

/// Starts a shop showing `price` with `behaviour`, a fake SMTP server, and a scratch database. The
/// database holds one watch on `/p/1`, due at `T0` and checked every `INTERVAL_S`.
async fn setup(test: &str, price: &str, behaviour: Behaviour) -> Fixture {
    let scratch = Scratch::new(test);
    assert_eq!(
        ensure_scratch_path(&scratch.path),
        Ok(()),
        "the scratch database must not be the live one"
    );
    let conn = open(&scratch.path).expect("open scratch database");
    let shop = start_shop(price, behaviour).await;
    let smtp = FakeSmtp::start().await;
    let fetcher = Fetcher::new(TIMEOUT, 1_048_576)
        .expect("client should build")
        .with_politeness(Politeness {
            host_interval: Duration::ZERO,
            ..Politeness::default()
        });
    let notifier = Notifier::unencrypted("127.0.0.1", smtp.port(), SENDER, RECIPIENT)
        .expect("notifier should build");
    let watch_id = insert_watch(&conn, shop.product_url(1).as_str(), None);
    Fixture {
        conn,
        scratch,
        shop,
        smtp,
        fetcher,
        notifier,
        watch_id,
    }
}

/// Runs one tick at `now` with the given clients.
async fn tick_with(
    conn: &Connection,
    fetcher: &Fetcher,
    notifier: Option<&Notifier>,
    now: i64,
) -> TickReport {
    tick(conn, fetcher, notifier, now)
        .await
        .expect("tick should not fail with a database error")
}

/// Inserts a watch on `url` with an optional CSS `selector`, due at `T0`, with a one-hour interval.
fn insert_watch(conn: &Connection, url: &str, selector: Option<&str>) -> i64 {
    let new = NewWatch {
        url: url.to_string(),
        name: Some("Kettle".to_string()),
        selector: selector.map(str::to_string),
        target: None,
        interval_s: INTERVAL_S,
    };
    watches::insert(conn, &new, T0).expect("insert watch")
}

/// The columns of a `watches` row that the scheduler writes.
struct WatchRow {
    status: String,
    failures: i64,
    next_check_at: i64,
}

/// Reads the scheduler's columns of watch `id` straight from the table.
fn watch_row(conn: &Connection, id: i64) -> WatchRow {
    conn.query_row(
        "SELECT status, failures, next_check_at FROM watches WHERE id = ?1",
        params![id],
        |row| {
            Ok(WatchRow {
                status: row.get(0)?,
                failures: row.get(1)?,
                next_check_at: row.get(2)?,
            })
        },
    )
    .expect("read watch row")
}

/// The number of observations stored for watch `watch_id`.
fn observation_count(conn: &Connection, watch_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM observations WHERE watch_id = ?1",
        params![watch_id],
        |row| row.get(0),
    )
    .expect("count observations")
}

/// The events stored for watch `watch_id`, failing the test if the read fails.
fn stored_events(conn: &Connection, watch_id: i64) -> Vec<StoredEvent> {
    events::for_watch(conn, watch_id).expect("read events")
}

/// The number of events of watch `watch_id` that no notification has been sent for.
fn unnotified_count(conn: &Connection, watch_id: i64) -> usize {
    stored_events(conn, watch_id)
        .iter()
        .filter(|stored| stored.notified_at.is_none())
        .count()
}

/// A raw HTTP/1.1 reply: the status line, the `headers` (each a full `Name: value` line), a
/// `Content-Length` and `Connection: close`, then `body`.
fn reply(status: &str, headers: &[&str], body: &str) -> Vec<u8> {
    let mut text = format!("HTTP/1.1 {status}\r\n");
    for header_line in headers {
        text.push_str(header_line);
        text.push_str("\r\n");
    }
    text.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    text.push_str(body);
    text.into_bytes()
}

/// The value of header `name` in a request head, matched without case.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// The request target of a request head, such as `/p/1`.
fn request_target(head: &str) -> &str {
    head.split_whitespace().nth(1).expect("request target")
}

/// Reads one request: the head through its blank line, then the body if the head declares a
/// `Content-Length`. Returns the head.
async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 512];
    let head_end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        let read = socket.read(&mut chunk).await.expect("read request");
        if read == 0 {
            break bytes.len();
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let body_len = header(&head, "Content-Length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < head_end + body_len {
        let read = socket.read(&mut chunk).await.expect("read request body");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    head
}

/// Binds a listener on 127.0.0.1 with a port the OS chooses.
async fn bind() -> TcpListener {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port")
}

/// The base URL (with a trailing slash) of `listener`.
fn base_url(listener: &TcpListener) -> Url {
    let address = listener.local_addr().expect("local address");
    Url::parse(&format!("http://{address}/")).expect("valid url")
}

/// Accepts connections on `listener` in the background. Each connection gets the reply that
/// `respond` makes to its request head. The tasks end with the test runtime.
fn serve(listener: TcpListener, respond: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) {
    let respond = Arc::new(respond);
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept connection");
            let respond = Arc::clone(&respond);
            tokio::spawn(async move {
                let head = read_request(&mut socket).await;
                let answer = respond(&head);
                // The client may close early; a failed write is not a test failure.
                let _ = socket.write_all(&answer).await;
                let _ = socket.shutdown().await;
            });
        }
    });
}

/// The HTML of a product page showing `price` in EUR as JSON-LD. Empty `price` gives a page with
/// no price at all.
fn product_page(price: &str) -> String {
    if price.is_empty() {
        return "<html><head><title>Kettle</title></head><body><h1>Kettle</h1>\
                <p>Sold out</p></body></html>"
            .to_owned();
    }
    format!(
        r#"<html><head><script type="application/ld+json">{{"@context":"https://schema.org","@type":"Product","name":"Kettle","offers":{{"@type":"Offer","price":"{price}","priceCurrency":"EUR","availability":"https://schema.org/InStock"}}}}</script></head><body><h1>Kettle</h1></body></html>"#
    )
}

/// The answer the shop gives to one request, given its target and head.
fn shop_reply(target: &str, head: &str, price: &str, behaviour: Behaviour) -> Vec<u8> {
    if target == "/robots.txt" {
        return match behaviour.robots {
            Some(body) => reply("200 OK", &[], body),
            None => reply("404 Not Found", &[], ""),
        };
    }
    if !target.starts_with("/p/") {
        return reply("404 Not Found", &[], "");
    }
    if behaviour.page_status != "200 OK" {
        return reply(behaviour.page_status, &[], "");
    }
    // The ETag names the price, so a changed price is a changed page and a client's
    // If-None-Match for the old price gets the new page.
    let etag = format!("\"{price}\"");
    if header(head, "If-None-Match") == Some(etag.as_str()) {
        return reply("304 Not Modified", &[], "");
    }
    let etag_line = format!("ETag: {etag}");
    reply("200 OK", &[etag_line.as_str()], &product_page(price))
}

/// Starts a shop that shows `price` with `behaviour`.
async fn start_shop(price: &str, behaviour: Behaviour) -> Shop {
    let listener = bind().await;
    let url = base_url(&listener);
    let price = Arc::new(Mutex::new(price.to_owned()));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let shared_price = Arc::clone(&price);
    let shared_requests = Arc::clone(&requests);
    serve(listener, move |head| {
        let target = request_target(head).to_owned();
        shared_requests.lock().unwrap().push(target.clone());
        let current = shared_price.lock().unwrap().clone();
        shop_reply(&target, head, &current, behaviour)
    });
    Shop {
        url,
        price,
        requests,
    }
}

#[tokio::test]
async fn first_tick_stores_one_observation_and_sends_nothing() {
    let fixture = setup("first_tick", PRICE, OPEN_SHOP).await;

    let report = fixture.tick_at(T0).await;

    assert_eq!(
        report,
        TickReport {
            checked: 1,
            failed: 0,
            events: 0,
            notified: 0,
        },
        "the first price has nothing to compare with, so no events"
    );
    assert_eq!(observation_count(&fixture.conn, fixture.watch_id), 1);
    assert_eq!(fixture.smtp.count(), 0);
}

#[tokio::test]
async fn price_drop_sends_one_message_for_both_events() {
    let fixture = setup("price_drop", PRICE, OPEN_SHOP).await;
    fixture.tick_at(T0).await;
    set_price(&fixture.shop, DROPPED);

    let report = fixture.tick_at(later(T0)).await;

    assert_eq!(
        report.events, 2,
        "a drop below every earlier price is two events"
    );
    assert_eq!(
        fixture.smtp.count(),
        1,
        "both events of one watch and time go in one email"
    );
    let messages = fixture.smtp.messages();
    let watch_url = fixture.shop.product_url(1);
    assert!(
        messages[0].data.contains(watch_url.as_str()),
        "the email does not contain the watch URL {watch_url}: {}",
        messages[0].data
    );
    let stored = stored_events(&fixture.conn, fixture.watch_id);
    let kinds: Vec<EventKind> = stored.iter().map(|stored| stored.event.kind).collect();
    assert!(kinds.contains(&EventKind::PriceDrop), "kinds: {kinds:?}");
    assert!(kinds.contains(&EventKind::LowestEver), "kinds: {kinds:?}");
    assert_eq!(unnotified_count(&fixture.conn, fixture.watch_id), 0);
}

#[tokio::test]
async fn email_is_sent_from_the_sender_to_the_recipient() {
    let fixture = setup("envelope", PRICE, OPEN_SHOP).await;
    fixture.tick_at(T0).await;
    set_price(&fixture.shop, DROPPED);
    fixture.tick_at(later(T0)).await;

    let messages = fixture.smtp.messages();

    assert_eq!(messages.len(), 1, "one email for the drop");
    assert_eq!(messages[0].from, SENDER);
    assert_eq!(messages[0].recipients, [RECIPIENT]);
}

#[tokio::test]
async fn unchanged_price_on_a_later_tick_sends_nothing() {
    let fixture = setup("unchanged_price", PRICE, OPEN_SHOP).await;
    fixture.tick_at(T0).await;
    set_price(&fixture.shop, DROPPED);
    fixture.tick_at(later(T0)).await;

    let report = fixture.tick_at(later(later(T0))).await;

    assert_eq!(
        report.events, 0,
        "the price did not move, so there is no new event"
    );
    assert_eq!(
        fixture.smtp.count(),
        1,
        "no second email for the same price"
    );
}

#[tokio::test]
async fn restart_does_not_resend_notified_events() {
    let mut fixture = setup("restart", PRICE, OPEN_SHOP).await;
    fixture.tick_at(T0).await;
    set_price(&fixture.shop, DROPPED);
    fixture.tick_at(later(T0)).await;
    let path = fixture.scratch.path.clone();

    // A restart: the connection goes, and the same file is opened again.
    drop(fixture.conn);
    fixture.conn = open(&path).expect("reopen scratch database");
    tick_with(
        &fixture.conn,
        &fixture.fetcher,
        Some(&fixture.notifier),
        later(later(T0)),
    )
    .await;

    assert_eq!(
        fixture.smtp.count(),
        1,
        "the notified events must not be sent again"
    );
    assert_eq!(unnotified_count(&fixture.conn, fixture.watch_id), 0);
}

#[tokio::test]
async fn failed_notification_leaves_events_unnotified() {
    let fixture = setup("smtp_fails", PRICE, OPEN_SHOP).await;
    fixture.smtp.answer_data_with(TEMPORARY_FAILURE);
    fixture.tick_at(T0).await;
    set_price(&fixture.shop, DROPPED);

    let report = fixture.tick_at(later(T0)).await;

    assert_eq!(report.events, 2);
    assert_eq!(
        fixture.smtp.count(),
        0,
        "the server refused the email, so none is recorded"
    );
    assert_eq!(
        unnotified_count(&fixture.conn, fixture.watch_id),
        2,
        "events whose email failed stay unnotified"
    );
}

#[tokio::test]
async fn events_are_sent_once_after_smtp_recovers() {
    let fixture = setup("smtp_recovers", PRICE, OPEN_SHOP).await;
    fixture.smtp.answer_data_with(TEMPORARY_FAILURE);
    fixture.tick_at(T0).await;
    set_price(&fixture.shop, DROPPED);
    fixture.tick_at(later(T0)).await;
    fixture.smtp.answer_data_with(ACCEPTED);

    fixture.tick_at(later(later(T0))).await;

    assert_eq!(
        fixture.smtp.count(),
        1,
        "the refused email is not recorded, and the retry is recorded once"
    );
    assert_eq!(unnotified_count(&fixture.conn, fixture.watch_id), 0);

    fixture.tick_at(later(later(later(T0)))).await;

    assert_eq!(
        fixture.smtp.count(),
        1,
        "the events are not sent again once they are notified"
    );
}

#[tokio::test]
async fn rate_limited_page_sets_error_and_pushes_next_check_back() {
    let behaviour = Behaviour {
        page_status: "429 Too Many Requests",
        ..OPEN_SHOP
    };
    let fixture = setup("rate_limited", PRICE, behaviour).await;

    fixture.tick_at(T0).await;

    let row = watch_row(&fixture.conn, fixture.watch_id);
    assert_eq!(row.status, "error");
    assert_eq!(row.failures, 1);
    assert!(
        row.next_check_at > T0 + INTERVAL_S,
        "a rate-limited watch waits longer than its interval, got {}",
        row.next_check_at
    );
}

#[tokio::test]
async fn rate_limited_watch_is_not_requested_again_at_the_same_time() {
    let behaviour = Behaviour {
        page_status: "429 Too Many Requests",
        ..OPEN_SHOP
    };
    let fixture = setup("rate_limited_not_due", PRICE, behaviour).await;
    fixture.tick_at(T0).await;
    let requests_after_first_tick = fixture.shop.request_count();

    let report = fixture.tick_at(T0).await;

    assert_eq!(
        report.checked, 0,
        "the watch is not due before its backoff ends"
    );
    assert_eq!(
        fixture.shop.request_count(),
        requests_after_first_tick,
        "the shop gets no request for a watch that is not due"
    );
}

#[tokio::test]
async fn robots_txt_disallowing_everything_blocks_the_watch_without_fetching_the_page() {
    let behaviour = Behaviour {
        robots: Some("User-agent: *\nDisallow: /\n"),
        ..OPEN_SHOP
    };
    let fixture = setup("robots_blocked", PRICE, behaviour).await;

    fixture.tick_at(T0).await;

    assert_eq!(watch_row(&fixture.conn, fixture.watch_id).status, "blocked");
    assert_eq!(
        fixture.shop.requests_to("/p/1"),
        0,
        "a disallowed page must not be requested"
    );
}

#[tokio::test]
async fn forbidden_page_blocks_the_watch() {
    let behaviour = Behaviour {
        page_status: "403 Forbidden",
        ..OPEN_SHOP
    };
    let fixture = setup("forbidden", PRICE, behaviour).await;

    fixture.tick_at(T0).await;

    assert_eq!(watch_row(&fixture.conn, fixture.watch_id).status, "blocked");
}

#[tokio::test]
async fn server_error_page_sets_error_status() {
    let behaviour = Behaviour {
        page_status: "500 Internal Server Error",
        ..OPEN_SHOP
    };
    let fixture = setup("server_error", PRICE, behaviour).await;

    fixture.tick_at(T0).await;

    let row = watch_row(&fixture.conn, fixture.watch_id);
    assert_eq!(row.status, "error");
    assert_eq!(row.failures, 1);
}

#[tokio::test]
async fn page_without_price_and_no_selector_needs_a_selector() {
    let fixture = setup("needs_selector", "", OPEN_SHOP).await;

    fixture.tick_at(T0).await;

    let row = watch_row(&fixture.conn, fixture.watch_id);
    assert_eq!(row.status, "needs-selector");
    assert_eq!(row.failures, 1);
}

#[tokio::test]
async fn page_without_price_under_a_selector_is_an_error() {
    let fixture = setup("selector_error", "", OPEN_SHOP).await;
    let with_selector = insert_watch(
        &fixture.conn,
        fixture.shop.product_url(2).as_str(),
        Some("span.price"),
    );

    fixture.tick_at(T0).await;

    assert_eq!(
        watch_row(&fixture.conn, with_selector).status,
        "error",
        "a watch that has a selector and finds no price is an error, not needs-selector"
    );
    assert_eq!(
        watch_row(&fixture.conn, fixture.watch_id).status,
        "needs-selector",
        "the watch without a selector still needs one"
    );
}

#[tokio::test]
async fn bad_url_fails_only_its_own_watch() {
    let fixture = setup("bad_url", PRICE, OPEN_SHOP).await;
    let bad = insert_watch(&fixture.conn, "not a url", None);

    let report = fixture.tick_at(T0).await;

    assert_eq!(report.checked, 2);
    assert_eq!(report.failed, 1, "only the bad url fails");
    let row = watch_row(&fixture.conn, bad);
    assert_eq!(row.status, "error");
    assert_eq!(row.failures, 1);
    assert_eq!(
        observation_count(&fixture.conn, fixture.watch_id),
        1,
        "the good watch is still checked in the same tick"
    );
}

#[tokio::test]
async fn tick_checks_at_most_batch_watches() {
    let fixture = setup("batch", PRICE, OPEN_SHOP).await;
    // One more watch than a tick may check.
    for n in 2..=BATCH + 1 {
        insert_watch(&fixture.conn, fixture.shop.product_url(n).as_str(), None);
    }

    let first = fixture.tick_at(T0).await;
    let second = fixture.tick_at(T0).await;

    assert_eq!(first.checked, BATCH, "one tick checks BATCH watches");
    assert_eq!(
        second.checked, 1,
        "the watch left out by the first tick is due at the same time and is checked next"
    );
}

#[tokio::test]
async fn unchanged_page_stores_a_copy_of_the_latest_observation() {
    let fixture = setup("not_modified", PRICE, OPEN_SHOP).await;
    fixture.tick_at(T0).await;

    // The price is the same, so the shop answers 304 to the saved validator.
    let report = fixture.tick_at(later(T0)).await;

    assert_eq!(report.events, 0);
    assert_eq!(
        observation_count(&fixture.conn, fixture.watch_id),
        2,
        "a not-modified check still records an observation at its time"
    );
    assert_eq!(watch_row(&fixture.conn, fixture.watch_id).status, "ok");
}

#[tokio::test]
async fn tick_without_a_notifier_leaves_events_unnotified() {
    let fixture = setup("no_notifier", PRICE, OPEN_SHOP).await;
    tick_with(&fixture.conn, &fixture.fetcher, None, T0).await;
    set_price(&fixture.shop, DROPPED);

    let report = tick_with(&fixture.conn, &fixture.fetcher, None, later(T0)).await;

    assert_eq!(report.events, 2);
    assert_eq!(report.notified, 0, "nothing is sent without a notifier");
    assert_eq!(unnotified_count(&fixture.conn, fixture.watch_id), 2);
    assert_eq!(fixture.smtp.count(), 0);
}

#[tokio::test]
async fn run_returns_ok_when_shutdown_is_already_complete() {
    let fixture = setup("run_ready_shutdown", PRICE, OPEN_SHOP).await;

    let outcome = tokio::time::timeout(
        TIMEOUT,
        run(
            &fixture.conn,
            &fixture.fetcher,
            Some(&fixture.notifier),
            std::future::ready(()),
        ),
    )
    .await;

    let result = outcome.expect("run should return at once, not wait for the next tick");
    assert!(result.is_ok(), "run should end cleanly, got {result:?}");
    assert!(
        fixture.shop.requests_to("/p/1") <= 1,
        "run should make at most one tick before it stops"
    );
}

#[test]
fn jittered_is_unchanged_at_unit_zero() {
    assert_eq!(jittered(1000, 0.0), 1000);
}

#[test]
fn jittered_adds_ten_percent_at_unit_one() {
    assert_eq!(jittered(1000, 1.0), 1100);
}

#[test]
fn jittered_takes_ten_percent_off_at_unit_minus_one() {
    assert_eq!(jittered(1000, -1.0), 900);
}

#[test]
fn jittered_scales_linearly_within_ten_percent() {
    // 1000 * (1 + 0.1 * 0.5) = 1050 and 1000 * (1 - 0.1 * 0.5) = 950.
    assert_eq!(jittered(1000, 0.5), 1050);
    assert_eq!(jittered(1000, -0.5), 950);
}

#[test]
fn jittered_clamps_units_above_one() {
    assert_eq!(jittered(1000, 5.0), 1100);
}

#[test]
fn jittered_clamps_units_below_minus_one() {
    assert_eq!(jittered(1000, -5.0), 900);
}

#[test]
fn jittered_rounds_to_the_nearest_second() {
    // 15 * 1.05 = 15.75 rounds to 16, and 15 * 0.95 = 14.25 rounds to 14.
    assert_eq!(jittered(15, 0.5), 16);
    assert_eq!(jittered(15, -0.5), 14);
}

#[test]
fn jittered_is_never_below_one_second() {
    // 1 * 0.9 = 0.9 rounds to 1, and the floor of 1 applies in any case.
    assert_eq!(jittered(1, -1.0), 1);
}
