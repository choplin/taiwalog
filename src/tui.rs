//! Interactive browse and preview interface for the local Agentlog catalog.

use std::{
    io::{self, Stderr},
    panic::{self, PanicHookInfo},
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    thread,
};

use anyhow::Context;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{list_shell, show_shell},
    display::visible_text,
    paths::AppPaths,
    storage::{CatalogItemView, CatalogSessionPreview, CatalogSessionSummary},
};

const BROWSE_SESSION_LIMIT: u32 = 500;
const WIDE_PREVIEW_MIN_WIDTH: u16 = 100;
const PREVIEW_SCROLL_STEP: u16 = 3;

static TERMINAL_SESSION_LOCK: Mutex<()> = Mutex::new(());

/// Starts the interactive catalog browser without scanning provider sources.
///
/// # Errors
///
/// Returns an error when the terminal cannot enter or leave interactive mode,
/// or when terminal input or drawing fails. Catalog read failures are displayed
/// in the interface and can be retried with `r`.
pub async fn run(paths: &AppPaths) -> anyhow::Result<()> {
    let mut terminal = TerminalSession::enter()?;
    let mut state = BrowseState::default();
    load_sessions(paths, &mut state).await;

    loop {
        terminal.draw(|frame| render(frame, &mut state))?;

        let event = read_event()?;
        match update(&mut state, event) {
            BrowseEffect::None => {}
            BrowseEffect::Exit => break,
            BrowseEffect::LoadSessions => load_sessions(paths, &mut state).await,
            BrowseEffect::LoadPreview(session_id) => {
                load_preview(paths, &mut state, session_id).await;
            }
        }
    }

    terminal.finish().context("restore interactive terminal")
}

async fn load_sessions(paths: &AppPaths, state: &mut BrowseState) {
    match list_shell(paths, BROWSE_SESSION_LIMIT).await {
        Ok(sessions) => {
            state.replace_sessions(sessions);
            if let Some(session_id) = state.selected_session_id() {
                load_preview(paths, state, session_id).await;
            }
        }
        Err(error) => state.set_catalog_error(format!("Could not read the catalog: {error:#}")),
    }
}

async fn load_preview(paths: &AppPaths, state: &mut BrowseState, session_id: i64) {
    match show_shell(paths, session_id).await {
        Ok(preview) => state.set_preview(preview),
        Err(error) => state.set_preview_error(format!("Could not load this preview: {error:#}")),
    }
}

