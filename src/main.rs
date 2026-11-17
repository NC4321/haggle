//! The haggle binary: parses arguments, sets up logging and reports errors.

use std::process::ExitCode;

use clap::Parser;
use haggle::cli::{self, Cli, CliError};
use tracing_subscriber::EnvFilter;

/// The exit code for configuration errors.
const CONFIG_EXIT_CODE: u8 = 2;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::new(&cli.log_level))
        .init();
    match cli::run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            match error {
                CliError::Config(_) => ExitCode::from(CONFIG_EXIT_CODE),
                _ => ExitCode::from(1),
            }
        }
    }
}
