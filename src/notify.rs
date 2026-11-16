//! ntfy client: turns the events of a check into a push notification and posts it.

use std::time::Duration;

use crate::db::{Event, EventKind, Watch};
use crate::fetch::USER_AGENT;

/// Request timeout for posting to ntfy.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// The `Priority` header value that makes ntfy alert loudly.
const HIGH_PRIORITY: &str = "4";

/// The order in which event kinds pick the lead event of a message.
const LEAD_ORDER: [EventKind; 7] = [
    EventKind::TargetHit,
    EventKind::LowestEver,
    EventKind::InflatedSale,
    EventKind::PriceDrop,
    EventKind::BackInStock,
    EventKind::OutOfStock,
    EventKind::PriceRise,
];

/// The base64 alphabet of RFC 4648, used for RFC 2047 encoded words.
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Bits per base64 character.
const BASE64_CHAR_BITS: u32 = 6;

/// Mask selecting one base64 character's worth of bits.
const BASE64_CHAR_MASK: u32 = 0x3F;

/// Bytes per base64 group.
const BASE64_GROUP_BYTES: usize = 3;

/// Characters per base64 group, padding included.
const BASE64_GROUP_CHARS: usize = 4;

/// Why a notification could not be delivered.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NotifyError {
    #[error("network error: {0}")]
    Network(String),
    #[error("ntfy answered HTTP {0}")]
    Status(u16),
}

/// A notification ready to be sent to ntfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Headline shown by the device.
    pub title: String,
    /// Text of the notification, one line per event.
    pub body: String,
    /// URL opened when the notification is tapped.
    pub click: String,
    /// Whether the notification should alert loudly.
    pub high_priority: bool,
}

/// Posts messages to one ntfy topic, optionally with a bearer token.
pub struct Notifier {
    client: reqwest::Client,
    url: url::Url,
    token: Option<String>,
}

impl Notifier {
    /// Builds a notifier for the topic at `url`, sending `token` as a bearer token when set.
    ///
    /// Returns `NotifyError::Network` when the HTTP client cannot be built.
    pub fn new(url: url::Url, token: Option<String>) -> Result<Notifier, NotifyError> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .user_agent(USER_AGENT)
            .timeout(NOTIFY_TIMEOUT)
            .build()
            .map_err(network_error)?;
        Ok(Notifier { client, url, token })
    }

    /// Posts `message` to the topic.
    ///
    /// Returns `NotifyError::Network` when the request fails, and
    /// `NotifyError::Status` with the code when ntfy answers with a non-2xx status.
    pub async fn send(&self, message: &Message) -> Result<(), NotifyError> {
        let mut request = self
            .client
            .post(self.url.clone())
            .header("Title", encode_header_value(&message.title))
            .header("Click", message.click.as_str())
            .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(message.body.clone());
        if message.high_priority {
            request = request.header("Priority", HIGH_PRIORITY);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(network_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(NotifyError::Status(status.as_u16()))
        }
    }
}

/// Builds the one message describing the events of one check of `watch`.
///
/// Panics when `events` is empty; callers only build a message for a check that produced events.
pub fn message_for(watch: &Watch, events: &[Event]) -> Message {
    let lead = LEAD_ORDER
        .iter()
        .find_map(|kind| events.iter().find(|event| event.kind == *kind))
        .expect("message_for needs at least one event");
    let title = format!("{}: {}", phrase(lead.kind), watch_label(watch));
    let body = events.iter().map(event_line).collect::<Vec<_>>().join("\n");
    Message {
        title,
        body,
        click: watch.url.clone(),
        high_priority: events
            .iter()
            .any(|event| event.kind == EventKind::TargetHit),
    }
}

/// Returns the name shown for a watch: its name, else its URL's host, else the URL itself.
fn watch_label(watch: &Watch) -> String {
    if let Some(name) = &watch.name {
        return name.clone();
    }
    url::Url::parse(&watch.url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| watch.url.clone())
}

/// Returns the headline phrase of an event kind.
fn phrase(kind: EventKind) -> &'static str {
    match kind {
        EventKind::TargetHit => "Target price reached",
        EventKind::LowestEver => "Lowest price ever",
        EventKind::InflatedSale => "Inflated sale",
        EventKind::PriceDrop => "Price drop",
        EventKind::BackInStock => "Back in stock",
        EventKind::OutOfStock => "Out of stock",
        EventKind::PriceRise => "Price rise",
    }
}

/// Returns one body line describing `event`.
fn event_line(event: &Event) -> String {
    match (event.kind, event.old, event.new) {
        (EventKind::InflatedSale, Some(old), Some(new)) => {
            format!("Inflated sale: {new}, claimed was {old}")
        }
        (kind, Some(old), Some(new)) => format!("{}: {old} → {new}", phrase(kind)),
        (kind, None, Some(new)) => format!("{}: {new}", phrase(kind)),
        (kind, _, _) => phrase(kind).to_string(),
    }
}

/// Returns `text` as a header value: unchanged when ASCII, else an RFC 2047 UTF-8 base64 encoded word.
fn encode_header_value(text: &str) -> String {
    if text.is_ascii() {
        return text.to_string();
    }
    format!("=?UTF-8?B?{}?=", base64_encode(text.as_bytes()))
}

/// Encodes `bytes` as standard base64 with `=` padding.
fn base64_encode(bytes: &[u8]) -> String {
    let mut encoded = String::new();
    for group in bytes.chunks(BASE64_GROUP_BYTES) {
        let mut triple: u32 = 0;
        for index in 0..BASE64_GROUP_BYTES {
            triple = (triple << 8) | u32::from(group.get(index).copied().unwrap_or(0));
        }
        for index in 0..BASE64_GROUP_CHARS {
            if index <= group.len() {
                let shift = BASE64_CHAR_BITS * (BASE64_GROUP_CHARS - 1 - index) as u32;
                let digit = ((triple >> shift) & BASE64_CHAR_MASK) as usize;
                encoded.push(char::from(BASE64_ALPHABET[digit]));
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

/// Maps a reqwest error to `NotifyError::Network`.
fn network_error(error: reqwest::Error) -> NotifyError {
    NotifyError::Network(error.to_string())
}