fn read_event() -> io::Result<BrowseEvent> {
    loop {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                return Ok(match key.code {
                    KeyCode::Char('q') => BrowseEvent::Quit,
                    KeyCode::Char('r') => BrowseEvent::Retry,
                    KeyCode::Char('j') | KeyCode::Down => BrowseEvent::Down,
                    KeyCode::Char('k') | KeyCode::Up => BrowseEvent::Up,
                    KeyCode::Char('g') | KeyCode::Home => BrowseEvent::Top,
                    KeyCode::Char('G') | KeyCode::End => BrowseEvent::Bottom,
                    KeyCode::PageDown => BrowseEvent::PageDown,
                    KeyCode::PageUp => BrowseEvent::PageUp,
                    KeyCode::Enter => BrowseEvent::Open,
                    KeyCode::Esc => BrowseEvent::Back,
                    _ => BrowseEvent::Ignore,
                });
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrowseEvent {
    Down,
    Up,
    Top,
    Bottom,
    PageDown,
    PageUp,
    Open,
    Back,
    Retry,
    Quit,
    Ignore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrowseMode {
    Browse,
    Preview,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrowseEffect {
    None,
    Exit,
    LoadSessions,
    LoadPreview(i64),
}

/// The small semantic state that separates terminal input from rendering.
#[derive(Clone, Debug, Eq, PartialEq)]
struct BrowseState {
    sessions: Vec<CatalogSessionSummary>,
    selected: usize,
    mode: BrowseMode,
    preview: Option<CatalogSessionPreview>,
    catalog_error: Option<String>,
    preview_error: Option<String>,
    preview_scroll: u16,
    list_offset: usize,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            mode: BrowseMode::Browse,
            preview: None,
            catalog_error: None,
            preview_error: None,
            preview_scroll: 0,
            list_offset: 0,
        }
    }
}

impl BrowseState {
    fn replace_sessions(&mut self, sessions: Vec<CatalogSessionSummary>) {
        self.sessions = sessions;
        self.selected = 0;
        self.mode = BrowseMode::Browse;
        self.preview = None;
        self.catalog_error = None;
        self.preview_error = None;
        self.preview_scroll = 0;
        self.list_offset = 0;
    }

    fn selected_session_id(&self) -> Option<i64> {
        self.sessions.get(self.selected).map(|session| session.id)
    }

    fn set_preview(&mut self, preview: CatalogSessionPreview) {
        self.preview = Some(preview);
        self.preview_error = None;
        self.preview_scroll = 0;
    }

    fn set_catalog_error(&mut self, message: String) {
        self.sessions.clear();
        self.selected = 0;
        self.mode = BrowseMode::Browse;
        self.preview = None;
        self.catalog_error = Some(message);
        self.preview_error = None;
        self.preview_scroll = 0;
        self.list_offset = 0;
    }

    fn set_preview_error(&mut self, message: String) {
        self.preview = None;
        self.preview_error = Some(message);
        self.preview_scroll = 0;
    }

    fn move_selection(&mut self, target: usize) -> BrowseEffect {
        if self.sessions.is_empty() || target == self.selected {
            return BrowseEffect::None;
        }

        self.selected = target;
        self.preview = None;
        self.preview_error = None;
        self.preview_scroll = 0;
        self.selected_session_id()
            .map_or(BrowseEffect::None, BrowseEffect::LoadPreview)
    }
}

fn update(state: &mut BrowseState, event: BrowseEvent) -> BrowseEffect {
    if state.catalog_error.is_some() {
        return match event {
            BrowseEvent::Retry => BrowseEffect::LoadSessions,
            BrowseEvent::Quit | BrowseEvent::Back => BrowseEffect::Exit,
            _ => BrowseEffect::None,
        };
    }

    match state.mode {
        BrowseMode::Browse => match event {
            BrowseEvent::Quit | BrowseEvent::Back => BrowseEffect::Exit,
            BrowseEvent::Retry => BrowseEffect::LoadSessions,
            BrowseEvent::Down => state.move_selection(
                state
                    .selected
                    .saturating_add(1)
                    .min(state.sessions.len().saturating_sub(1)),
            ),
            BrowseEvent::Up => state.move_selection(state.selected.saturating_sub(1)),
            BrowseEvent::Top => state.move_selection(0),
            BrowseEvent::Bottom => state.move_selection(state.sessions.len().saturating_sub(1)),
            BrowseEvent::Open => {
                if state.selected_session_id().is_some() {
                    state.mode = BrowseMode::Preview;
                }
                BrowseEffect::None
            }
            BrowseEvent::Ignore | BrowseEvent::PageDown | BrowseEvent::PageUp => BrowseEffect::None,
        },
        BrowseMode::Preview => match event {
            BrowseEvent::Quit => BrowseEffect::Exit,
            BrowseEvent::Back => {
                state.mode = BrowseMode::Browse;
                BrowseEffect::None
            }
            BrowseEvent::Retry => state
                .selected_session_id()
                .map_or(BrowseEffect::None, BrowseEffect::LoadPreview),
            BrowseEvent::Down => {
                state.preview_scroll = state.preview_scroll.saturating_add(1);
                BrowseEffect::None
            }
            BrowseEvent::Up => {
                state.preview_scroll = state.preview_scroll.saturating_sub(1);
                BrowseEffect::None
            }
            BrowseEvent::PageDown => {
                state.preview_scroll = state.preview_scroll.saturating_add(PREVIEW_SCROLL_STEP);
                BrowseEffect::None
            }
            BrowseEvent::PageUp => {
                state.preview_scroll = state.preview_scroll.saturating_sub(PREVIEW_SCROLL_STEP);
                BrowseEffect::None
            }
            BrowseEvent::Top => {
                state.preview_scroll = 0;
                BrowseEffect::None
            }
            BrowseEvent::Bottom => {
                state.preview_scroll = state.preview_bottom_scroll();
                BrowseEffect::None
            }
            BrowseEvent::Open | BrowseEvent::Ignore => BrowseEffect::None,
        },
    }
}

impl BrowseState {
    fn preview_bottom_scroll(&self) -> u16 {
        self.preview.as_ref().map_or(0, |preview| {
            let line_count = preview_lines(preview).len().saturating_sub(1);
            u16::try_from(line_count).unwrap_or(u16::MAX)
        })
    }
}

fn render(frame: &mut Frame, state: &mut BrowseState) {
    let area = frame.area();
    if let Some(message) = &state.catalog_error {
        render_message(frame, area, "Catalog error", message, "r retry | q quit");
        return;
    }

    if state.sessions.is_empty() {
        render_message(
            frame,
            area,
            "No cataloged sessions",
            "Run `agentlog scan` to refresh provider history, then return to Browse.",
            "q quit | r reload catalog",
        );
        return;
    }

    match state.mode {
        BrowseMode::Preview => render_full_preview(frame, area, state),
        BrowseMode::Browse if area.width >= WIDE_PREVIEW_MIN_WIDTH => {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
                .split(area);
            render_session_list(frame, columns[0], state);
            render_preview(frame, columns[1], state, "Preview");
        }
        BrowseMode::Browse => render_session_list(frame, area, state),
    }
}

fn render_message(frame: &mut Frame, area: Rect, title: &str, message: &str, controls: &str) {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);
    let message = Paragraph::new(visible_text(message))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(visible_text(title)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(message, regions[0]);
    frame.render_widget(Paragraph::new(visible_text(controls)), regions[1]);
}

fn render_session_list(frame: &mut Frame, area: Rect, state: &mut BrowseState) {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);
    let items = state
        .sessions
        .iter()
        .map(|session| {
            let source_status = if session.source_diagnostic_status == "error" {
                "stale/error"
            } else {
                session.source_diagnostic_status.as_str()
            };
            ListItem::new(Line::from(format!(
                "[{}; {}] {}",
                visible_text(&session.provider),
                visible_text(source_status),
                session_title(session)
            )))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Recent sessions ({})", state.sessions.len())),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default().with_offset(state.list_offset);
    list_state.select(Some(state.selected));
    frame.render_stateful_widget(list, regions[0], &mut list_state);
    state.list_offset = list_state.offset();
    frame.render_widget(
        Paragraph::new("j/k or arrows select | Enter preview | r reload | q quit"),
        regions[1],
    );
}

fn render_full_preview(frame: &mut Frame, area: Rect, state: &BrowseState) {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);
    render_preview(frame, regions[0], state, "Session preview");
    frame.render_widget(
        Paragraph::new(
            "j/k or arrows scroll | PgUp/PgDn page | g/G top/bottom | Esc back | q quit",
        ),
        regions[1],
    );
}

fn render_preview(frame: &mut Frame, area: Rect, state: &BrowseState, title: &str) {
    if let Some(message) = &state.preview_error {
        let preview = Paragraph::new(visible_text(message))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(visible_text(title)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(preview, area);
        return;
    }

    let Some(preview) = &state.preview else {
        frame.render_widget(
            Paragraph::new("Loading selected session preview...").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(visible_text(title)),
            ),
            area,
        );
        return;
    };

    let preview = Paragraph::new(preview_lines(preview))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(visible_text(title)),
        )
        .wrap(Wrap { trim: false })
        .scroll((state.preview_scroll, 0));
    frame.render_widget(preview, area);
}

fn preview_lines(preview: &CatalogSessionPreview) -> Vec<Line<'static>> {
    let session = &preview.session;
    let mut lines = vec![
        Line::from(Span::styled(
            session_title(session),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "{} | {} | {} | source: {}",
            visible_text(&session.provider),
            visible_text(&session.source_format),
            visible_text(&session.session_key),
            visible_text(&session.source_diagnostic_status)
        )),
    ];
    append_optional_metadata(&mut lines, "Repository", session.repository.as_deref());
    append_optional_metadata(&mut lines, "Directory", session.cwd.as_deref());
    append_optional_metadata(&mut lines, "Model", session.model.as_deref());
    append_optional_metadata(&mut lines, "Execution", session.execution_kind.as_deref());
    lines.push(Line::from(""));

    for item in &preview.items {
        match item {
            CatalogItemView::UserText { content } => append_text_item(
                &mut lines,
                "user",
                content,
                Style::default().fg(Color::Cyan),
            ),
            CatalogItemView::AssistantText { content } => append_text_item(
                &mut lines,
                "assistant",
                content,
                Style::default().fg(Color::Green),
            ),
            CatalogItemView::ToolMarker { name, status } => lines.push(Line::from(Span::styled(
                format!(
                    "tool: {} ({})",
                    visible_text(name),
                    visible_text(status.as_deref().unwrap_or("unknown"))
                ),
                Style::default().fg(Color::Yellow),
            ))),
        }
    }

    if preview.items_truncated {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Preview is limited to the first 80 catalog items.",
            Style::default().fg(Color::Yellow),
        )));
    }

    lines
}

