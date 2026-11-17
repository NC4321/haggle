//! Tests for `haggle::notify`: `message_for` on hand-built events, and `Notifier` sending through
//! a fake SMTP server on 127.0.0.1.

#[path = "support/fake_smtp.rs"]
mod fake_smtp;

use fake_smtp::{FakeSmtp, ReceivedMessage};
use haggle::config::{SMTP_HOST, SMTP_PORT};
use haggle::db::{Event, EventKind, Watch, WatchStatus};
use haggle::money::{Currency, Money};
use haggle::notify::{Message, Notifier, NotifyError, message_for};
use tokio::net::TcpListener;

/// The product page of the watches used here. Its host is `shop.example`.
const SHOP_URL: &str = "https://shop.example/p/1";

/// The sender and recipient addresses of the send tests.
const FROM: &str = "haggle@example.com";
const TO: &str = "me@example.com";

/// The event line of the sample message the send tests post. It is ASCII, so it can be found
/// in the raw data as it is.
const SAMPLE_LINE: &str = "Price drop: 10.00 EUR to 9.00 EUR";

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

/// The event lines of a message body: the lines before the blank line that precedes the URL.
fn event_lines(message: &Message) -> Vec<&str> {
    message
        .body
        .split("\n\n")
        .next()
        .unwrap_or("")
        .lines()
        .collect()
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
fn single_price_drop_subject_is_phrase_and_watch_name() {
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.subject, "Price drop: Kettle");
}

#[test]
fn single_price_drop_body_shows_old_and_new_price() {
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(event_lines(&message), ["Price drop: 10.00 EUR → 9.00 EUR"]);
}

