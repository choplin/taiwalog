//! Command-line transport for Agentlog application workflows.

use std::{
    fs::File,
    fs::OpenOptions,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
};

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};

use crate::{
    app::{
        SyncProgress, list_shell, paths_report, preview_purge_catalog, purge_catalog_shell,
        purge_previewed_catalog, show_shell, sync_shell_with_progress,
    },
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
    /// Preview and interactively confirm, or with --yes purge, the Agentlog-owned catalog.
    Purge {
        /// Purge the reported Agentlog-owned catalog contents.
        #[arg(long)]
        yes: bool,
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
        Command::Purge { yes } => print_purge(&paths, yes).await,
        Command::Browse => tui::run(&paths).await,
    }
}

async fn print_purge(paths: &AppPaths, yes: bool) -> anyhow::Result<()> {
    if yes {
        let summary = purge_catalog_shell(paths, true).await?;
        print_purge_report(&summary);
        print_purge_completion(&summary);
        return Ok(());
    }

    let preview = preview_purge_catalog(paths).await?;
    print_purge_report(&preview.summary);
    match confirm_purge_from_terminal(&preview.summary)? {
        Some(true) => {
            let summary = purge_previewed_catalog(paths, preview).await?;
            print_purge_completion(&summary);
        }
        Some(false) => eprintln!("Purge canceled; no changes were made."),
        None => eprintln!("No changes were made. Re-run with `agentlog purge --yes` to confirm."),
    }
    Ok(())
}

fn print_purge_report(summary: &crate::app::PurgeSummary) {
    eprintln!(
        "Agentlog-owned database target: {}",
        visible_text(&summary.database.display().to_string())
    );
    eprintln!("Catalog sources: {}", summary.sources);
    eprintln!("Catalog sessions: {}", summary.sessions);
    eprintln!("Transcript items: {}", summary.transcript_items);
    eprintln!(
        "Approximate database size: {} ({} bytes)",
        human_readable_bytes(summary.approximate_bytes),
        summary.approximate_bytes
    );
    eprintln!(
        "Warning: Catalog data may not be reconstructible: only provider logs that still exist and remain readable can be synchronized."
    );
}

fn print_purge_completion(summary: &crate::app::PurgeSummary) {
    if summary.cleared {
        eprintln!(
            "Purge complete: cleared {} catalog sources, {} sessions, and {} transcript items from the locked catalog state ({}; {} bytes).",
            summary.sources,
            summary.sessions,
            summary.transcript_items,
            human_readable_bytes(summary.approximate_bytes),
            summary.approximate_bytes
        );
    } else {
        eprintln!("Purge complete: no Agentlog-owned database was present.");
    }
}

/// Prompts only through the controlling terminal, never through piped input.
///
/// An unavailable terminal intentionally leaves purge in preview-only mode.
fn confirm_purge_from_terminal(summary: &crate::app::PurgeSummary) -> anyhow::Result<Option<bool>> {
    let Some(mut terminal) = interactive_terminal() else {
        return Ok(None);
    };
    loop {
        eprint!(
            "Purge {} catalog sources, {} sessions, and {} transcript items now? [y/N] ",
            summary.sources, summary.sessions, summary.transcript_items
        );
        io::stderr().flush()?;
        let answer = read_bounded_prompt_line(&mut terminal)?;
        match purge_confirmation_answer(answer.as_deref()) {
            PromptAnswer::Yes => return Ok(Some(true)),
            PromptAnswer::No => return Ok(Some(false)),
            PromptAnswer::Invalid => {
                eprintln!("Please answer `y`/`yes` or `n`/`no`.");
            }
        }
    }
}

