//! Command-line transport for Agentlog application workflows.

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

use crate::{
    app::{list_shell, paths_report, scan_shell, show_shell},
    display::visible_text,
    paths::AppPaths,
    storage::CatalogItemView,
    tui,
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
    /// Refresh supported provider history without changing provider-owned files.
    Scan {
        /// Emit a machine-readable result.
        #[arg(long)]
        json: bool,
    },
    /// List already-cataloged sessions without scanning provider-owned files.
    List {
        /// Maximum number of sessions to display.
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
        /// Emit a machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Preview one already-cataloged session without scanning provider-owned files.
    Show {
        /// Catalog session ID from `agentlog list`.
        session_id: i64,
        /// Emit a machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Browse already-cataloged sessions in an interactive terminal UI.
    Browse,
}

/// Parses process arguments and dispatches one command-line workflow.
///
/// # Errors
///
/// Returns an error when path resolution, an application workflow, or output
/// serialization fails.
pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::resolve(cli.home).context("resolve Agentlog-owned paths")?;

    match cli.command.unwrap_or(Command::Scan { json: false }) {
        Command::Paths { json } => print_paths(&paths, json),
        Command::Scan { json } => print_scan(&paths, json).await,
        Command::List { limit, json } => print_list(&paths, limit, json).await,
        Command::Show { session_id, json } => print_show(&paths, session_id, json).await,
        Command::Browse => tui::run(&paths).await,
    }
}

fn print_paths(paths: &AppPaths, json: bool) -> anyhow::Result<()> {
    let report = paths_report(paths);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Agentlog-owned home: {}",
            visible_text(&report.agentlog_home.display().to_string())
        );
        println!(
            "Agentlog-owned config: {}",
            visible_text(&report.config_file.display().to_string())
        );
        println!(
            "Agentlog-owned data: {}",
            visible_text(&report.data_directory.display().to_string())
        );
        println!(
            "Agentlog-owned database: {}",
            visible_text(&report.database.display().to_string())
        );
        println!(
            "Provider-owned sources: {}",
            visible_text(&report.provider_sources)
        );
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
        for provider in &summary.provider_summaries {
            println!(
                "Provider {}: candidates={}, refreshed={}, partial={}, failed={}, sessions={}",
                visible_text(provider.provider),
                provider.candidate_sources,
                provider.refreshed_sources,
                provider.partial_sources,
                provider.failed_sources,
                provider.sessions_available
            );
        }
        println!("Scan result: {}", visible_text(&summary.message));
    }
    Ok(())
}

async fn print_list(paths: &AppPaths, limit: u32, json: bool) -> anyhow::Result<()> {
    let sessions = list_shell(paths, limit).await?;
    if json {
        println!("{}", serde_json::to_string(&sessions)?);
    } else if sessions.is_empty() {
        println!("No cataloged sessions. Run `agentlog scan` to refresh sources.");
    } else {
        for session in sessions {
            let title = session
                .title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(&session.session_key);
            let freshness = if session.source_diagnostic_status == "error" {
                "stale/error"
            } else {
                session.source_diagnostic_status.as_str()
            };
            println!(
                "{}  [{}:{}; {}] {}",
                session.id,
                visible_text(&session.provider),
                visible_text(&session.source_format),
                visible_text(freshness),
                visible_text(title)
            );
        }
    }
    Ok(())
}

async fn print_show(paths: &AppPaths, session_id: i64, json: bool) -> anyhow::Result<()> {
    let preview = show_shell(paths, session_id).await?;
    if json {
        println!("{}", serde_json::to_string(&preview)?);
    } else {
        let freshness = &preview.session.source_diagnostic_status;
        println!(
            "Session {} [{}:{}; source status: {}]",
            preview.session.id,
            visible_text(&preview.session.provider),
            visible_text(&preview.session.source_format),
            visible_text(freshness)
        );
        let title = preview
            .session
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&preview.session.session_key);
        println!("{}", visible_text(title));
        for item in preview.items {
            match item {
                CatalogItemView::UserText { content } => {
                    print_visible_transcript("user", &content);
                }
                CatalogItemView::AssistantText { content } => {
                    print_visible_transcript("assistant", &content);
                }
                CatalogItemView::ToolMarker { name, status } => {
                    println!(
                        "tool: {} ({})",
                        visible_text(&name),
                        visible_text(status.as_deref().unwrap_or("unknown"))
                    );
                }
            }
        }
        if preview.items_truncated {
            println!("Preview truncated after 80 items.");
        }
    }
    Ok(())
}

fn print_visible_transcript(label: &str, content: &str) {
    if content.is_empty() {
        println!("{label}:");
        return;
    }

    for (index, line) in content.lines().enumerate() {
        let prefix = if index == 0 {
            format!("{label}: ")
        } else {
            "  ".to_owned()
        };
        println!("{prefix}{}", visible_text(line));
    }
}
