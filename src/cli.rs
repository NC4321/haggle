//! The command-line interface: argument definitions and the command implementations.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use time::OffsetDateTime;
use time::macros::format_description;
use tokio::net::TcpListener;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::config::{Config, ConfigError};
use crate::db::watches;
use crate::db::{self, DbError, NewWatch, observations};
use crate::extract::{self, ExtractError};
use crate::fetch::{DEFAULT_MAX_BYTES, DEFAULT_TIMEOUT, FetchError, Fetched, Fetcher, fetch};
use crate::money::{self, MoneyError};
use crate::notify::{Notifier, NotifyError};
use crate::scheduler;
use crate::web;

/// Command-line arguments of the haggle binary.
#[derive(Debug, Parser)]
#[command(version, about = "Watches product pages and tracks their prices")]
pub struct Cli {
    /// Log filter directive, for example `debug` or `haggle=trace`.
    #[arg(long, global = true, default_value = "warn")]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands of haggle.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Fetch a page once and print what was found, storing nothing.
    Check {
        /// The product page URL (http or https).
        url: String,
        /// CSS selector for the price, tried when no structured data is found.
        #[arg(long)]
        selector: Option<String>,
    },
    /// Start watching a product page.
    Add {
        /// The product page URL (http or https).
        url: String,
        /// A display name for the watch.
        #[arg(long)]
        name: Option<String>,
        /// Price to alert at, with its currency, for example `199.99 EUR`.
        #[arg(long)]
        target: Option<String>,
        /// CSS selector for the price, tried when no structured data is found.
        #[arg(long)]
        selector: Option<String>,
        /// Time between checks: a number and one of s, m, h, d.
        #[arg(long, default_value = DEFAULT_INTERVAL)]
        every: String,
    },
    /// List all watches.
    Ls,
    /// Show the recorded prices of a watch, oldest first.
    History {
        /// The watch id.
        id: i64,
    },
    /// Stop watching and delete the stored history.
    Rm {
        /// The watch id.
        id: i64,
    },
    /// Run the scheduler until interrupted (Ctrl-C or SIGTERM).
    Serve,
}

/// The check interval used when `--every` is not given.
const DEFAULT_INTERVAL: &str = "6h";

/// The shortest check interval allowed, in seconds.
const MIN_INTERVAL_SECONDS: i64 = 5 * 60;

/// The format of times printed by `history`.
const TIME_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
    format_description!("[year]-[month]-[day] [hour]:[minute] UTC");

/// Why a command failed.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("invalid URL {0}: {1}")]
    InvalidUrl(String, String),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error("the server answered 304 Not Modified to an unconditional request")]
    UnexpectedNotModified,
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("invalid target price {0}: {1}")]
    InvalidTarget(String, String),
    #[error("invalid interval {0}: use a number and s, m, h or d, at least 5m")]
    InvalidInterval(String),
    #[error("no watch with id {0}")]
    NoSuchWatch(i64),
    #[error(transparent)]
    Notify(#[from] NotifyError),
    #[error("{0}")]
    Io(String),
}

/// Runs the command in `cli`.
///
/// Returns `CliError` when an argument is invalid, the configuration or database is
/// unusable, a watch id is unknown, the fetch fails, or no price can be extracted.
pub async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Check { url, selector } => check(&url, selector.as_deref()).await,
        Command::Add {
            url,
            name,
            target,
            selector,
            every,
        } => add(&url, name, target.as_deref(), selector, &every),
        Command::Ls => list(),
        Command::History { id } => history(id),
        Command::Rm { id } => remove(id),
        Command::Serve => serve().await,
    }
}

fn parse_url(url: &str) -> Result<url::Url, CliError> {
    let parsed = url::Url::parse(url)
        .map_err(|error| CliError::InvalidUrl(url.to_string(), error.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        scheme => Err(CliError::InvalidUrl(
            url.to_string(),
            format!("unsupported scheme {scheme}"),
        )),
    }
}

async fn check(url: &str, selector: Option<&str>) -> Result<(), CliError> {
    let url = parse_url(url)?;
    let fetcher = Fetcher::new(DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES)?;
    let html = match fetch(&fetcher, &url, None).await? {
        Fetched::Body { html, .. } => html,
        Fetched::NotModified => return Err(CliError::UnexpectedNotModified),
    };
    let found = extract::extract(&html, selector)?;
    println!("price: {}", found.price);
    if let Some(list_price) = &found.list_price {
        println!("list price: {list_price}");
    }
    println!("availability: {}", found.availability);
    if let Some(title) = &found.title {
        println!("title: {title}");
    }
    println!("source: {}", found.source);
    Ok(())
}

fn open_database() -> Result<rusqlite::Connection, CliError> {
    let config = Config::from_env()?;
    Ok(db::open(&config.db_path)?)
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Parses `<n>s|m|h|d` into seconds, requiring at least `MIN_INTERVAL_SECONDS`.
fn parse_interval(text: &str) -> Result<i64, CliError> {
    let invalid = || CliError::InvalidInterval(text.to_string());
    let unit_start = text.len().saturating_sub(1);
    let (count, unit) = text.split_at_checked(unit_start).ok_or_else(invalid)?;
    let unit_seconds: i64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(invalid()),
    };
    let count: i64 = count.parse().map_err(|_| invalid())?;
    let seconds = count.checked_mul(unit_seconds).ok_or_else(invalid)?;
    if seconds < MIN_INTERVAL_SECONDS {
        return Err(invalid());
    }
    Ok(seconds)
}

fn parse_target(text: &str) -> Result<money::Money, CliError> {
    money::parse_price(text, None).map_err(|error| {
        let reason = match error {
            MoneyError::NoCurrency => "name the currency, for example 199.99 EUR".to_string(),
            other => other.to_string(),
        };
        CliError::InvalidTarget(text.to_string(), reason)
    })
}

fn format_time(seconds: i64) -> Result<String, DbError> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|moment| moment.format(TIME_FORMAT).ok())
        .ok_or_else(|| DbError::Corrupt(format!("timestamp {seconds} is out of range")))
}

