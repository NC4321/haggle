//! The command-line interface: argument definitions and the `check` command.

use clap::{Parser, Subcommand};

use crate::extract::{self, ExtractError};
use crate::fetch::{DEFAULT_MAX_BYTES, DEFAULT_TIMEOUT, FetchError, Fetched, Fetcher, fetch};

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
}

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
}

/// Runs the command in `cli`.
///
/// Returns `CliError` when the URL is invalid, the fetch fails, or no price can be extracted.
pub async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Check { url, selector } => check(&url, selector.as_deref()).await,
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