fn append_optional_metadata(lines: &mut Vec<Line<'static>>, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        lines.push(Line::from(format!("{label}: {}", visible_text(value))));
    }
}

fn append_text_item(lines: &mut Vec<Line<'static>>, label: &str, content: &str, style: Style) {
    if content.is_empty() {
        lines.push(Line::from(Span::styled(format!("{label}:"), style)));
        return;
    }

    for (index, line) in content.lines().enumerate() {
        let prefix = if index == 0 {
            format!("{label}: ")
        } else {
            "  ".to_owned()
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{}", visible_text(line)),
            style,
        )));
    }
}

fn session_title(session: &CatalogSessionSummary) -> String {
    visible_text(
        session
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&session.session_key),
    )
}

type TerminalBackend = CrosstermBackend<Stderr>;

struct TerminalSession {
    terminal: Terminal<TerminalBackend>,
    active: bool,
    // Drop this before releasing the process-wide lock so a following session
    // always installs its hook on the original process hook.
    _panic_hook: PanicHookRestorer,
    _process_lock: MutexGuard<'static, ()>,
}

impl TerminalSession {
    fn enter() -> anyhow::Result<Self> {
        let process_lock = acquire_terminal_session_lock()?;
        enable_raw_mode().context("enable terminal raw mode")?;
        if let Err(error) = execute!(io::stderr(), EnterAlternateScreen, Hide) {
            let _ = restore_terminal();
            return Err(error).context("enter terminal alternate screen");
        }

        let terminal = match Terminal::new(CrosstermBackend::new(io::stderr())) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = restore_terminal();
                return Err(error).context("create terminal renderer");
            }
        };

        Ok(Self {
            terminal,
            active: true,
            _panic_hook: PanicHookRestorer::install(),
            _process_lock: process_lock,
        })
    }

    fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> io::Result<()> {
        self.terminal.draw(render).map(|_| ())
    }

    fn finish(mut self) -> io::Result<()> {
        let result = restore_terminal();
        self.active = result.is_err();
        result
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            let _ = restore_terminal();
        }
    }
}

