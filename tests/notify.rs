//! Tests for `haggle::notify`: `message_for` on hand-built events, and `Notifier::send` against a
//! local HTTP server on 127.0.0.1.

use haggle::db::{Event, EventKind, Watch, WatchStatus};
use haggle::money::{Currency, Money};
use haggle::notify::{Message, Notifier, NotifyError, message_for};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use url::Url;

/// The product page of the watches used here. Its host is `shop.example`.
const SHOP_URL: &str = "https://shop.example/p/1";

/// The topic path the notifier is pointed at in the send tests.
const TOPIC_PATH: &str = "/haggle-test";

/// A request as the local server read it.
struct Recorded {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    /// The value of the first header called `name`, matched without regard to case.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A money amount in EUR. Amounts are in minor units, so 1000 is 10.00 EUR.
fn eur(minor: i64) -> Money {
    Money {
        minor,
        currency: Currency::Eur,
    }
}

/// A watch with the given URL and name, shaped like a new watch: status `Ok`, no validators.
fn watch(url: &str, name: Option<&str>) -> Watch {
    Watch {
        id: 1,
        url: url.to_string(),
        name: name.map(str::to_string),
        selector: None,
        target: None,
        interval_s: 3600,
        enabled: true,
        status: WatchStatus::Ok,
        etag: None,
        last_modified: None,
        next_check_at: 0,
        failures: 0,
        created_at: 0,
    }
}

/// The watch named "Kettle" at `SHOP_URL`.
fn kettle() -> Watch {
    watch(SHOP_URL, Some("Kettle"))
}

/// An event on watch 1. The prices are minor units, `None` for an absent price.
fn event(kind: EventKind, old: Option<i64>, new: Option<i64>) -> Event {
    Event {
        watch_id: 1,
        kind,
        old: old.map(eur),
        new: new.map(eur),
    }
}

/// The lines of a message body.
fn body_lines(message: &Message) -> Vec<&str> {
    message.body.lines().collect()
}

/// The phrase of each event kind, in the priority order that picks the lead event.
fn priority_table() -> Vec<(EventKind, &'static str)> {
    vec![
        (EventKind::TargetHit, "Target price reached"),
        (EventKind::LowestEver, "Lowest price ever"),
        (EventKind::InflatedSale, "Inflated sale"),
        (EventKind::PriceDrop, "Price drop"),
        (EventKind::BackInStock, "Back in stock"),
        (EventKind::OutOfStock, "Out of stock"),
        (EventKind::PriceRise, "Price rise"),
    ]
}

#[test]
fn single_price_drop_titles_with_phrase_and_watch_name() {
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.title, "Price drop: Kettle");
}

#[test]
fn single_price_drop_body_shows_old_and_new_price() {
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(body_lines(&message), ["Price drop: 10.00 EUR → 9.00 EUR"]);
}

#[test]
fn click_is_the_watch_url() {
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.click, SHOP_URL);
}

#[test]
fn price_drop_is_not_high_priority() {
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&kettle(), &events);
    assert!(!message.high_priority);
}