fn interactive_terminal() -> Option<File> {
    let stdin = io::stdin().is_terminal();
    let stdout = io::stdout().is_terminal();
    let stderr = io::stderr().is_terminal();
    if !(stdin && stdout && stderr) {
        return None;
    }
    let terminal = OpenOptions::new().read(true).open("/dev/tty").ok()?;
    let foreground = rustix::termios::tcgetpgrp(&terminal).ok()? == rustix::process::getpgrp();
    PromptEligibility {
        stdio: [stdin, stdout, stderr],
        foreground,
    }
    .allows_prompt()
    .then_some(terminal)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PromptEligibility {
    stdio: [bool; 3],
    foreground: bool,
}

impl PromptEligibility {
    const fn allows_prompt(self) -> bool {
        self.stdio[0] && self.stdio[1] && self.stdio[2] && self.foreground
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptAnswer {
    Yes,
    No,
    Invalid,
}

fn purge_confirmation_answer(answer: Option<&str>) -> PromptAnswer {
    let Some(answer) = answer.map(str::trim) else {
        return PromptAnswer::No;
    };
    if answer.is_empty() || answer.eq_ignore_ascii_case("n") || answer.eq_ignore_ascii_case("no") {
        PromptAnswer::No
    } else if answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") {
        PromptAnswer::Yes
    } else {
        PromptAnswer::Invalid
    }
}

fn read_bounded_prompt_line(reader: &mut impl Read) -> io::Result<Option<String>> {
    const MAX_ANSWER_BYTES: usize = 16;
    let mut answer = Vec::with_capacity(MAX_ANSWER_BYTES);
    let mut truncated = false;
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte)? {
            0 if answer.is_empty() && !truncated => return Ok(None),
            0 => break,
            1 if byte[0] == b'\n' => break,
            1 if answer.len() < MAX_ANSWER_BYTES => answer.push(byte[0]),
            1 => truncated = true,
            _ => unreachable!("single-byte read returned more than one byte"),
        }
    }
    if truncated {
        return Ok(Some("<answer too long>".to_owned()));
    }
    Ok(Some(String::from_utf8_lossy(&answer).into_owned()))
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
    let mut reporter = SyncProgressReporter::new();
    let summary = sync_shell_with_progress(paths, |progress| reporter.report(progress)).await?;
    if json {
        println!("{}", serde_json::to_string(&summary)?);
    } else {
        eprintln!("Schema version: {}", summary.schema_version);
        eprintln!("Collectors installed: {}", summary.collectors_installed);
        eprintln!("Providers failed: {}", summary.providers_failed);
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

/// Writes compact synchronization progress to stderr without touching stdout.
struct SyncProgressReporter {
    interactive: bool,
    live_line_active: bool,
}

impl SyncProgressReporter {
    fn new() -> Self {
        Self {
            interactive: std::io::stderr().is_terminal(),
            live_line_active: false,
        }
    }

    fn report(&mut self, progress: SyncProgress) {
        match progress {
            SyncProgress::ProviderDiscovering { provider } => {
                if self.interactive {
                    self.replace_live_line(&format!(
                        "[~] Provider {}: discovering sources",
                        visible_text(&provider)
                    ));
                } else {
                    eprintln!(
                        "[~] Provider {}: discovering sources",
                        visible_text(&provider)
                    );
                }
            }
            SyncProgress::ProviderCandidates {
                provider,
                candidate_sources,
            } => {
                if self.interactive {
                    self.replace_live_line(&format!(
                        "[~] Provider {}: 0/{} sources processed",
                        visible_text(&provider),
                        candidate_sources
                    ));
                } else {
                    eprintln!(
                        "[~] Provider {}: starting {} candidate sources",
                        visible_text(&provider),
                        candidate_sources
                    );
                }
            }
            SyncProgress::SourceStaged {
                provider,
                processed_sources,
                candidate_sources,
            } => {
                if self.interactive {
                    self.replace_live_line(&format!(
                        "[~] Provider {}: {}/{} sources processed",
                        visible_text(&provider),
                        processed_sources,
                        candidate_sources
                    ));
                }
            }
            SyncProgress::ProviderCompleted {
                provider,
                report,
                failure,
            } => {
                self.clear_live_line();
                if let Some(failure) = failure {
                    eprintln!(
                        "[!] Provider {}: failed ({})",
                        visible_text(&provider),
                        visible_text(&failure)
                    );
                } else {
                    eprintln!(
                        "[+] Provider {}: candidates={}, refreshed={}, partial={}, failed={}, missing={}",
                        visible_text(&provider),
                        report.candidate_sources,
                        report.refreshed_sources,
                        report.partial_sources,
                        report.failed_sources,
                        report.missing_sources
                    );
                }
            }
        }
    }

    fn replace_live_line(&mut self, line: &str) {
        eprint!("\r\x1b[2K{line}");
        let _ = std::io::stderr().flush();
        self.live_line_active = true;
    }

    fn clear_live_line(&mut self) {
        if self.live_line_active {
            eprint!("\r\x1b[2K");
            let _ = std::io::stderr().flush();
            self.live_line_active = false;
        }
    }
}

impl Drop for SyncProgressReporter {
    fn drop(&mut self) {
        self.clear_live_line();
    }
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

    use super::{
        Cli, Command, PromptAnswer, PromptEligibility, human_readable_bytes,
        purge_confirmation_answer, read_bounded_prompt_line,
    };

    #[test]
    fn database_sizes_use_readable_binary_units() {
        assert_eq!(human_readable_bytes(0), "0 B");
        assert_eq!(human_readable_bytes(1023), "1023 B");
        assert_eq!(human_readable_bytes(1024), "1.0 KiB");
        assert_eq!(human_readable_bytes(128_339_968), "122.4 MiB");
        assert_eq!(human_readable_bytes(u64::MAX), "16.0 EiB");
    }

    #[test]
    fn purge_accepts_preview_and_explicit_confirmation() {
        let preview = Cli::try_parse_from(["agentlog", "purge"]).expect("parse purge preview");
        assert!(matches!(
            preview.command,
            Some(Command::Purge { yes: false })
        ));

        let confirmed =
            Cli::try_parse_from(["agentlog", "purge", "--yes"]).expect("parse confirmed purge");
        assert!(matches!(
            confirmed.command,
            Some(Command::Purge { yes: true })
        ));
    }

    #[test]
    fn purge_rejects_legacy_data_clear_and_dry_run_forms() {
        assert!(Cli::try_parse_from(["agentlog", "data", "clear"]).is_err());
        assert!(Cli::try_parse_from(["agentlog", "purge", "--dry-run"]).is_err());
    }

    #[test]
    fn purge_confirmation_defaults_to_no() {
        assert_eq!(purge_confirmation_answer(None), PromptAnswer::No);
        assert_eq!(purge_confirmation_answer(Some("\n")), PromptAnswer::No);
        assert_eq!(purge_confirmation_answer(Some("No\n")), PromptAnswer::No);
        assert_eq!(purge_confirmation_answer(Some("Y\n")), PromptAnswer::Yes);
        assert_eq!(purge_confirmation_answer(Some("YeS\n")), PromptAnswer::Yes);
        assert_eq!(
            purge_confirmation_answer(Some("maybe\n")),
            PromptAnswer::Invalid
        );
    }

    #[test]
    fn purge_prompt_requires_all_three_standard_streams_to_be_terminals() {
        let eligible = PromptEligibility {
            stdio: [true, true, true],
            foreground: true,
        };
        assert!(eligible.allows_prompt());
        for ineligible in [
            PromptEligibility {
                stdio: [false, true, true],
                ..eligible
            },
            PromptEligibility {
                stdio: [true, false, true],
                ..eligible
            },
            PromptEligibility {
                stdio: [true, true, false],
                ..eligible
            },
            PromptEligibility {
                foreground: false,
                ..eligible
            },
        ] {
            assert!(!ineligible.allows_prompt());
        }
    }

    #[test]
    fn purge_prompt_line_buffer_is_bounded() {
        let mut input = std::io::Cursor::new(format!("{}\ny\n", "x".repeat(1_024)));
        let first = read_bounded_prompt_line(&mut input).expect("read oversized answer");
        let second = read_bounded_prompt_line(&mut input).expect("read following answer");
        assert_eq!(
            purge_confirmation_answer(first.as_deref()),
            PromptAnswer::Invalid
        );
        assert_eq!(
            purge_confirmation_answer(second.as_deref()),
            PromptAnswer::Yes
        );
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
