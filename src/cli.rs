//! Command-line transport for Taiwalog application workflows.

use std::{
    fs::OpenOptions,
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};
use indicatif::{ProgressBar as IndicatifProgressBar, ProgressStyle};
use urushi::{
    Align, ComponentRole, GridStyle, Length, RenderSettings, StyledText, TextStyle, Theme,
    VerticalAlign, View,
};
use urushi_cli::{CliTheme, Summary, Warning};
use urushi_prompt::{Confirm, ConfirmAnswer, FieldKey, Form, FormOutcome, Group};

use crate::{
    app::{
        SyncProgress, list_shell, paths_report, preview_purge_catalog, purge_catalog_shell,
        show_shell, sync_shell_with_progress,
    },
    display::visible_text,
    paths::AppPaths,
    storage::CatalogItemView,
    theme::{DEFAULT_COLOR_SCHEME, theme_set},
    tui,
};

/// Browse local AI coding-agent history without changing provider-owned files.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Taiwalog-owned home directory. Intended for tests and isolated installs.
    #[arg(long, global = true, env = "TAIWALOG_HOME", value_name = "DIR")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show Taiwalog-owned paths and the separate provider-source boundary.
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
        /// Catalog session ID from `taiwalog list`.
        session_id: i64,
        /// Emit a machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Preview and interactively confirm, or with --yes purge, the Taiwalog-owned catalog.
    Purge {
        /// Purge the reported Taiwalog-owned catalog contents.
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
    let paths = AppPaths::resolve(home).context("resolve Taiwalog-owned paths")?;
    let themes = theme_set();
    let theme = themes.select(DEFAULT_COLOR_SCHEME);

    match command {
        Command::Paths { json } => print_paths(&paths, json, theme),
        Command::Sync { json } => print_sync(&paths, json, theme).await,
        Command::List { limit, json } => print_list(&paths, limit, json, theme).await,
        Command::Show { session_id, json } => print_show(&paths, session_id, json, theme).await,
        Command::Purge { yes } => print_purge(&paths, yes, theme).await,
        Command::Browse => tui::run(&paths, theme).await,
    }
}

/// Taiwalog's run-level composition over reusable urushi components.
///
/// The rails and start/end vocabulary belong to this CLI workflow, while
/// styling, stable component views, ANSI rendering, and live-region lifecycle
/// remain owned by urushi.
struct RunUi {
    theme: Theme,
    cli_theme: CliTheme,
    live: bool,
}

impl RunUi {
    fn new(theme: &Theme) -> Self {
        Self {
            theme: theme.clone(),
            cli_theme: CliTheme::from_theme(theme),
            live: io::stderr().is_terminal(),
        }
    }

    const fn is_live(&self) -> bool {
        self.live
    }

    fn start(&self, title: impl AsRef<str>) -> io::Result<()> {
        let accent = self.theme.text_style(ComponentRole::Accent);
        let muted = self.theme.text_style(ComponentRole::Muted);
        let view = View::row(
            VerticalAlign::Top,
            [
                View::text("┌", accent.clone()),
                View::text("  ", muted),
                View::text(title.as_ref(), accent),
            ],
        );
        self.write_view(&view)
    }

    fn summary(&self, summary: &Summary) -> io::Result<()> {
        self.write_view(&self.cli_theme.summary(summary))
    }

    fn warning(&self, message: impl Into<String>) -> io::Result<()> {
        self.write_view(&self.cli_theme.warning(&Warning::new("Warning", message)))
    }

    fn finish_success(&self, message: impl AsRef<str>) -> io::Result<()> {
        self.finish(
            "✓",
            self.theme.text_style(ComponentRole::Success),
            message.as_ref(),
        )
    }

    fn finish_cancelled(&self, message: impl AsRef<str>) -> io::Result<()> {
        self.finish(
            "–",
            TextStyle::new().foreground(self.theme.tokens().warning),
            message.as_ref(),
        )
    }

    fn finish(&self, marker: &str, outcome: TextStyle, message: &str) -> io::Result<()> {
        self.write_view(&run_outcome_view(&self.theme, marker, outcome, message))
    }

    fn spinner(&self, message: impl Into<String>) -> io::Result<Activity> {
        Activity::new(self.live, None, message.into())
    }

    fn progress(&self, total: u64, message: impl Into<String>) -> io::Result<Activity> {
        Activity::new(self.live, Some(total), message.into())
    }