#[test]
fn lowest_ever_leads_the_title_over_price_drop() {
    let events = [
        event(EventKind::PriceDrop, Some(1000), Some(900)),
        event(EventKind::LowestEver, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.title, "Lowest price ever: Kettle");
}

#[test]
fn body_has_one_line_per_event_in_input_order() {
    let events = [
        event(EventKind::PriceDrop, Some(1000), Some(900)),
        event(EventKind::LowestEver, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert_eq!(
        body_lines(&message),
        [
            "Price drop: 10.00 EUR → 9.00 EUR",
            "Lowest price ever: 9.00 EUR",
        ]
    );
}

#[test]
fn body_follows_input_order_not_priority_order() {
    let events = [
        event(EventKind::LowestEver, None, Some(900)),
        event(EventKind::PriceDrop, Some(1000), Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert_eq!(
        body_lines(&message),
        [
            "Lowest price ever: 9.00 EUR",
            "Price drop: 10.00 EUR → 9.00 EUR",
        ]
    );
}

#[test]
fn target_hit_is_high_priority_and_leads_the_title() {
    let events = [event(EventKind::TargetHit, None, Some(900))];
    let message = message_for(&kettle(), &events);
    assert!(message.high_priority);
    assert_eq!(message.title, "Target price reached: Kettle");
}

#[test]
fn target_hit_among_other_events_makes_the_message_high_priority() {
    let events = [
        event(EventKind::PriceDrop, Some(1000), Some(900)),
        event(EventKind::TargetHit, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert!(message.high_priority);
}

#[test]
fn message_without_target_hit_is_not_high_priority() {
    let events = [
        event(EventKind::LowestEver, None, Some(900)),
        event(EventKind::BackInStock, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert!(!message.high_priority);
}

#[test]
fn lead_is_the_highest_priority_event_whatever_the_input_order() {
    // For each cut of the priority table, the events are the rest of the table in reverse
    // order. The lead must still be the first kind of the cut.
    for start in 0..priority_table().len() {
        let table = priority_table();
        let phrase = table[start].1;
        let events: Vec<Event> = table
            .into_iter()
            .skip(start)
            .rev()
            .map(|(kind, _)| event(kind, None, None))
            .collect();
        let message = message_for(&kettle(), &events);
        assert_eq!(message.title, format!("{phrase}: Kettle"), "cut at {start}");
    }
}

#[test]
fn label_is_the_host_when_the_watch_has_no_name() {
    let shop = watch(SHOP_URL, None);
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&shop, &events);
    assert_eq!(message.title, "Price drop: shop.example");
}

#[test]
fn label_is_the_url_when_the_watch_has_no_name_and_the_url_has_no_host() {
    let odd = watch("not a url", None);
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&odd, &events);
    assert_eq!(message.title, "Price drop: not a url");
}

#[test]
fn inflated_sale_body_names_new_price_and_claimed_old_price() {
    // 1200 minor units claimed was, 700 is what the item costs now.
    let events = [event(EventKind::InflatedSale, Some(1200), Some(700))];
    let message = message_for(&kettle(), &events);
    assert_eq!(
        body_lines(&message),
        ["Inflated sale: 7.00 EUR, claimed was 12.00 EUR"]
    );
}

#[test]
fn back_in_stock_with_only_new_price_shows_phrase_and_price() {
    let events = [event(EventKind::BackInStock, None, Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(body_lines(&message), ["Back in stock: 9.00 EUR"]);
}

#[test]
fn back_in_stock_titles_with_phrase_and_watch_name() {
    let events = [event(EventKind::BackInStock, None, Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.title, "Back in stock: Kettle");
}

#[test]
fn price_rise_body_shows_old_and_new_price() {
    let events = [event(EventKind::PriceRise, Some(900), Some(1000))];
    let message = message_for(&kettle(), &events);
    assert_eq!(body_lines(&message), ["Price rise: 9.00 EUR → 10.00 EUR"]);
}

#[test]
fn event_without_prices_has_phrase_only_in_body() {
    let events = [event(EventKind::OutOfStock, None, None)];
    let message = message_for(&kettle(), &events);
    assert_eq!(body_lines(&message), ["Out of stock"]);
}

/// The message a send test posts when it does not care about the content.
fn sample_message() -> Message {
    Message {
        title: "Price drop: Kettle".to_string(),
        body: "Price drop: 10.00 EUR → 9.00 EUR".to_string(),
        click: SHOP_URL.to_string(),
        high_priority: false,
    }
}

/// A raw HTTP/1.1 reply with an empty body and the given status line.
fn reply(status: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

/// Reads more bytes of a request into `raw`. A connection that closes mid-request fails the test.
async fn read_more(socket: &mut TcpStream, raw: &mut Vec<u8>) {
    let mut chunk = [0u8; 512];
    let read = socket.read(&mut chunk).await.expect("read request");
    assert!(read > 0, "client closed the connection mid-request");
    raw.extend_from_slice(&chunk[..read]);
}

/// Reads one request: the head up to the blank line, then the body its Content-Length gives.
async fn read_request(socket: &mut TcpStream) -> Recorded {
    let mut raw = Vec::new();
    let head_end = loop {
        if let Some(at) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        read_more(socket, &mut raw).await;
    };
    let head = String::from_utf8(raw[..head_end].to_vec()).expect("request head is UTF-8");
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("method").to_string();
    let path = request_line.next().expect("request target").to_string();
    let headers: Vec<(String, String)> = lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (name, value) = line.split_once(':').expect("header line has a colon");
            (name.trim().to_string(), value.trim().to_string())
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, value)| {
            value.parse().expect("numeric Content-Length")
        });
    while raw.len() < head_end + length {
        read_more(socket, &mut raw).await;
    }
    let body = String::from_utf8(raw[head_end..head_end + length].to_vec())
        .expect("request body is UTF-8");
    Recorded {
        method,
        path,
        headers,
        body,
    }
}

/// Serves one connection on 127.0.0.1 with `reply`. The URL is the topic path on that server.
/// The handle yields what the client sent.
async fn serve_once(reply: Vec<u8>) -> (Url, JoinHandle<Recorded>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let address = listener.local_addr().expect("local address");
    let url = Url::parse(&format!("http://{address}{TOPIC_PATH}")).expect("valid url");
    let request = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept connection");
        let request = read_request(&mut socket).await;
        socket.write_all(&reply).await.expect("write reply");
        socket.shutdown().await.expect("shut down connection");
        request
    });
    (url, request)
}

/// Sends `message` with `token` to a local server that answers 200, and returns what it got.
async fn sent(token: Option<&str>, message: &Message) -> Recorded {
    let (url, request) = serve_once(reply("200 OK")).await;
    let notifier = Notifier::new(url, token.map(str::to_string)).expect("notifier should build");
    notifier.send(message).await.expect("send should succeed");
    request.await.expect("server task should finish")
}

/// The error of a send that must fail. Panics if the send succeeded.
fn send_error(result: Result<(), NotifyError>) -> NotifyError {
    match result {
        Ok(()) => panic!("expected the send to fail"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn send_posts_to_the_topic_path() {
    let request = sent(None, &sample_message()).await;
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, TOPIC_PATH);
}

#[tokio::test]
async fn send_body_is_the_message_body_as_utf8_text() {
    let request = sent(None, &sample_message()).await;
    assert_eq!(request.body, "Price drop: 10.00 EUR → 9.00 EUR");
}

#[tokio::test]
async fn send_declares_the_body_as_plain_text() {
    let request = sent(None, &sample_message()).await;
    let content_type = request.header("Content-Type").expect("Content-Type header");
    assert!(
        content_type.starts_with("text/plain"),
        "Content-Type was {content_type:?}"
    );
}

#[tokio::test]
async fn send_sends_ascii_title_as_is_in_title_header() {
    let request = sent(None, &sample_message()).await;
    assert_eq!(request.header("Title"), Some("Price drop: Kettle"));
}

#[tokio::test]
async fn send_sends_click_header_as_the_watch_url() {
    let request = sent(None, &sample_message()).await;
    assert_eq!(request.header("Click"), Some(SHOP_URL));
}

#[tokio::test]
async fn send_sends_bearer_authorization_when_a_token_is_set() {
    let request = sent(Some("tok"), &sample_message()).await;
    assert_eq!(request.header("Authorization"), Some("Bearer tok"));
}

#[tokio::test]
async fn send_sends_no_authorization_header_without_a_token() {
    let request = sent(None, &sample_message()).await;
    assert_eq!(request.header("Authorization"), None);
}

#[tokio::test]
async fn send_sets_priority_4_for_high_priority_message() {
    let message = Message {
        high_priority: true,
        ..sample_message()
    };
    let request = sent(None, &message).await;
    assert_eq!(request.header("Priority"), Some("4"));
}

#[tokio::test]
async fn send_sends_no_priority_header_for_normal_message() {
    let request = sent(None, &sample_message()).await;
    assert_eq!(request.header("Priority"), None);
}

#[tokio::test]
async fn send_encodes_non_ascii_title_as_rfc_2047_base64() {
    // "Café" is the bytes 43 61 66 C3 A9 in UTF-8, which base64 writes as "Q2Fmw6k=".
    let message = Message {
        title: "Café".to_string(),
        ..sample_message()
    };
    let request = sent(None, &message).await;
    assert_eq!(request.header("Title"), Some("=?UTF-8?B?Q2Fmw6k=?="));
}

#[tokio::test]
async fn send_returns_status_error_with_the_code_of_a_500_reply() {
    let (url, _request) = serve_once(reply("500 Internal Server Error")).await;
    let notifier = Notifier::new(url, None).expect("notifier should build");
    let error = send_error(notifier.send(&sample_message()).await);
    assert_eq!(error, NotifyError::Status(500));
}

#[tokio::test]
async fn send_returns_status_error_with_the_code_of_a_403_reply() {
    let (url, _request) = serve_once(reply("403 Forbidden")).await;
    let notifier = Notifier::new(url, None).expect("notifier should build");
    let error = send_error(notifier.send(&sample_message()).await);
    assert_eq!(error, NotifyError::Status(403));
}

#[tokio::test]
async fn send_returns_network_error_when_nothing_listens_on_the_port() {
    // Bind a port, then release it, so nothing is listening when the send connects.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let address = listener.local_addr().expect("local address");
    drop(listener);
    let url = Url::parse(&format!("http://{address}{TOPIC_PATH}")).expect("valid url");
    let notifier = Notifier::new(url, None).expect("notifier should build");
    let error = send_error(notifier.send(&sample_message()).await);
    assert!(
        matches!(error, NotifyError::Network(_)),
        "expected a network error, got {error:?}"
    );
}
