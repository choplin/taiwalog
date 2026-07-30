//! Command-line transport for Agentlog application workflows.

use std::path::PathBuf;

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};

use crate::{
    app::{clear_data_shell, list_shell, paths_report, show_shell, sync_shell},
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
    /// Synchronize supported provider history without changing provider-owned files.
    Sync {
        /// Emit a machine-readable result.
        #[arg(long)]
        json: bool,
    },
    /// List already-cataloged sessions without synchronizing provider-owned files.
    List {
        /// Maximum number of sessions to display.
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
        /// Emit a machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Preview one already-cataloged session without synchronizing provider-owned files.
    Show {
        /// Catalog session ID from `agentlog list`.
        session_id: i64,
        /// Emit a machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Inspect or explicitly clear only the Agentlog-owned catalog database.
    Data {
        #[command(subcommand)]
        command: DataCommand,
    },
    /// Browse already-cataloged sessions in an interactive terminal UI.
    Browse,
}

#[derive(Debug, Subcommand)]
enum DataCommand {
    /// Report the catalog clear target; add --yes to clear its contents.
    Clear {
        /// Explicitly report without changing the catalog.
        #[arg(long, conflicts_with = "yes")]
        dry_run: bool,
        /// Confirm clearing the reported Agentlog-owned catalog only.
        #[arg(long)]
        yes: bool,
    },
}

/// Parses process arguments and dispatches one command-line workflow.
///
/// # Errors
///
/// Returns an error when path resolution, an application workflow, or output
/// serialization fails.
pub async fn run() -> anyhow::Result<()> {
    let Cli { home, command } = Cli::parse();
    let Some(command) = command else {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    let paths = AppPaths::resolve(home).context("resolve Agentlog-owned paths")?;

    match command {
        Command::Paths { json } => print_paths(&paths, json),
        Command::Sync { json } => print_sync(&paths, json).await,
        Command::List { limit, json } => print_list(&paths, limit, json).await,
        Command::Show { session_id, json } => print_show(&paths, session_id, json).await,
        Command::Data { command } => match command {
            DataCommand::Clear { dry_run, yes } => print_data_clear(&paths, dry_run, yes).await,
        },
        Command::Browse => tui::run(&paths).await,
    }
}

async fn print_data_clear(paths: &AppPaths, dry_run: bool, yes: bool) -> anyhow::Result<()> {
    let summary = clear_data_shell(paths, yes).await?;
    println!(
        "Agentlog-owned database target: {}",
        visible_text(&summary.database.display().to_string())
    );
    println!("Catalog sessions: {}", summary.sessions);
    println!("Transcript items: {}", summary.transcript_items);
    println!(
        "Approximate database size: {} ({} bytes)",
        human_readable_bytes(summary.approximate_bytes),
        summary.approximate_bytes
    );
    if dry_run {
        println!("Dry run: no data was changed. Re-run with --yes to clear.");
    } else if !yes {
        println!("No data was changed. Re-run with --yes to clear.");
    } else if summary.cleared {
        println!("Data clear: cleared the reported Agentlog-owned catalog contents.");
    } else {
        println!("Data clear: no Agentlog-owned database was present.");
    }
    Ok(())
}

fn human_readable_bytes(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut unit = 0;
    let mut unit_bytes = 1_u64;
    while unit < UNITS.len() - 1 && bytes >= unit_bytes * 1024 {
        unit += 1;
        unit_bytes *= 1024;
    }
    let rounded_tenths =
        (u128::from(bytes) * 10 + u128::from(unit_bytes) / 2) / u128::from(unit_bytes);
    format!(
        "{}.{} {}",
        rounded_tenths / 10,
        rounded_tenths % 10,
        UNITS[unit]
    )
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

async fn print_sync(paths: &AppPaths, json: bool) -> anyhow::Result<()> {
    let summary = sync_shell(paths).await?;
    if json {
        println!("{}", serde_json::to_string(&summary)?);
    } else {
        eprintln!("Schema version: {}", summary.schema_version);
        eprintln!("Collectors installed: {}", summary.collectors_installed);
        eprintln!("Sources refreshed: {}", summary.sources_refreshed);
        eprintln!("Sources partial: {}", summary.sources_partial);
        eprintln!("Sources failed: {}", summary.sources_failed);
        eprintln!("Sources missing: {}", summary.sources_missing);
        eprintln!("Sessions available: {}", summary.sessions_available);
        for provider in &summary.provider_summaries {
            eprintln!(
                "Provider {}: candidates={}, refreshed={}, partial={}, failed={}, missing={}, sessions={}",
                visible_text(&provider.provider),
                provider.candidate_sources,
                provider.refreshed_sources,
                provider.partial_sources,
                provider.failed_sources,
                provider.missing_sources,
                provider.sessions_available
            );
            if let Some(failure) = &provider.failure {
                eprintln!("  warning: {}", visible_text(failure));
            }
        }
        eprintln!("Sync result: {}", visible_text(&summary.message));
    }
    Ok(())
}

async fn print_list(paths: &AppPaths, limit: u32, json: bool) -> anyhow::Result<()> {
    let sessions = list_shell(paths, limit).await?;
    if json {
        println!("{}", serde_json::to_string(&sessions)?);
    } else if sessions.is_empty() {
        println!("No cataloged sessions. Run `agentlog sync` to synchronize sources.");
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

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, DataCommand, human_readable_bytes};

    #[test]
    fn database_sizes_use_readable_binary_units() {
        assert_eq!(human_readable_bytes(0), "0 B");
        assert_eq!(human_readable_bytes(1023), "1023 B");
        assert_eq!(human_readable_bytes(1024), "1.0 KiB");
        assert_eq!(human_readable_bytes(128_339_968), "122.4 MiB");
        assert_eq!(human_readable_bytes(u64::MAX), "16.0 EiB");
    }

    #[test]
    fn data_clear_accepts_an_explicit_dry_run() {
        let cli = Cli::try_parse_from(["agentlog", "data", "clear", "--dry-run"])
            .expect("parse data clear dry-run");
        assert!(matches!(
            cli.command,
            Some(Command::Data {
                command: DataCommand::Clear {
                    dry_run: true,
                    yes: false
                }
            })
        ));
    }

    #[test]
    fn data_clear_rejects_conflicting_confirmation_and_dry_run() {
        assert!(Cli::try_parse_from(["agentlog", "data", "clear", "--dry-run", "--yes"]).is_err());
    }

    #[test]
    fn bare_root_keeps_the_command_empty_for_help_without_running_sync() {
        let cli = Cli::try_parse_from(["agentlog"]).expect("parse bare root command");
        assert!(cli.command.is_none());
    }

    #[test]
    fn sync_is_accepted_and_scan_is_rejected() {
        let sync = Cli::try_parse_from(["agentlog", "sync"]).expect("parse explicit sync command");
        assert!(matches!(sync.command, Some(Command::Sync { json: false })));
        assert!(Cli::try_parse_from(["agentlog", "scan"]).is_err());
    }
}