fn add(
    url: &str,
    name: Option<String>,
    target: Option<&str>,
    selector: Option<String>,
    every: &str,
) -> Result<(), CliError> {
    let url = parse_url(url)?;
    let target = target.map(parse_target).transpose()?;
    let interval_s = parse_interval(every)?;
    let connection = open_database()?;
    let new = NewWatch {
        url: url.to_string(),
        name,
        selector,
        target,
        interval_s,
    };
    let id = watches::insert(&connection, &new, now_seconds())?;
    println!("added watch {id}");
    Ok(())
}

fn list() -> Result<(), CliError> {
    let connection = open_database()?;
    let all = watches::list(&connection)?;
    if all.is_empty() {
        println!("no watches");
    }
    for watch in all {
        let price = observations::latest(&connection, watch.id)?.map_or_else(
            || "-".to_string(),
            |observation| observation.price.to_string(),
        );
        let label = watch.name.as_deref().unwrap_or(&watch.url);
        println!(
            "{}  {}  {}  {}",
            watch.id,
            watch.status.as_str(),
            price,
            label
        );
    }
    Ok(())
}

fn history(id: i64) -> Result<(), CliError> {
    let connection = open_database()?;
    watches::get(&connection, id)?.ok_or(CliError::NoSuchWatch(id))?;
    for observation in observations::history(&connection, id)? {
        let mut line = format!(
            "{}  {}  {}",
            format_time(observation.at)?,
            observation.price,
            observation.availability
        );
        if let Some(list_price) = observation.list_price {
            line.push_str(&format!("  (list {list_price})"));
        }
        println!("{line}");
    }
    Ok(())
}

fn remove(id: i64) -> Result<(), CliError> {
    let connection = open_database()?;
    if !watches::delete(&connection, id)? {
        return Err(CliError::NoSuchWatch(id));
    }
    println!("removed watch {id}");
    Ok(())
}

/// Runs the scheduler and the web interface until Ctrl-C or SIGTERM.
async fn serve() -> Result<(), CliError> {
    // Registered before any other work so an early signal is not lost to the default handler.
    let mut interrupt = register_signal(SignalKind::interrupt())?;
    let mut terminate = register_signal(SignalKind::terminate())?;
    let config = Config::from_env()?;
    let connection = db::open(&config.db_path)?;
    let fetcher = Arc::new(Fetcher::new(DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES)?);
    let notifier = match &config.ntfy {
        Some(ntfy) => Some(Notifier::new(ntfy.url.clone(), ntfy.token.clone())?),
        None => {
            tracing::warn!("notifications are off: HAGGLE_NTFY_URL is not set");
            None
        }
    };
    let (stop_web, web_server) = start_web(&config, Arc::clone(&fetcher)).await?;
    let shutdown = async {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    };
    tracing::info!(
        db_path = %config.db_path.display(),
        listen = %config.listen,
        ntfy = notifier.is_some(),
        "serving"
    );
    let scheduled = scheduler::run(&connection, &fetcher, notifier.as_ref(), shutdown).await;
    // The send fails only when the server has already stopped; join_web reports that exit.
    let _ = stop_web.send(());
    let served = join_web(web_server).await;
    scheduled?;
    served?;
    tracing::info!("shutting down");
    Ok(())
}

/// Opens a second database connection and starts the web interface on `config.listen`
/// in a background task.
///
/// Returns the sender that stops the server and the handle of its task. Returns
/// `CliError::Db` when the database cannot be opened, and `CliError::Io` when the
/// address cannot be bound.
async fn start_web(
    config: &Config,
    fetcher: Arc<Fetcher>,
) -> Result<(oneshot::Sender<()>, JoinHandle<std::io::Result<()>>), CliError> {
    let connection = db::open(&config.db_path)?;
    let state = web::AppState {
        db: Arc::new(Mutex::new(connection)),
        fetcher,
    };
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|error| CliError::Io(format!("cannot listen on {}: {error}", config.listen)))?;
    let (stop_sender, stop_receiver) = oneshot::channel::<()>();
    let server = tokio::spawn(web::serve(listener, state, async {
        // A dropped sender also ends the wait, so the server stops either way.
        let _ = stop_receiver.await;
    }));
    Ok((stop_sender, server))
}

/// Waits for the web server task to finish, returning its I/O error or a panic as `CliError::Io`.
async fn join_web(server: JoinHandle<std::io::Result<()>>) -> Result<(), CliError> {
    server
        .await
        .map_err(|error| CliError::Io(format!("web server task failed: {error}")))?
        .map_err(|error| CliError::Io(format!("web server failed: {error}")))
}

fn register_signal(kind: SignalKind) -> Result<Signal, CliError> {
    signal(kind)
        .map_err(|error| CliError::Io(format!("cannot listen for shutdown signals: {error}")))
}
