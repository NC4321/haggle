//! The command-line interface: argument definitions and the command implementations.

use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use time::OffsetDateTime;
use time::macros::format_description;
use tokio::net::TcpListener;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::config::{self, Config, ConfigError};
use crate::db::watches;
use crate::db::{self, DbError, NewObservation, NewWatch, observations};
use crate::demo_shop::{self, DemoClock, Product};
use crate::extract::{self, ExtractError, Source};
use crate::fetch::{
    DEFAULT_MAX_BYTES, DEFAULT_TIMEOUT, FetchError, Fetched, Fetcher, Politeness, fetch,
};
use crate::money::{self, Currency, Money, MoneyError};
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
    /// Run a fake shop with a scripted timeline and show it in the web UI, with time sped up.
    Demo,
}

/// The check interval used when `--every` is not given.
const DEFAULT_INTERVAL: &str = "6h";

/// The shortest check interval allowed, in seconds.
const MIN_INTERVAL_SECONDS: i64 = 5 * 60;

/// Demo seconds that pass per real second.
const DEMO_SPEED: i64 = 60;

/// The check interval of demo watches, in seconds.
const DEMO_INTERVAL_SECONDS: i64 = 300;

/// The kettle's target price in the demo, in euro cents.
const DEMO_KETTLE_TARGET_MINOR: i64 = 4000;

/// The slug of the demo product that gets a target price.
const DEMO_TARGET_SLUG: &str = "kettle";

/// Demo seconds between the seeded history observations.
const DEMO_HISTORY_STEP_SECONDS: i64 = 6 * 3600;

/// How far back the seeded demo history starts, in demo seconds.
const DEMO_HISTORY_SPAN_SECONDS: i64 = 30 * 86400;

/// The newest seeded demo observation, in demo seconds before now.
const DEMO_HISTORY_END_SECONDS: i64 = 3600;

/// Real time between scheduler ticks in the demo.
const DEMO_TICK: Duration = Duration::from_secs(1);

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
        Command::Demo => demo().await,
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
    let notifier = match &config.email {
        Some(email) => Some(Notifier::gmail(
            config::SMTP_HOST,
            config::SMTP_PORT,
            &email.user,
            &email.password,
            &email.to,
        )?),
        None => {
            tracing::warn!(
                "notifications are off: GMAIL_USER, GMAIL_APP_PASSWORD and ALERT_EMAIL are not set"
            );
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
        email = notifier.is_some(),
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

/// Removes the demo database and its `-wal` and `-shm` files when dropped.
struct DemoDatabaseFiles(PathBuf);

impl Drop for DemoDatabaseFiles {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.clone().into_os_string();
            path.push(suffix);
            // The file may never have been created, so a missing file is fine.
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Runs a fake shop and the web interface on a temporary database, with sped-up time,
/// until Ctrl-C or SIGTERM.
async fn demo() -> Result<(), CliError> {
    // Registered before any other work so an early signal is not lost to the default handler.
    let mut interrupt = register_signal(SignalKind::interrupt())?;
    let mut terminate = register_signal(SignalKind::terminate())?;
    let db_path = std::env::temp_dir().join(format!("haggle-demo-{}.db", std::process::id()));
    let _cleanup = DemoDatabaseFiles(db_path.clone());
    let config = demo_config(&db_path)?;
    println!("database: {}", db_path.display());
    println!("dashboard: http://{}/", config.listen);
    let connection = db::open(&config.db_path)?;
    let clock = Arc::new(DemoClock::new(now_seconds(), DEMO_SPEED));
    let shop_base = start_demo_shop(Arc::clone(&clock)).await?;
    seed_demo(&connection, &shop_base, clock.now())?;
    let fetcher = Arc::new(
        Fetcher::new(DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES)?.with_politeness(Politeness {
            host_interval: Duration::ZERO,
            ..Default::default()
        }),
    );
    let (stop_web, web_server) = start_web(&config, Arc::clone(&fetcher)).await?;
    let shutdown = async {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    };
    let ticked = tick_until(shutdown, &connection, &fetcher, &clock).await;
    // The send fails only when the server has already stopped; join_web reports that exit.
    let _ = stop_web.send(());
    let served = join_web(web_server).await;
    ticked?;
    served
}

/// Builds the demo configuration: the temporary database and `HAGGLE_LISTEN` from the environment.
fn demo_config(db_path: &std::path::Path) -> Result<Config, CliError> {
    Ok(Config::from_vars(|name| match name {
        "HAGGLE_DB" => Some(db_path.display().to_string()),
        other => std::env::var(other).ok(),
    })?)
}

/// Binds the fake shop on a free local port and serves it in a background task.
///
/// Returns the shop's base URL, or `CliError::Io` when it cannot be bound.
async fn start_demo_shop(clock: Arc<DemoClock>) -> Result<url::Url, CliError> {
    let io_error =
        |error: std::io::Error| CliError::Io(format!("cannot start the demo shop: {error}"));
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(io_error)?;
    let address = listener.local_addr().map_err(io_error)?;
    tokio::spawn(axum::serve(listener, demo_shop::router(clock)).into_future());
    url::Url::parse(&format!("http://{address}/"))
        .map_err(|error| CliError::InvalidUrl(address.to_string(), error.to_string()))
}

/// Inserts a watch for every demo product and seeds its price history.
fn seed_demo(
    connection: &rusqlite::Connection,
    shop_base: &url::Url,
    now: i64,
) -> Result<(), CliError> {
    for product in demo_shop::PRODUCTS {
        let target = (product.slug == DEMO_TARGET_SLUG).then_some(Money {
            minor: DEMO_KETTLE_TARGET_MINOR,
            currency: Currency::Eur,
        });
        let new = NewWatch {
            url: demo_shop::url_for(shop_base, product).to_string(),
            name: Some(product.name.to_string()),
            selector: None,
            target,
            interval_s: DEMO_INTERVAL_SECONDS,
        };
        let watch_id = watches::insert(connection, &new, now)?;
        seed_history(connection, watch_id, product, now)?;
    }
    Ok(())
}

/// Records one observation of `product` every `DEMO_HISTORY_STEP_SECONDS` of demo time
/// before `now`, following the product's scripted timeline.
fn seed_history(
    connection: &rusqlite::Connection,
    watch_id: i64,
    product: &Product,
    now: i64,
) -> Result<(), DbError> {
    let mut offset = -DEMO_HISTORY_SPAN_SECONDS;
    while offset <= -DEMO_HISTORY_END_SECONDS {
        let step = demo_shop::state_at(product, offset);
        observations::insert(
            connection,
            &NewObservation {
                watch_id,
                at: now + offset,
                price: Money {
                    minor: step.price_minor,
                    currency: product.currency,
                },
                list_price: step.list_price_minor.map(|minor| Money {
                    minor,
                    currency: product.currency,
                }),
                availability: step.availability,
                source: Source::JsonLd,
            },
        )?;
        offset += DEMO_HISTORY_STEP_SECONDS;
    }
    Ok(())
}

/// Runs a scheduler tick at the demo time every `DEMO_TICK` until `shutdown` completes.
async fn tick_until(
    shutdown: impl std::future::Future<Output = ()>,
    connection: &rusqlite::Connection,
    fetcher: &Fetcher,
    clock: &DemoClock,
) -> Result<(), CliError> {
    tokio::pin!(shutdown);
    loop {
        scheduler::tick(connection, fetcher, None, clock.now()).await?;
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            () = tokio::time::sleep(DEMO_TICK) => {}
        }
    }
}