    fn write_view(&self, view: &View) -> io::Result<()> {
        if self.live {
            return urushi::eprintln_view(view);
        }
        let resolved = urushi::resolve(view, urushi::Available::NONE).map_err(io::Error::other)?;
        let rendered = urushi::render(&resolved, &RenderSettings::default());
        let mut stderr = io::stderr().lock();
        for line in rendered.lines() {
            writeln!(stderr, "{}", line.trim_end())?;
        }
        Ok(())
    }
}

fn run_outcome_view(theme: &Theme, marker: &str, outcome: TextStyle, message: &str) -> View {
    let muted = theme.text_style(ComponentRole::Muted);
    let body = theme.text_style(ComponentRole::Body);
    View::column(
        Align::Left,
        [
            View::text("│", muted.clone()),
            View::grid(
                GridStyle::new().columns([Some(Length::Cells(5)), None]),
                [[
                    View::text(format!("└  {marker} "), outcome),
                    View::text(message, body),
                ]],
            ),
        ],
    )
}

struct Activity {
    progress: Option<IndicatifProgressBar>,
}

impl Activity {
    fn new(live: bool, total: Option<u64>, message: String) -> io::Result<Self> {
        if !live {
            eprintln!("{message}");
            return Ok(Self { progress: None });
        }
        let progress =
            total.map_or_else(IndicatifProgressBar::new_spinner, IndicatifProgressBar::new);
        let template = if total.is_some() {
            "{spinner} {msg} [{pos}/{len}]"
        } else {
            "{spinner} {msg}"
        };
        progress.set_style(
            ProgressStyle::with_template(template)
                .map_err(io::Error::other)?
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
        );
        progress.set_message(message);
        progress.enable_steady_tick(std::time::Duration::from_millis(80));
        Ok(Self {
            progress: Some(progress),
        })
    }

    fn set_position(&self, position: u64) {
        if let Some(progress) = &self.progress {
            progress.set_position(position);
        }
    }

    fn stop(self, message: impl Into<String>) {
        self.finish(message);
    }

    fn success(self, message: impl Into<String>) {
        self.finish(message);
    }

    fn error(self, message: impl Into<String>) {
        self.finish(message);
    }

    fn finish(self, message: impl Into<String>) {
        let message = message.into();
        if let Some(progress) = self.progress {
            progress.finish_and_clear();
        }
        eprintln!("{message}");
    }
}

async fn print_purge(paths: &AppPaths, yes: bool, theme: &Theme) -> anyhow::Result<()> {
    let prompt_eligible = !yes && interactive_terminal();
    let report = RunUi::new(theme);

    report
        .start("Purge Taiwalog catalog")
        .context("start purge report")?;

    if yes {
        let summary = execute_purge(&report, paths).await?;
        print_purge_report(&report, &summary, "Purged catalog")?;
        print_purge_completion(&report, &summary)?;
        return Ok(());
    }

    let preview = preview_purge_catalog(paths).await?;
    print_purge_report(&report, &preview, "Catalog preview")?;
    match confirm_purge_from_terminal(&preview, prompt_eligible, theme)? {
        Some(true) => {
            let summary = execute_purge(&report, paths).await?;
            print_purge_completion(&report, &summary)?;
        }
        Some(false) => report
            .finish_cancelled("Purge canceled; no changes were made.")
            .context("finish canceled purge report")?,
        None => {
            report
                .finish_cancelled(
                    "No changes were made. Re-run with `taiwalog purge --yes` to confirm.",
                )
                .context("finish unconfirmed purge report")?;
        }
    }
    Ok(())
}

async fn execute_purge(
    report: &RunUi,
    paths: &AppPaths,
) -> anyhow::Result<crate::app::PurgeSummary> {
    let progress = report
        .spinner("Purging catalog rows")
        .context("start purge progress")?;
    match purge_catalog_shell(paths, true).await {
        Ok(summary) => {
            progress.success("Catalog rows purged");
            Ok(summary)
        }
        Err(error) => {
            progress.error("Catalog purge failed");
            Err(error)
        }
    }
}