fn acquire_terminal_session_lock() -> anyhow::Result<MutexGuard<'static, ()>> {
    match TERMINAL_SESSION_LOCK.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::WouldBlock) => Err(anyhow::anyhow!(
            "an Agentlog interactive terminal session is already active"
        )),
        Err(TryLockError::Poisoned(error)) => Ok(error.into_inner()),
    }
}

fn restore_terminal() -> io::Result<()> {
    let raw_mode_result = disable_raw_mode();
    let screen_result = execute!(io::stderr(), LeaveAlternateScreen, Show);
    raw_mode_result.and(screen_result)
}

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

struct PanicHookRestorer {
    previous: Arc<Mutex<Option<PanicHook>>>,
}

impl PanicHookRestorer {
    fn install() -> Self {
        let previous = Arc::new(Mutex::new(Some(panic::take_hook())));
        let previous_for_hook = Arc::clone(&previous);
        panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal();
            if let Ok(previous) = previous_for_hook.lock()
                && let Some(previous) = previous.as_ref()
            {
                previous(info);
            }
        }));
        Self { previous }
    }
}

impl Drop for PanicHookRestorer {
    fn drop(&mut self) {
        if thread::panicking() {
            let previous = Arc::clone(&self.previous);
            if let Ok(handle) = thread::Builder::new()
                .name("agentlog-panic-hook-restore".to_owned())
                .spawn(move || restore_panic_hook(&previous))
            {
                match handle.join() {
                    Ok(()) | Err(_) => {}
                }
            }
            return;
        }
        restore_panic_hook(&self.previous);
    }
}

