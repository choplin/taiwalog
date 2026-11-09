use std::{path::PathBuf, process::ExitCode};

use taiwalog::full_scan::{Config, render_report, run};
use clap::Parser;

/// Measure one sequential, read-only pass over the six MVP provider roots.
///
/// The report contains counts, sizes, elapsed time, and source immutability.
/// The five-run wrapper records peak RSS without exposing transcript contents.
#[derive(Debug, Parser)]
#[command(name = "full-scan-feasibility")]
struct Args {
    /// Override the home directory used for conventional provider locations.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Override the conventional Codex home.
    #[arg(long, value_name = "PATH")]
    codex_home: Option<PathBuf>,

    /// Override the conventional Kimi Code home.
    #[arg(long, value_name = "PATH")]
    kimi_home: Option<PathBuf>,

    /// Override the conventional provider data home.
    #[arg(long, value_name = "PATH")]
    xdg_data_home: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let config = Config {
        home: args.home,
        codex_home: args.codex_home,
        kimi_home: args.kimi_home,
        xdg_data_home: args.xdg_data_home,
    };

    match run(&config) {
        Ok(report) => {
            print!("{}", render_report(&report));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("measurement failed: {error}");
            ExitCode::FAILURE
        }
    }
}