fn print_purge_report(
    report: &RunUi,
    summary: &crate::app::PurgeSummary,
    heading: &str,
) -> anyhow::Result<()> {
    let fields = Summary::new(heading)
        .field(
            "Database target",
            visible_text(&summary.database.display().to_string()),
        )
        .field("Catalog sources", summary.sources.to_string())
        .field("Catalog sessions", summary.sessions.to_string())
        .field("Transcript items", summary.transcript_items.to_string())
        .field(
            "Approximate size",
            format!(
                "{} ({} bytes)",
                human_readable_bytes(summary.approximate_bytes),
                summary.approximate_bytes
            ),
        );
    let warning = "Catalog data may not be reconstructible: only provider logs that still exist and remain readable can be synchronized.";

    report
        .summary(&fields)
        .context("write purge catalog report")?;
    report.warning(warning).context("write purge warning")?;
    Ok(())
}

fn print_purge_completion(
    report: &RunUi,
    summary: &crate::app::PurgeSummary,
) -> anyhow::Result<()> {
    let message = if summary.cleared {
        format!(
            "Purge complete: cleared {} catalog sources, {} sessions, and {} transcript items from the locked catalog state ({}; {} bytes).",
            summary.sources,
            summary.sessions,
            summary.transcript_items,
            human_readable_bytes(summary.approximate_bytes),
            summary.approximate_bytes
        )
    } else {
        "Purge complete: no Taiwalog-owned database was present.".to_owned()
    };
    report
        .finish_success(message)
        .context("finish purge report")?;
    Ok(())
}

/// Prompts only through the controlling terminal, never through piped input.
///
/// An unavailable terminal intentionally leaves purge in preview-only mode.
fn confirm_purge_from_terminal(
    summary: &crate::app::PurgeSummary,
    prompt_eligible: bool,
    theme: &Theme,
) -> anyhow::Result<Option<bool>> {
    if !prompt_eligible {
        return Ok(None);
    }

    let answer_key = FieldKey::<ConfirmAnswer>::new("purge-confirmation");
    let confirmation = Confirm::new(
        answer_key.clone(),
        purge_confirmation_message(summary),
        Some(false),
    )
    .context("configure purge confirmation")?
    .help("←/→ choose • y/n answer • enter submit • esc cancel");
    let form = Form::builder()
        .group(
            Group::builder()
                .field(confirmation)
                .build()
                .context("configure purge prompt group")?,
        )
        .build()
        .context("configure purge prompt")?;

    match form.run(theme).context("run purge confirmation")? {
        FormOutcome::Submitted(values) => values
            .get(&answer_key)
            .map(|answer| Some(answer.value))
            .context("read submitted purge confirmation"),
        FormOutcome::Cancelled => Ok(Some(false)),
    }
}

fn purge_confirmation_message(summary: &crate::app::PurgeSummary) -> String {
    format!(
        "Purge {} catalog sources, {} sessions, and {} transcript items now?",
        summary.sources, summary.sessions, summary.transcript_items
    )
}

fn interactive_terminal() -> bool {
    let stdin = io::stdin().is_terminal();
    let stdout = io::stdout().is_terminal();
    let stderr = io::stderr().is_terminal();
    if !(stdin && stdout && stderr) {
        return false;
    }
    let Ok(terminal) = OpenOptions::new().read(true).open("/dev/tty") else {
        return false;
    };
    let foreground = rustix::termios::tcgetpgrp(&terminal)
        .is_ok_and(|group| group == rustix::process::getpgrp());
    PromptEligibility {
        stdio: [stdin, stdout, stderr],
        foreground,
    }
    .allows_prompt()
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

fn print_paths(paths: &AppPaths, json: bool, theme: &Theme) -> anyhow::Result<()> {
    let report = paths_report(paths);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let palette = OutputPalette::for_stdout(theme);
        println!(
            "{} {}",
            palette.paint(ComponentRole::Muted, "Taiwalog-owned home:"),
            palette.paint(
                ComponentRole::Body,
                &visible_text(&report.taiwalog_home.display().to_string())
            )
        );
        println!(
            "{} {}",
            palette.paint(ComponentRole::Muted, "Taiwalog-owned config:"),
            palette.paint(
                ComponentRole::Body,
                &visible_text(&report.config_file.display().to_string())
            )
        );
        println!(
            "{} {}",
            palette.paint(ComponentRole::Muted, "Taiwalog-owned data:"),
            palette.paint(
                ComponentRole::Body,
                &visible_text(&report.data_directory.display().to_string())
            )
        );
        println!(
            "{} {}",
            palette.paint(ComponentRole::Muted, "Taiwalog-owned database:"),
            palette.paint(
                ComponentRole::Body,
                &visible_text(&report.database.display().to_string())
            )
        );
        println!(
            "{} {}",
            palette.paint(ComponentRole::Muted, "Provider-owned sources:"),
            palette.paint(ComponentRole::Body, &visible_text(&report.provider_sources))
        );
    }
    Ok(())
}