fn restore_panic_hook(previous: &Arc<Mutex<Option<PanicHook>>>) {
    if let Ok(mut previous) = previous.lock()
        && let Some(previous) = previous.take()
    {
        panic::set_hook(previous);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        panic::{self, AssertUnwindSafe},
        sync::Arc,
    };

    use ratatui::{Terminal, backend::TestBackend};

    use crate::storage::{CatalogItemView, CatalogSessionPreview, CatalogSessionSummary};

    use super::{
        BrowseEffect, BrowseEvent, BrowseMode, BrowseState, PanicHookRestorer,
        WIDE_PREVIEW_MIN_WIDTH, acquire_terminal_session_lock, render, session_title, update,
    };

    fn session(id: i64, title: Option<&str>) -> CatalogSessionSummary {
        CatalogSessionSummary {
            id,
            provider: "codex".to_owned(),
            source_format: "session_jsonl".to_owned(),
            source_locator: format!("/private/tmp/source-{id}.jsonl"),
            session_key: format!("session-{id}"),
            title: title.map(str::to_owned),
            repository: None,
            cwd: None,
            model: None,
            execution_kind: None,
            started_at: Some(id),
            last_visible_event_at: Some(id),
            source_diagnostic_status: "ok".to_owned(),
            source_last_success_at: Some(id),
        }
    }

    fn preview(session: CatalogSessionSummary) -> CatalogSessionPreview {
        CatalogSessionPreview {
            session,
            items: vec![CatalogItemView::UserText {
                content: "A visible request".to_owned(),
            }],
            items_truncated: false,
        }
    }

    fn rendered_text(state: &mut BrowseState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("create TestBackend terminal");
        terminal
            .draw(|frame| render(frame, state))
            .expect("render UI");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn missing_or_blank_titles_use_the_stable_fallback() {
        assert_eq!(session_title(&session(1, None)), "session-1");
        assert_eq!(session_title(&session(2, Some("   "))), "session-2");
        assert_eq!(session_title(&session(3, Some("Keep title"))), "Keep title");
    }

    #[test]
    fn semantic_events_select_preview_and_scroll_without_terminal_io() {
        let mut state = BrowseState::default();
        state.replace_sessions(vec![
            session(2, Some("Most recent")),
            session(1, Some("Older")),
        ]);

        assert_eq!(
            update(&mut state, BrowseEvent::Down),
            BrowseEffect::LoadPreview(1)
        );
        assert_eq!(state.selected, 1);
        state.list_offset = 1;
        state.set_preview(preview(session(1, Some("Older"))));
        assert_eq!(update(&mut state, BrowseEvent::Open), BrowseEffect::None);
        assert_eq!(state.mode, BrowseMode::Preview);
        assert_eq!(
            update(&mut state, BrowseEvent::PageDown),
            BrowseEffect::None
        );
        assert!(state.preview_scroll > 0);
        assert_eq!(update(&mut state, BrowseEvent::Back), BrowseEffect::None);
        assert_eq!(state.mode, BrowseMode::Browse);
        assert!(state.preview_scroll > 0);
        assert_eq!(state.list_offset, 1);
    }

    #[test]
    fn renderer_keeps_the_list_scroll_anchor_after_returning_from_preview() {
        let mut state = BrowseState::default();
        state.replace_sessions((0_i64..40).map(|id| session(id, Some("Recent"))).collect());
        state.selected = 20;
        state.list_offset = 15;
        assert_eq!(update(&mut state, BrowseEvent::Open), BrowseEffect::None);
        assert_eq!(update(&mut state, BrowseEvent::Back), BrowseEffect::None);

        let rendered = rendered_text(&mut state, 80, 12);
        assert!(rendered.contains("Recent sessions (40)"));
        assert_eq!(state.list_offset, 15);
    }

    #[test]
    fn renderer_uses_passive_preview_only_at_wide_widths() {
        let selected = session(2, Some("Most recent"));
        let mut state = BrowseState::default();
        state.replace_sessions(vec![selected.clone(), session(1, None)]);
        state.set_preview(preview(selected));

        let wide = rendered_text(&mut state, WIDE_PREVIEW_MIN_WIDTH, 16);
        assert!(wide.contains("Recent sessions (2)"));
        assert!(wide.contains("Preview"));
        assert!(wide.contains("A visible request"));

        let narrow = rendered_text(&mut state, WIDE_PREVIEW_MIN_WIDTH - 1, 16);
        assert!(narrow.contains("Recent sessions (2)"));
        assert!(!narrow.contains("A visible request"));
    }

    #[test]
    fn renderer_distinguishes_empty_catalog_and_catalog_failure() {
        let mut empty_state = BrowseState::default();
        let empty = rendered_text(&mut empty_state, 80, 12);
        assert!(empty.contains("No cataloged sessions"));
        assert!(empty.contains("agentlog scan"));

        let mut failed = BrowseState::default();
        failed.set_catalog_error("catalog is unavailable".to_owned());
        let error = rendered_text(&mut failed, 80, 12);
        assert!(error.contains("Catalog error"));
        assert!(error.contains("catalog is unavailable"));
    }

    #[test]
    fn renderer_makes_catalog_controls_inert_in_every_visible_field() {
        let mut selected = session(2, Some("title\u{1b}]0;owned\u{7}"));
        selected.provider = "provider\u{1b}]8;;https://bad\u{7}".to_owned();
        selected.source_format = "format\u{9b}".to_owned();
        selected.session_key = "key\u{1b}]2;bad\u{7}".to_owned();
        selected.repository = Some("repo\u{1b}]52;c;bad\u{7}".to_owned());
        selected.cwd = Some("cwd\u{9b}".to_owned());
        selected.model = Some("model\u{1b}[31m".to_owned());
        selected.execution_kind = Some("kind\u{7}".to_owned());
        selected.source_diagnostic_status = "status\u{1b}[2J".to_owned();
        let mut state = BrowseState::default();
        state.replace_sessions(vec![selected.clone()]);
        state.set_preview(CatalogSessionPreview {
            session: selected,
            items: vec![
                CatalogItemView::UserText {
                    content: "first line\nsecond\u{1b}]0;bad\u{7}".to_owned(),
                },
                CatalogItemView::ToolMarker {
                    name: "tool\u{1b}]8;;bad\u{7}".to_owned(),
                    status: Some("status\u{9b}".to_owned()),
                },
            ],
            items_truncated: false,
        });
        state.mode = BrowseMode::Preview;

        let rendered = rendered_text(&mut state, 120, 30);
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{9b}'));
        assert!(rendered.contains("\\u{001B}"));
        assert!(rendered.contains("\\u{009B}"));
        assert!(rendered.contains("user: first line"));
        assert!(rendered.contains("  second\\u{001B}]0;bad\\u{0007}"));
    }

    #[test]
    fn renderer_keeps_empty_transcript_items_visible() {
        let selected = session(2, Some("Most recent"));
        let mut state = BrowseState::default();
        state.replace_sessions(vec![selected.clone()]);
        state.set_preview(CatalogSessionPreview {
            session: selected,
            items: vec![CatalogItemView::UserText {
                content: String::new(),
            }],
            items_truncated: false,
        });
        state.mode = BrowseMode::Preview;

        let rendered = rendered_text(&mut state, 80, 12);
        assert!(rendered.contains("user:"));
    }

    #[test]
    fn process_wide_terminal_lock_rejects_a_nested_session() {
        let first = acquire_terminal_session_lock().expect("acquire first terminal lock");
        let second = acquire_terminal_session_lock().expect_err("reject nested terminal lock");
        assert!(second.to_string().contains("already active"));
        drop(first);
    }

    #[test]
    fn caught_panic_restores_the_hook_and_releases_the_next_session_lock() {
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let mut hook_state = None;
        let first_panic = panic::catch_unwind(AssertUnwindSafe(|| {
            let _session_lock = acquire_terminal_session_lock().expect("acquire session lock");
            let hook = PanicHookRestorer::install();
            hook_state = Some(Arc::clone(&hook.previous));
            let _hook = hook;
            panic!("test panic while the interactive hook is active");
        }));
        let hook_was_restored = hook_state
            .as_ref()
            .and_then(|state| state.lock().ok().map(|previous| previous.is_none()))
            .unwrap_or(false);
        let next_session = acquire_terminal_session_lock();
        let second_panic = panic::catch_unwind(AssertUnwindSafe(|| panic!("test next session")));
        panic::set_hook(original_hook);

        assert!(first_panic.is_err());
        assert!(hook_was_restored);
        assert!(next_session.is_ok());
        assert!(second_panic.is_err());
    }
}