#[test]
fn body_is_event_lines_then_blank_line_then_watch_url() {
    let events = [
        event(EventKind::PriceDrop, Some(1000), Some(900)),
        event(EventKind::LowestEver, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert_eq!(
        message.body,
        "Price drop: 10.00 EUR → 9.00 EUR\nLowest price ever: 9.00 EUR\n\nhttps://shop.example/p/1"
    );
}

#[test]
fn body_ends_with_the_watch_url_as_its_last_line() {
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.body.lines().last(), Some(SHOP_URL));
}

#[test]
fn lowest_ever_leads_the_subject_over_price_drop() {
    let events = [
        event(EventKind::PriceDrop, Some(1000), Some(900)),
        event(EventKind::LowestEver, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.subject, "Lowest price ever: Kettle");
}

#[test]
fn body_has_one_line_per_event_in_input_order() {
    let events = [
        event(EventKind::PriceDrop, Some(1000), Some(900)),
        event(EventKind::LowestEver, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert_eq!(
        event_lines(&message),
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
        event_lines(&message),
        [
            "Lowest price ever: 9.00 EUR",
            "Price drop: 10.00 EUR → 9.00 EUR",
        ]
    );
}

#[test]
fn target_hit_leads_the_subject() {
    let events = [event(EventKind::TargetHit, None, Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.subject, "Target price reached: Kettle");
}

#[test]
fn target_hit_among_other_events_still_leads_the_subject() {
    let events = [
        event(EventKind::PriceDrop, Some(1000), Some(900)),
        event(EventKind::TargetHit, None, Some(900)),
    ];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.subject, "Target price reached: Kettle");
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
        assert_eq!(
            message.subject,
            format!("{phrase}: Kettle"),
            "cut at {start}"
        );
    }
}

#[test]
fn label_is_the_host_when_the_watch_has_no_name() {
    let shop = watch(SHOP_URL, None);
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&shop, &events);
    assert_eq!(message.subject, "Price drop: shop.example");
}

#[test]
fn label_is_the_url_when_the_watch_has_no_name_and_the_url_has_no_host() {
    let odd = watch("not a url", None);
    let events = [event(EventKind::PriceDrop, Some(1000), Some(900))];
    let message = message_for(&odd, &events);
    assert_eq!(message.subject, "Price drop: not a url");
}

#[test]
fn inflated_sale_body_names_new_price_and_claimed_old_price() {
    // 1200 minor units claimed was, 700 is what the item costs now.
    let events = [event(EventKind::InflatedSale, Some(1200), Some(700))];
    let message = message_for(&kettle(), &events);
    assert_eq!(
        event_lines(&message),
        ["Inflated sale: 7.00 EUR, claimed was 12.00 EUR"]
    );
}

#[test]
fn back_in_stock_with_only_new_price_shows_phrase_and_price() {
    let events = [event(EventKind::BackInStock, None, Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(event_lines(&message), ["Back in stock: 9.00 EUR"]);
}

#[test]
fn back_in_stock_subject_is_phrase_and_watch_name() {
    let events = [event(EventKind::BackInStock, None, Some(900))];
    let message = message_for(&kettle(), &events);
    assert_eq!(message.subject, "Back in stock: Kettle");
}

#[test]
fn price_rise_body_shows_old_and_new_price() {
    let events = [event(EventKind::PriceRise, Some(900), Some(1000))];
    let message = message_for(&kettle(), &events);
    assert_eq!(event_lines(&message), ["Price rise: 9.00 EUR → 10.00 EUR"]);
}

#[test]
fn event_without_prices_has_phrase_only_in_body() {
    let events = [event(EventKind::OutOfStock, None, None)];
    let message = message_for(&kettle(), &events);
    assert_eq!(event_lines(&message), ["Out of stock"]);
}

/// The message the send tests post when they do not care about its content. The body is the
/// sample event line, a blank line and the watch URL.
fn sample_message() -> Message {
    Message {
        subject: "Price drop: Kettle".to_string(),
        body: format!("{SAMPLE_LINE}\n\n{SHOP_URL}"),
    }
}

/// A notifier that sends through `server` as `FROM` to `TO`, with no TLS and no login.
fn notifier_for(server: &FakeSmtp) -> Notifier {
    Notifier::unencrypted("127.0.0.1", server.port(), FROM, TO).expect("notifier should build")
}

/// Sends the sample message through `server` and returns the one message it received.
async fn sent_to(server: &FakeSmtp) -> ReceivedMessage {
    notifier_for(server)
        .send(&sample_message())
        .await
        .expect("send should succeed");
    let mut messages = server.messages();
    assert_eq!(messages.len(), 1, "expected exactly one message");
    messages.remove(0)
}

/// The error of a send that must fail. Panics if the send succeeded.
fn send_error(result: Result<(), NotifyError>) -> NotifyError {
    match result {
        Ok(()) => panic!("expected the send to fail"),
        Err(error) => error,
    }
}

/// The error of a notifier that must not build. Panics if it did.
fn build_error(result: Result<Notifier, NotifyError>) -> NotifyError {
    match result {
        Ok(_) => panic!("expected the notifier not to build"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn send_delivers_exactly_one_message() {
    let server = FakeSmtp::start().await;
    sent_to(&server).await;
    assert_eq!(server.count(), 1);
}

#[tokio::test]
async fn send_envelope_sender_is_the_from_address() {
    let server = FakeSmtp::start().await;
    let received = sent_to(&server).await;
    assert_eq!(received.from, FROM);
}

#[tokio::test]
async fn send_envelope_recipient_is_the_to_address() {
    let server = FakeSmtp::start().await;
    let received = sent_to(&server).await;
    assert_eq!(received.recipients, [TO]);
}

#[tokio::test]
async fn send_data_has_a_subject_header_with_the_subject() {
    let server = FakeSmtp::start().await;
    let received = sent_to(&server).await;
    assert!(
        received
            .data
            .lines()
            .any(|line| line == "Subject: Price drop: Kettle"),
        "no Subject header in {:?}",
        received.data
    );
}

#[tokio::test]
async fn send_data_contains_the_event_line() {
    let server = FakeSmtp::start().await;
    let received = sent_to(&server).await;
    assert!(
        received.data.contains(SAMPLE_LINE),
        "event line missing from {:?}",
        received.data
    );
}

#[tokio::test]
async fn send_data_contains_the_watch_url() {
    let server = FakeSmtp::start().await;
    let received = sent_to(&server).await;
    assert!(
        received.data.contains(SHOP_URL),
        "watch URL missing from {:?}",
        received.data
    );
}

#[tokio::test]
async fn send_data_is_declared_as_plain_text() {
    let server = FakeSmtp::start().await;
    let received = sent_to(&server).await;
    assert!(
        received
            .data
            .lines()
            .any(|line| line.starts_with("Content-Type: text/plain")),
        "no text/plain Content-Type in {:?}",
        received.data
    );
}

#[tokio::test]
async fn send_fails_with_transport_error_when_the_server_refuses_the_data() {
    let server = FakeSmtp::start().await;
    server.answer_data_with(451);
    let error = send_error(notifier_for(&server).send(&sample_message()).await);
    assert!(
        matches!(error, NotifyError::Transport(_)),
        "expected a transport error, got: {error}"
    );
}

#[tokio::test]
async fn send_to_a_port_with_nothing_listening_is_a_transport_error() {
    // Bind a port, then release it, so nothing is listening when the send connects.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener.local_addr().expect("local address").port();
    drop(listener);
    let notifier =
        Notifier::unencrypted("127.0.0.1", port, FROM, TO).expect("notifier should build");
    let error = send_error(notifier.send(&sample_message()).await);
    assert!(
        matches!(error, NotifyError::Transport(_)),
        "expected a transport error, got: {error}"
    );
}

#[test]
fn unencrypted_rejects_a_from_address_that_is_not_an_address() {
    let error = build_error(Notifier::unencrypted(
        "127.0.0.1",
        SMTP_PORT,
        "not an address",
        TO,
    ));
    assert!(
        matches!(error, NotifyError::Address(_)),
        "expected an address error, got: {error}"
    );
}

#[test]
fn unencrypted_rejects_a_to_address_that_is_not_an_address() {
    let error = build_error(Notifier::unencrypted(
        "127.0.0.1",
        SMTP_PORT,
        FROM,
        "nobody",
    ));
    assert!(
        matches!(error, NotifyError::Address(_)),
        "expected an address error, got: {error}"
    );
}

#[test]
fn gmail_builds_for_valid_addresses() {
    assert!(Notifier::gmail(SMTP_HOST, SMTP_PORT, FROM, "app-pass", TO).is_ok());
}

#[test]
fn gmail_rejects_a_user_address_that_is_not_an_address() {
    let error = build_error(Notifier::gmail(
        SMTP_HOST, SMTP_PORT, "a b@c", "app-pass", TO,
    ));
    assert!(
        matches!(error, NotifyError::Address(_)),
        "expected an address error, got: {error}"
    );
}

#[test]
fn gmail_rejects_a_to_address_that_is_not_an_address() {
    let error = build_error(Notifier::gmail(
        SMTP_HOST,
        SMTP_PORT,
        FROM,
        "app-pass",
        "not-an-address",
    ));
    assert!(
        matches!(error, NotifyError::Address(_)),
        "expected an address error, got: {error}"
    );
}