async fn print_sync(paths: &AppPaths, json: bool, theme: &Theme) -> anyhow::Result<()> {
    let report = RunUi::new(theme);
    report
        .start("Synchronize provider history")
        .context("start sync report")?;
    let mut reporter = SyncProgressReporter::new(&report);
    let summary = sync_shell_with_progress(paths, |progress| reporter.report(progress)).await?;
    reporter.finish().context("finish sync progress")?;
    print_sync_summary(&report, &summary)?;
    report
        .finish_success(visible_text(&summary.message))
        .context("finish sync report")?;
    if json {
        println!("{}", serde_json::to_string(&summary)?);
    }
    Ok(())
}

fn print_sync_summary(report: &RunUi, summary: &crate::app::SyncSummary) -> anyhow::Result<()> {
    let mut fields = Summary::new("Sync summary")
        .field("Schema version", summary.schema_version.to_string())
        .field(
            "Collectors installed",
            summary.collectors_installed.to_string(),
        )
        .field("Providers failed", summary.providers_failed.to_string())
        .field("Sources refreshed", summary.sources_refreshed.to_string())
        .field("Sources partial", summary.sources_partial.to_string())
        .field("Sources failed", summary.sources_failed.to_string())
        .field("Sources missing", summary.sources_missing.to_string())
        .field("Sessions available", summary.sessions_available.to_string());
    for provider in &summary.provider_summaries {
        fields = fields.field(
            format!("Provider {}", visible_text(&provider.provider)),
            format!(
                "candidates={}, refreshed={}, partial={}, failed={}, missing={}, sessions={}",
                provider.candidate_sources,
                provider.refreshed_sources,
                provider.partial_sources,
                provider.failed_sources,
                provider.missing_sources,
                provider.sessions_available
            ),
        );
    }
    report.summary(&fields).context("write sync summary")?;
    for provider in &summary.provider_summaries {
        if let Some(failure) = &provider.failure {
            report
                .warning(format!(
                    "Provider {}: {}",
                    visible_text(&provider.provider),
                    visible_text(failure)
                ))
                .context("write provider warning")?;
        }
    }
    Ok(())
}

/// Writes compact synchronization progress to stderr without touching stdout.
struct SyncProgressReporter<'a> {
    report: &'a RunUi,
    active: Option<ActiveProgress>,
    error: Option<io::Error>,
}

impl<'a> SyncProgressReporter<'a> {
    const fn new(report: &'a RunUi) -> Self {
        Self {
            report,
            active: None,
            error: None,
        }
    }

