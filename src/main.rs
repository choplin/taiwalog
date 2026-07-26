use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

use agentlog::{
    app::{paths_report, scan_shell},
    paths::AppPaths,
};

/// Browse local AI coding-agent history without changing provider-owned files.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Agentlog-owned home directory. Intended for tests and isolated installs.
    #[arg(long, global = true, env = "AGENTLOG_HOME", value_name = "DIR")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show Agentlog-owned paths and the separate provider-source boundary.
    Paths {
        /// Emit a machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Refresh Codex history without changing provider-owned files.
    Scan {
        /// Emit a machine-readable result.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::resolve(cli.home).context("resolve Agentlog-owned paths")?;

    match cli.command.unwrap_or(Command::Scan { json: false }) {
        Command::Paths { json } => print_paths(&paths, json),
        Command::Scan { json } => print_scan(&paths, json).await,
    }
}

fn print_paths(paths: &AppPaths, json: bool) -> anyhow::Result<()> {
    let report = paths_report(paths);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Agentlog-owned home: {}", report.agentlog_home.display());
        println!("Agentlog-owned config: {}", report.config_file.display());
        println!("Agentlog-owned data: {}", report.data_directory.display());
        println!("Agentlog-owned database: {}", report.database.display());
        println!("Provider-owned sources: {}", report.provider_sources);
    }
    Ok(())
}

async fn print_scan(paths: &AppPaths, json: bool) -> anyhow::Result<()> {
    let summary = scan_shell(paths).await?;
    if json {
        println!("{}", serde_json::to_string(&summary)?);
    } else {
        println!("Schema version: {}", summary.schema_version);
        println!("Collectors installed: {}", summary.collectors_installed);
        println!("Sources refreshed: {}", summary.sources_refreshed);
        println!("Sources partial: {}", summary.sources_partial);
        println!("Sources failed: {}", summary.sources_failed);
        println!("Sessions available: {}", summary.sessions_available);
        println!("Scan result: {}", summary.message);
    }
    Ok(())
}
