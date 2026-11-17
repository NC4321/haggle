//! Email client: turns the events of a check into an email and sends it over SMTP.

use std::time::Duration;

use lettre::address::AddressError;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message as LettreMessage};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::db::{Event, EventKind, Watch};

/// Timeout for one SMTP exchange with the mail server.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Why a notification could not be delivered.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NotifyError {
    #[error("bad email address: {0}")]
    Address(String),
    #[error("SMTP error: {0}")]
    Transport(String),
}

/// An email ready to be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Subject line of the email.
    pub subject: String,
    /// Plain text of the email: one line per event, then the watch URL.
    pub body: String,
}

/// Sends plain-text emails from one address to another over SMTP.
pub struct Notifier {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    to: Mailbox,
}

impl Notifier {
    /// Builds a notifier that sends through the SMTP server `host`:`port`, upgrading
    /// the connection with STARTTLS and logging in as `user` with `password`.
    ///
    /// The emails are sent from `user` to `to`.
    /// Returns `NotifyError::Address` when `user` or `to` is not a mailbox, and
    /// `NotifyError::Transport` when the SMTP transport cannot be set up for `host`.
    pub fn gmail(
        host: &str,
        port: u16,
        user: &str,
        password: &str,
        to: &str,
    ) -> Result<Notifier, NotifyError> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
            .map_err(transport_error)?
            .port(port)
            .credentials(Credentials::new(user.to_string(), password.to_string()))
            .timeout(Some(NOTIFY_TIMEOUT))
            .build();
        Notifier::with_transport(transport, user, to)
    }

    /// Builds a notifier that sends from `from` to `to` through `host`:`port` with no
    /// encryption and no login.
    ///
    /// Only for a local relay or a test server: nothing protects the traffic.
    /// Returns `NotifyError::Address` when `from` or `to` is not a mailbox.
    pub fn unencrypted(
        host: &str,
        port: u16,
        from: &str,
        to: &str,
    ) -> Result<Notifier, NotifyError> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(port)
            .timeout(Some(NOTIFY_TIMEOUT))
            .build();
        Notifier::with_transport(transport, from, to)
    }

    /// Sends `message` as one plain-text UTF-8 email.
    ///
    /// Returns `NotifyError::Transport` when the email cannot be built or the SMTP exchange fails.
    pub async fn send(&self, message: &Message) -> Result<(), NotifyError> {
        let email = LettreMessage::builder()
            .from(self.from.clone())
            .to(self.to.clone())
            .subject(message.subject.as_str())
            .header(ContentType::TEXT_PLAIN)
            .body(message.body.clone())
            .map_err(transport_error)?;
        self.transport.send(email).await.map_err(transport_error)?;
        Ok(())
    }

    /// Pairs `transport` with the parsed `from` and `to` mailboxes.
    fn with_transport(
        transport: AsyncSmtpTransport<Tokio1Executor>,
        from: &str,
        to: &str,
    ) -> Result<Notifier, NotifyError> {
        Ok(Notifier {
            transport,
            from: parse_mailbox(from)?,
            to: parse_mailbox(to)?,
        })
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
    let subject = format!("{}: {}", phrase(lead.kind), watch_label(watch));
    let lines = events.iter().map(event_line).collect::<Vec<_>>().join("\n");
    let body = format!("{lines}\n\n{}", watch.url);
    Message { subject, body }
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

/// Parses `text` as a mailbox, failing with `NotifyError::Address` when it is not one.
fn parse_mailbox(text: &str) -> Result<Mailbox, NotifyError> {
    text.parse()
        .map_err(|error: AddressError| NotifyError::Address(format!("{text}: {error}")))
}

/// Maps any lettre error to `NotifyError::Transport`.
fn transport_error(error: impl std::fmt::Display) -> NotifyError {
    NotifyError::Transport(error.to_string())
}