    fn report(&mut self, progress: SyncProgress) {
        if self.error.is_some() {
            return;
        }
        match progress {
            SyncProgress::ProviderDiscovering { provider } => {
                match self.report.spinner(format!(
                    "Provider {}: discovering sources",
                    visible_text(&provider)
                )) {
                    Ok(spinner) => self.active = Some(ActiveProgress::Spinner(spinner)),
                    Err(error) => self.error = Some(error),
                }
            }
            SyncProgress::ProviderCandidates {
                provider,
                candidate_sources,
            } => {
                let provider = visible_text(&provider);
                self.stop_active(format!(
                    "Provider {provider}: discovered {candidate_sources} candidate sources"
                ));
                if self.error.is_none() {
                    match self.report.progress(
                        candidate_sources,
                        format!("Provider {provider}: synchronizing sources"),
                    ) {
                        Ok(progress) => self.active = Some(ActiveProgress::Bar(progress)),
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            SyncProgress::SourceStaged {
                provider: _,
                processed_sources,
                candidate_sources: _,
            } => {
                if self.report.is_live()
                    && let Some(ActiveProgress::Bar(progress)) = &mut self.active
                {
                    progress.set_position(processed_sources);
                }
            }
            SyncProgress::ProviderCompleted {
                provider,
                report,
                failure,
            } => {
                if let Some(failure) = failure {
                    self.error_active(format!(
                        "Provider {}: failed ({})",
                        visible_text(&provider),
                        visible_text(&failure)
                    ));
                } else {
                    self.complete_active(format!(
                        "Provider {}: candidates={}, refreshed={}, partial={}, failed={}, missing={}",
                        visible_text(&provider),
                        report.candidate_sources,
                        report.refreshed_sources,
                        report.partial_sources,
                        report.failed_sources,
                        report.missing_sources
                    ));
                }
            }
        }
    }

    fn stop_active(&mut self, message: String) {
        self.finish_active(FinishProgress::Stop, message);
    }

    fn error_active(&mut self, message: String) {
        self.finish_active(FinishProgress::Error, message);
    }

    fn complete_active(&mut self, message: String) {
        self.finish_active(FinishProgress::Success, message);
    }

    fn finish_active(&mut self, finish: FinishProgress, message: String) {
        let Some(active) = self.active.take() else {
            return;
        };
        let progress = match active {
            ActiveProgress::Spinner(progress) | ActiveProgress::Bar(progress) => progress,
        };
        match finish {
            FinishProgress::Stop => progress.stop(message),
            FinishProgress::Success => progress.success(message),
            FinishProgress::Error => progress.error(message),
        }
    }

    fn finish(mut self) -> io::Result<()> {
        if let Some(error) = self.error.take() {
            Err(error)
        } else {
            Ok(())
        }
    }
}

enum ActiveProgress {
    Spinner(Activity),
    Bar(Activity),
}

#[derive(Clone, Copy)]
enum FinishProgress {
    Stop,
    Success,
    Error,
}

async fn print_list(paths: &AppPaths, limit: u32, json: bool, theme: &Theme) -> anyhow::Result<()> {
    let sessions = list_shell(paths, limit).await?;
    if json {
        println!("{}", serde_json::to_string(&sessions)?);
    } else if sessions.is_empty() {
        let palette = OutputPalette::for_stdout(theme);
        println!(
            "{}",
            palette.paint(
                ComponentRole::Muted,
                "No cataloged sessions. Run `taiwalog sync` to synchronize sources."
            )
        );
    } else {
        let palette = OutputPalette::for_stdout(theme);
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
                "{}  {} {}",
                palette.paint(ComponentRole::Accent, &session.id.to_string()),
                palette.paint(
                    ComponentRole::Muted,
                    &format!(
                        "[{}:{}; {}]",
                        visible_text(&session.provider),
                        visible_text(&session.source_format),
                        visible_text(freshness)
                    )
                ),
                palette.paint(ComponentRole::Body, &visible_text(title))
            );
        }
    }
    Ok(())
}

async fn print_show(
    paths: &AppPaths,
    session_id: i64,
    json: bool,
    theme: &Theme,
) -> anyhow::Result<()> {
    let preview = show_shell(paths, session_id).await?;
    if json {
        println!("{}", serde_json::to_string(&preview)?);
    } else {
        let palette = OutputPalette::for_stdout(theme);
        let freshness = &preview.session.source_diagnostic_status;
        println!(
            "{} {}",
            palette.paint(
                ComponentRole::Accent,
                &format!("Session {}", preview.session.id)
            ),
            palette.paint(
                ComponentRole::Muted,
                &format!(
                    "[{}:{}; source status: {}]",
                    visible_text(&preview.session.provider),
                    visible_text(&preview.session.source_format),
                    visible_text(freshness)
                )
            )
        );
        let title = preview
            .session
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&preview.session.session_key);
        println!(
            "{}",
            palette.paint(ComponentRole::Body, &visible_text(title))
        );
        for item in preview.items {
            match item {
                CatalogItemView::UserText { content } => {
                    print_visible_transcript("user", &content, ComponentRole::Accent, &palette);
                }
                CatalogItemView::AssistantText { content } => {
                    print_visible_transcript(
                        "assistant",
                        &content,
                        ComponentRole::Success,
                        &palette,
                    );
                }
                CatalogItemView::ToolMarker { name, status } => {
                    println!(
                        "{} {} {}",
                        palette.warning("tool:"),
                        palette.paint(ComponentRole::Body, &visible_text(&name)),
                        palette.paint(
                            ComponentRole::Muted,
                            &format!("({})", visible_text(status.as_deref().unwrap_or("unknown")))
                        )
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

fn print_visible_transcript(
    label: &str,
    content: &str,
    label_role: ComponentRole,
    palette: &OutputPalette,
) {
    if content.is_empty() {
        println!("{}", palette.paint(label_role, &format!("{label}:")));
        return;
    }

    for (index, line) in content.lines().enumerate() {
        let prefix = if index == 0 {
            format!("{} ", palette.paint(label_role, &format!("{label}:")))
        } else {
            "  ".to_owned()
        };
        println!(
            "{prefix}{}",
            palette.paint(ComponentRole::Body, &visible_text(line))
        );
    }
}

struct OutputPalette<'a> {
    theme: &'a Theme,
    settings: RenderSettings,
}

impl<'a> OutputPalette<'a> {
    fn for_stdout(theme: &'a Theme) -> Self {
        let color = io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
        let settings = if color {
            RenderSettings::all()
        } else {
            RenderSettings::default()
        };
        Self { theme, settings }
    }

    fn paint(&self, role: ComponentRole, text: &str) -> String {
        self.paint_style(self.theme.text_style(role), text)
    }

    fn warning(&self, text: &str) -> String {
        self.paint_style(
            TextStyle::new().foreground(self.theme.tokens().warning),
            text,
        )
    }

    fn paint_style(&self, style: TextStyle, text: &str) -> String {
        urushi::render_text(&StyledText::new(text, style), &self.settings)
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        Cli, Command, PromptEligibility, human_readable_bytes, purge_confirmation_message,
        run_outcome_view,
    };
    use crate::theme::{DEFAULT_COLOR_SCHEME, theme_set};

    #[test]
    fn database_sizes_use_readable_binary_units() {
        assert_eq!(human_readable_bytes(0), "0 B");
        assert_eq!(human_readable_bytes(1023), "1023 B");
        assert_eq!(human_readable_bytes(1024), "1.0 KiB");
        assert_eq!(human_readable_bytes(128_339_968), "122.4 MiB");
        assert_eq!(human_readable_bytes(u64::MAX), "16.0 EiB");
    }

    #[test]
    fn run_outcomes_wrap_without_moving_workflow_vocabulary_into_urushi() {
        let themes = theme_set();
        let theme = themes.select(DEFAULT_COLOR_SCHEME);
        let view = run_outcome_view(
            theme,
            "✓",
            theme.text_style(urushi::ComponentRole::Success),
            "one two three four",
        );
        let resolved =
            urushi::resolve(&view, urushi::Available::columns(15)).expect("resolve run outcome");
        let rendered = urushi::render(&resolved, &urushi::RenderSettings::default());

        assert_eq!(rendered.lines().count(), 3);
        assert!(
            rendered
                .lines()
                .nth(1)
                .is_some_and(|line| line.starts_with("└  ✓ ")),
            "unexpected outcome rendering: {rendered:?}"
        );
        assert!(
            rendered
                .lines()
                .nth(2)
                .is_some_and(|line| line.starts_with("     "))
        );
    }

    #[test]
    fn purge_accepts_preview_and_explicit_confirmation() {
        let preview = Cli::try_parse_from(["taiwalog", "purge"]).expect("parse purge preview");
        assert!(matches!(
            preview.command,
            Some(Command::Purge { yes: false })
        ));

        let confirmed =
            Cli::try_parse_from(["taiwalog", "purge", "--yes"]).expect("parse confirmed purge");
        assert!(matches!(
            confirmed.command,
            Some(Command::Purge { yes: true })
        ));
    }

    #[test]
    fn purge_rejects_legacy_data_clear_and_dry_run_forms() {
        assert!(Cli::try_parse_from(["taiwalog", "data", "clear"]).is_err());
        assert!(Cli::try_parse_from(["taiwalog", "purge", "--dry-run"]).is_err());
    }

    #[test]
    fn purge_confirmation_message_names_the_previewed_blast_radius() {
        let summary = crate::app::PurgeSummary {
            database: std::path::PathBuf::from("/tmp/taiwalog.sqlite3"),
            sources: 2,
            sessions: 3,
            transcript_items: 5,
            approximate_bytes: 0,
            cleared: false,
        };
        assert_eq!(
            purge_confirmation_message(&summary),
            "Purge 2 catalog sources, 3 sessions, and 5 transcript items now?"
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
    fn bare_root_keeps_the_command_empty_for_help_without_running_sync() {
        let cli = Cli::try_parse_from(["taiwalog"]).expect("parse bare root command");
        assert!(cli.command.is_none());
    }

    #[test]
    fn sync_is_accepted_and_scan_is_rejected() {
        let sync = Cli::try_parse_from(["taiwalog", "sync"]).expect("parse explicit sync command");
        assert!(matches!(sync.command, Some(Command::Sync { json: false })));
        assert!(Cli::try_parse_from(["taiwalog", "scan"]).is_err());
    }
}
