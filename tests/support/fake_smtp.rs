//! A minimal fake SMTP server for tests, on 127.0.0.1.
//!
//! It speaks plain SMTP without STARTTLS or AUTH, so a client can send to it over an unencrypted
//! transport. Each message it accepts is recorded with its envelope sender, its envelope
//! recipients and its raw DATA text. Test files include it with
//! `#[path = "support/fake_smtp.rs"] mod fake_smtp;`.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The reply code that accepts a message at the end of DATA.
const ACCEPT: u16 = 250;

/// One message the server accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedMessage {
    /// The envelope sender, from `MAIL FROM`, without the angle brackets.
    pub from: String,
    /// The envelope recipients, from each `RCPT TO`, without the angle brackets.
    pub recipients: Vec<String>,
    /// The raw DATA text, with dot-stuffing undone and each line ending in CRLF.
    pub data: String,
}

/// The envelope of the transaction in progress on one connection.
#[derive(Default)]
struct Transaction {
    from: Option<String>,
    recipients: Vec<String>,
}

/// State shared by the accept loop, every connection and the test.
struct Shared {
    messages: Vec<ReceivedMessage>,
    /// The reply code given at the end of DATA. Only `250` accepts and records the message.
    data_reply: u16,
}

/// A running fake SMTP server. It stops accepting connections when dropped.
pub struct FakeSmtp {
    port: u16,
    shared: Arc<Mutex<Shared>>,
    accept_loop: JoinHandle<()>,
}

impl FakeSmtp {
    /// Binds a free port on 127.0.0.1 and starts accepting connections.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let port = listener.local_addr().expect("local address").port();
        let shared = Arc::new(Mutex::new(Shared {
            messages: Vec::new(),
            data_reply: ACCEPT,
        }));
        let state = Arc::clone(&shared);
        let accept_loop = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(session(socket, Arc::clone(&state)));
            }
        });
        Self {
            port,
            shared,
            accept_loop,
        }
    }

    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Makes the end of DATA answer with `code` instead of 250. Any code other than 250 refuses
    /// the message, so the message is not recorded.
    pub fn answer_data_with(&self, code: u16) {
        self.shared.lock().expect("fake server state").data_reply = code;
    }

    /// A snapshot of the messages accepted so far, in arrival order.
    pub fn messages(&self) -> Vec<ReceivedMessage> {
        self.shared
            .lock()
            .expect("fake server state")
            .messages
            .clone()
    }

    /// The number of messages accepted so far.
    pub fn count(&self) -> usize {
        self.shared
            .lock()
            .expect("fake server state")
            .messages
            .len()
    }
}

impl Drop for FakeSmtp {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

/// Writes one reply line, ending it with CRLF.
async fn say(writer: &mut OwnedWriteHalf, line: &str) -> std::io::Result<()> {
    writer.write_all(format!("{line}\r\n").as_bytes()).await
}

/// The address inside the angle brackets of a `MAIL FROM` or `RCPT TO` line. Empty if there are
/// no brackets.
fn address_in(line: &str) -> String {
    line.split_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map_or_else(String::new, |(address, _)| address.to_string())
}

/// Runs one SMTP conversation. The client may run several transactions before it quits.
async fn session(socket: TcpStream, shared: Arc<Mutex<Shared>>) -> std::io::Result<()> {
    let (read_half, mut writer) = socket.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let mut transaction = Transaction::default();
    say(&mut writer, "220 fake.smtp ready").await?;
    while let Some(line) = lines.next_line().await? {
        let command = line.to_ascii_uppercase();
        if command.starts_with("EHLO") || command.starts_with("HELO") {
            say(&mut writer, "250 fake.smtp").await?;
        } else if command.starts_with("MAIL FROM:") {
            transaction = Transaction {
                from: Some(address_in(&line)),
                recipients: Vec::new(),
            };
            say(&mut writer, "250 sender OK").await?;
        } else if command.starts_with("RCPT TO:") {
            transaction.recipients.push(address_in(&line));
            say(&mut writer, "250 recipient OK").await?;
        } else if command == "DATA" {
            if transaction.from.is_none() || transaction.recipients.is_empty() {
                say(&mut writer, "503 MAIL and RCPT come first").await?;
                continue;
            }
            say(&mut writer, "354 End data with <CR><LF>.<CR><LF>").await?;
            let mut data = String::new();
            loop {
                let Some(text) = lines.next_line().await? else {
                    return Ok(());
                };
                if text == "." {
                    break;
                }
                let unstuffed = if text.starts_with("..") {
                    &text[1..]
                } else {
                    text.as_str()
                };
                data.push_str(unstuffed);
                data.push_str("\r\n");
            }
            let finished = std::mem::take(&mut transaction);
            let code = shared.lock().expect("fake server state").data_reply;
            if code == ACCEPT {
                shared
                    .lock()
                    .expect("fake server state")
                    .messages
                    .push(ReceivedMessage {
                        from: finished.from.unwrap_or_default(),
                        recipients: finished.recipients,
                        data,
                    });
                say(&mut writer, "250 queued").await?;
            } else {
                say(&mut writer, &format!("{code} requested action aborted")).await?;
            }
        } else if command == "RSET" {
            transaction = Transaction::default();
            say(&mut writer, "250 reset").await?;
        } else if command == "NOOP" {
            say(&mut writer, "250 OK").await?;
        } else if command == "QUIT" {
            say(&mut writer, "221 bye").await?;
            return Ok(());
        } else {
            say(&mut writer, "500 command not recognised").await?;
        }
    }
    Ok(())
}
