//! Read-only M0 measurements. This is not a production collector.

use std::{
    collections::BTreeSet,
    env,
    fmt::Write as _,
    fs::{self, File},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use futures_util::TryStreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

const MAX_JSONL_RECORD_BYTES: usize = 1_048_576;

/// Optional root overrides for deterministic tests and nonstandard installs.
#[derive(Debug, Default)]
pub struct Config {
    pub home: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub kimi_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
}

/// The privacy-preserving output of one sequential full scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub providers: Vec<ProviderMeasurement>,
    pub scan_wall_time: Duration,
    pub hash_audit_wall_time: Duration,
}

/// Provider-level measurements. The fields intentionally contain no paths,
/// source digests, session identifiers, source text, or raw parse failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMeasurement {
    pub provider: Provider,
    pub candidate_sources: usize,
    pub accepted_sources: usize,
    pub catalog_sessions: usize,
    pub reference_legacy_session_ids: usize,
    pub reference_legacy_prompt_records: usize,
    pub rejected_or_partial_sources: usize,
    pub largest_candidate_source_bytes: u64,
    pub scan_time: Duration,
    pub sources_unchanged: bool,
    pub note: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    Codex,
    Claude,
    OpenCode,
    Gemini,
    Cursor,
    Kimi,
}

impl Provider {
    const ALL: [Self; 6] = [
        Self::Codex,
        Self::Claude,
        Self::OpenCode,
        Self::Gemini,
        Self::Cursor,
        Self::Kimi,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
            Self::Cursor => "cursor",
            Self::Kimi => "kimi",
        }
    }
}

#[derive(Debug, Clone)]
struct Source {
    path: PathBuf,
    kind: SourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    CodexCurrent,
    CodexLegacy,
    ClaudeJsonl,
    OpenCodeDatabase,
    GeminiJsonl,
    CursorJsonl,
    KimiWire,
}

/// Execute one provider at a time. Before and after each measured pass, an
/// in-memory SHA-256 fingerprint checks every provider input read by the
/// harness. Hash-audit time is intentionally excluded from scan timings.
///
/// # Errors
///
/// Returns an error only when the conventional roots cannot be inspected. A
/// malformed or unreadable individual source is counted as rejected instead.
pub fn run(config: &Config) -> io::Result<Report> {
    let roots = Roots::from_config(config)?;
    let runtime = RuntimeBuilder::new_current_thread().enable_time().build()?;
    let mut scan_wall_time = Duration::ZERO;
    let mut hash_audit_wall_time = Duration::ZERO;
    let mut providers = Vec::with_capacity(Provider::ALL.len());
    for provider in Provider::ALL {
        let sources = discover(provider, &roots)?;
        let before_paths = audit_paths(provider, &roots, &sources)?;
        let audit_started = Instant::now();
        let before = fingerprint(&before_paths)?;
        hash_audit_wall_time += audit_started.elapsed();
        let started = Instant::now();
        let mut measurement = measure_provider(
            provider,
            &sources,
            &roots,
            provider_root_note(provider, &roots)?,
            &runtime,
        );
        measurement.scan_time = started.elapsed();
        scan_wall_time += measurement.scan_time;
        let after_sources = discover(provider, &roots)?;
        let after_paths = audit_paths(provider, &roots, &after_sources)?;
        let audit_started = Instant::now();
        let after = fingerprint(&after_paths)?;
        hash_audit_wall_time += audit_started.elapsed();
        measurement.sources_unchanged = before == after;
        providers.push(measurement);
    }
    Ok(Report {
        providers,
        scan_wall_time,
        hash_audit_wall_time,
    })
}

/// Render only aggregate values. The text is safe to paste into a ticket.
#[must_use]
pub fn render_report(report: &Report) -> String {
    let mut output = String::from(
        "provider\tcandidate_sources\taccepted_sources\tcatalog_sessions\tlegacy_distinct_session_ids_reference\tlegacy_prompt_records_reference\trejected_or_partial_sources\tlargest_candidate_source_bytes\tscan_ms\timmutable\tnote\n",
    );
    for value in &report.providers {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            value.provider.name(),
            value.candidate_sources,
            value.accepted_sources,
            value.catalog_sessions,
            value.reference_legacy_session_ids,
            value.reference_legacy_prompt_records,
            value.rejected_or_partial_sources,
            value.largest_candidate_source_bytes,
            value.scan_time.as_millis(),
            value.sources_unchanged,
            value.note.unwrap_or("-"),
        );
    }
    let _ = writeln!(
        output,
        "sequential_scan_wall_time_ms\t{}",
        report.scan_wall_time.as_millis()
    );
    let _ = writeln!(
        output,
        "hash_audit_wall_time_ms\t{}",
        report.hash_audit_wall_time.as_millis()
    );
    output
}

#[derive(Debug)]
struct Roots {
    codex: PathBuf,
    claude: PathBuf,
    opencode_databases: Vec<PathBuf>,
    gemini: PathBuf,
    cursor: PathBuf,
    kimi: PathBuf,
}

impl Roots {
    fn from_config(config: &Config) -> io::Result<Self> {
        let home = config.home.clone().or_else(home_directory).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable")
        })?;
        let codex = config
            .codex_home
            .clone()
            .or_else(|| env::var_os("CODEX_HOME").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".codex"));
        let kimi = config
            .kimi_home
            .clone()
            .or_else(|| env::var_os("KIMI_CODE_HOME").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".kimi-code"));
        let xdg_data_home = config
            .xdg_data_home
            .clone()
            .or_else(|| env::var_os("XDG_DATA_HOME").map(PathBuf::from));
        let opencode_databases = match xdg_data_home {
            Some(path) => vec![path.join("opencode/opencode.db")],
            None => vec![
                home.join(".local/share/opencode/opencode.db"),
                home.join("Library/Application Support/opencode/opencode.db"),
            ],
        };
        Ok(Self {
            codex,
            claude: home.join(".claude/projects"),
            opencode_databases,
            gemini: home.join(".gemini/tmp"),
            cursor: home.join(".cursor/projects"),
            kimi,
        })
    }
}

fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn discover(provider: Provider, roots: &Roots) -> io::Result<Vec<Source>> {
    let mut sources = match provider {
        Provider::Codex => {
            let mut values = files_with_extension(&roots.codex.join("sessions"), "jsonl")?
                .into_iter()
                .map(|path| Source {
                    path,
                    kind: SourceKind::CodexCurrent,
                })
                .collect::<Vec<_>>();
            let legacy = roots.codex.join("history.jsonl");
            if is_regular_file(&legacy)? {
                values.push(Source {
                    path: legacy,
                    kind: SourceKind::CodexLegacy,
                });
            }
            values
        }
        Provider::Claude => files_with_extension(&roots.claude, "jsonl")?
            .into_iter()
            .map(|path| Source {
                path,
                kind: SourceKind::ClaudeJsonl,
            })
            .collect(),
        Provider::OpenCode => roots
            .opencode_databases
            .iter()
            .filter_map(|path| match is_regular_file(path) {
                Ok(true) => Some(Ok(Source {
                    path: path.clone(),
                    kind: SourceKind::OpenCodeDatabase,
                })),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<io::Result<Vec<_>>>()?,
        Provider::Gemini => files_with_extension(&roots.gemini, "jsonl")?
            .into_iter()
            .map(|path| Source {
                path,
                kind: SourceKind::GeminiJsonl,
            })
            .collect(),
        Provider::Cursor => files_with_extension(&roots.cursor, "jsonl")?
            .into_iter()
            .map(|path| Source {
                path,
                kind: SourceKind::CursorJsonl,
            })
            .collect(),
        Provider::Kimi => kimi_wire_files(&roots.kimi)?,
    };
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(sources)
}

fn measure_provider(
    provider: Provider,
    sources: &[Source],
    roots: &Roots,
    empty_note: Option<&'static str>,
    runtime: &Runtime,
) -> ProviderMeasurement {
    let mut accepted_sources = 0;
    let mut rejected_or_partial_sources = 0;
    let mut catalog_sessions = BTreeSet::new();
    let mut legacy_session_ids = BTreeSet::new();
    let mut legacy_prompt_records = 0;
    let mut directly_counted_sessions = 0;
    let mut largest_candidate_source_bytes = 0;
    let mut note = sources
        .is_empty()
        .then_some(empty_note.unwrap_or("source root not present"));

    for source in sources {
        largest_candidate_source_bytes =
            largest_candidate_source_bytes.max(composite_source_size(source, roots));
        let evaluation = match source.kind {
            SourceKind::OpenCodeDatabase => probe_opencode_source(source, runtime),
            SourceKind::KimiWire => probe_kimi_composite(source, &roots.kimi),
            _ => evaluate_source(source),
        };
        match evaluation {
            Ok(Evaluation::Accepted { catalog_identity }) => {
                accepted_sources += 1;
                if let Some(identity) = catalog_identity {
                    catalog_sessions.insert(source_scoped_catalog_key(source, &identity));
                }
            }
            Ok(Evaluation::AcceptedCount { catalog_sessions }) => {
                accepted_sources += 1;
                directly_counted_sessions += catalog_sessions;
            }
            Ok(Evaluation::PartialLegacy {
                session_ids,
                prompt_records,
            }) => {
                rejected_or_partial_sources += 1;
                legacy_session_ids.extend(session_ids);
                legacy_prompt_records += prompt_records;
            }
            Ok(Evaluation::Rejected) | Err(()) => rejected_or_partial_sources += 1,
        }
    }

    let catalog_sessions = catalog_sessions.len() + directly_counted_sessions;
    if rejected_or_partial_sources > 0 && note.is_none() {
        note = Some("one or more sources were rejected or partial");
    }
    ProviderMeasurement {
        provider,
        candidate_sources: sources.len(),
        accepted_sources,
        catalog_sessions,
        reference_legacy_session_ids: legacy_session_ids.len(),
        reference_legacy_prompt_records: legacy_prompt_records,
        rejected_or_partial_sources,
        largest_candidate_source_bytes,
        scan_time: Duration::ZERO,
        sources_unchanged: false,
        note,
    }
}

fn source_scoped_catalog_key(source: &Source, native_identity: &str) -> String {
    format!("{}\u{0}{native_identity}", source.path.to_string_lossy())
}

fn composite_source_size(source: &Source, roots: &Roots) -> u64 {
    let mut paths = vec![source.path.clone()];
    match source.kind {
        SourceKind::OpenCodeDatabase => {
            paths.extend(
                ["-wal", "-shm"]
                    .into_iter()
                    .map(|suffix| PathBuf::from(format!("{}{}", source.path.display(), suffix))),
            );
        }
        SourceKind::KimiWire => {
            paths.push(roots.kimi.join("session_index.jsonl"));
            if let Some(session_directory) = kimi_session_directory(&source.path) {
                paths.push(session_directory.join("state.json"));
            }
        }
        _ => {}
    }
    paths.into_iter().fold(0, |total, path| {
        total.saturating_add(fs::metadata(path).map_or(0, |metadata| metadata.len()))
    })
}

fn provider_root_note(provider: Provider, roots: &Roots) -> io::Result<Option<&'static str>> {
    let roots = match provider {
        Provider::Codex => vec![roots.codex.clone()],
        Provider::Claude => vec![roots.claude.clone()],
        Provider::OpenCode => roots
            .opencode_databases
            .iter()
            .filter_map(|path| path.parent().map(Path::to_owned))
            .collect(),
        Provider::Gemini => vec![roots.gemini.clone()],
        Provider::Cursor => vec![roots.cursor.clone()],
        Provider::Kimi => vec![roots.kimi.clone()],
    };
    for root in roots {
        match fs::symlink_metadata(root) {
            Ok(_) => return Ok(Some("source root present but no candidate source")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Some("source root absent"))
}

enum Evaluation {
    Accepted {
        catalog_identity: Option<String>,
    },
    AcceptedCount {
        catalog_sessions: usize,
    },
    PartialLegacy {
        session_ids: BTreeSet<String>,
        prompt_records: usize,
    },
    Rejected,
}

fn evaluate_source(source: &Source) -> Result<Evaluation, ()> {
    match source.kind {
        SourceKind::OpenCodeDatabase | SourceKind::KimiWire => Ok(Evaluation::Rejected),
        SourceKind::CodexLegacy => {
            let records = read_jsonl(&source.path).map_err(|_| ())?;
            if records.malformed > 0 {
                return Ok(Evaluation::Rejected);
            }
            Ok(Evaluation::PartialLegacy {
                session_ids: records.legacy_session_ids,
                prompt_records: records.legacy_prompt_records,
            })
        }
        SourceKind::CodexCurrent => {
            let records = read_jsonl(&source.path).map_err(|_| ())?;
            let Some(id) = records.codex_session_id else {
                return Ok(Evaluation::Rejected);
            };
            (records.malformed == 0 && records.records > 0)
                .then_some(Evaluation::Accepted {
                    catalog_identity: records
                        .visible_conversations
                        .contains(&ConversationFormat::Codex)
                        .then_some(id),
                })
                .ok_or(())
        }
        SourceKind::ClaudeJsonl => {
            let records = read_jsonl(&source.path).map_err(|_| ())?;
            (records.malformed == 0 && records.claude_session_id.is_some())
                .then_some(Evaluation::Accepted {
                    catalog_identity: records
                        .visible_conversations
                        .contains(&ConversationFormat::Claude)
                        .then_some(records.claude_session_id)
                        .flatten(),
                })
                .ok_or(())
        }
        SourceKind::GeminiJsonl => {
            let records = read_jsonl(&source.path).map_err(|_| ())?;
            (records.malformed == 0 && records.gemini_header_session_id.is_some())
                .then_some(Evaluation::Accepted {
                    catalog_identity: records
                        .visible_conversations
                        .contains(&ConversationFormat::Gemini)
                        .then_some(records.gemini_header_session_id)
                        .flatten(),
                })
                .ok_or(())
        }
        SourceKind::CursorJsonl => {
            let records = read_jsonl(&source.path).map_err(|_| ())?;
            (records.malformed == 0
                && records.cursor_native_shape
                && cursor_path_identity(&source.path).is_some())
            .then_some(Evaluation::Accepted {
                catalog_identity: records
                    .visible_conversations
                    .contains(&ConversationFormat::Cursor)
                    .then(|| cursor_path_identity(&source.path))
                    .flatten(),
            })
            .ok_or(())
        }
    }
}

#[derive(Default)]
struct JsonlRecords {
    records: usize,
    malformed: usize,
    codex_session_id: Option<String>,
    claude_session_id: Option<String>,
    gemini_header_session_id: Option<String>,
    visible_conversations: BTreeSet<ConversationFormat>,
    cursor_native_shape: bool,
    kimi_visible_conversation: bool,
    kimi_index_entries: BTreeSet<(String, String)>,
    legacy_session_ids: BTreeSet<String>,
    legacy_prompt_records: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ConversationFormat {
    Codex,
    Claude,
    Gemini,
    Cursor,
}

fn read_jsonl(path: &Path) -> io::Result<JsonlRecords> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut records = JsonlRecords::default();
    let mut line = Vec::with_capacity(8192);
    let mut chunk = [0_u8; 8192];
    let mut overlong = false;
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            if !line.is_empty() || overlong {
                process_jsonl_record(&mut records, &line, overlong);
            }
            return Ok(records);
        }
        for byte in &chunk[..read] {
            if *byte == b'\n' {
                process_jsonl_record(&mut records, &line, overlong);
                line.clear();
                overlong = false;
            } else if line.len() < MAX_JSONL_RECORD_BYTES {
                line.push(*byte);
            } else {
                overlong = true;
            }
        }
    }
}

fn process_jsonl_record(records: &mut JsonlRecords, line: &[u8], overlong: bool) {
    if overlong {
        records.malformed += 1;
        return;
    }
    match std::str::from_utf8(line)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    {
        Some(value) => {
            records.records += 1;
            if value.get("type").and_then(Value::as_str) == Some("session_meta") {
                records.codex_session_id = value
                    .pointer("/payload/id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
            }
            if codex_visible_conversation(&value) {
                records
                    .visible_conversations
                    .insert(ConversationFormat::Codex);
            }
            if claude_visible_conversation(&value) {
                records
                    .visible_conversations
                    .insert(ConversationFormat::Claude);
            }
            if gemini_visible_conversation(&value) {
                records
                    .visible_conversations
                    .insert(ConversationFormat::Gemini);
            }
            if let Some(session_id) = value.get("sessionId").and_then(Value::as_str) {
                if !session_id.trim().is_empty() {
                    records.claude_session_id = Some(session_id.to_owned());
                    if records.records == 1 {
                        records.gemini_header_session_id = Some(session_id.to_owned());
                    }
                }
            }
            if let (Some(session_id), Some(session_dir)) = (
                value.get("sessionId").and_then(Value::as_str),
                value.get("sessionDir").and_then(Value::as_str),
            ) {
                if !session_id.trim().is_empty() && !session_dir.trim().is_empty() {
                    records
                        .kimi_index_entries
                        .insert((session_id.to_owned(), session_dir.to_owned()));
                }
            }
            if cursor_visible_conversation(&value) {
                records
                    .visible_conversations
                    .insert(ConversationFormat::Cursor);
            }
            records.cursor_native_shape |= matches!(
                value.get("role").and_then(Value::as_str),
                Some("user" | "assistant")
            ) && value.get("message").is_some_and(Value::is_object);
            records.kimi_visible_conversation |= kimi_visible_conversation(&value);
            if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
                records.legacy_session_ids.insert(session_id.to_owned());
            }
            if value.get("text").and_then(Value::as_str).is_some() {
                records.legacy_prompt_records += 1;
            }
        }
        None => records.malformed += 1,
    }
}

fn codex_visible_conversation(value: &Value) -> bool {
    let record_type = value.get("type").and_then(Value::as_str);
    let payload = value.get("payload").unwrap_or(&Value::Null);
    match (record_type, payload.get("type").and_then(Value::as_str)) {
        (Some("response_item"), Some("message"))
            if matches!(
                payload.get("role").and_then(Value::as_str),
                Some("user" | "assistant")
            ) =>
        {
            payload
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    parts.iter().any(|part| {
                        matches!(
                            part.get("type").and_then(Value::as_str),
                            Some("input_text" | "output_text" | "text")
                        ) && nonempty_string(part.get("text"))
                    })
                })
        }
        (Some("event_msg"), Some("user_message" | "agent_message")) => {
            nonempty_string(payload.get("message"))
        }
        _ => false,
    }
}

fn claude_visible_conversation(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("user" | "assistant")
    ) && visible_content(value.pointer("/message/content"))
}

fn gemini_visible_conversation(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("user" | "gemini")
    ) && visible_content(value.get("content"))
}

fn cursor_visible_conversation(value: &Value) -> bool {
    matches!(
        value.get("role").and_then(Value::as_str),
        Some("user" | "assistant")
    ) && visible_content(value.pointer("/message/content"))
}

fn visible_content(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(parts)) => parts.iter().any(|part| {
            nonempty_string(part.get("text"))
                || nonempty_string(part.get("content"))
                || part.as_str().is_some_and(|text| !text.trim().is_empty())
        }),
        Some(Value::Object(object)) => nonempty_string(object.get("text")),
        _ => false,
    }
}

fn nonempty_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
}

fn kimi_visible_conversation(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        Some("turn.prompt") => visible_content(value.get("input")),
        Some("context.append_message") => value.get("message").is_some_and(|message| {
            matches!(
                message.get("role").and_then(Value::as_str),
                Some("user" | "assistant")
            ) && (nonempty_string(message.get("text")) || visible_content(message.get("content")))
        }),
        _ => false,
    }
}

fn scan_bytes(path: &Path) -> io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
    }
}

fn probe_opencode_source(source: &Source, runtime: &Runtime) -> Result<Evaluation, ()> {
    scan_bytes(&source.path).map_err(|_| ())?;
    let catalog_sessions = runtime.block_on(probe_opencode_database(&source.path))?;
    Ok(Evaluation::AcceptedCount { catalog_sessions })
}

async fn probe_opencode_database(path: &Path) -> Result<usize, ()> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false);
    let pool: SqlitePool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| ())?;
    let mut transaction = pool.begin().await.map_err(|_| ())?;
    let mut session_ids = BTreeSet::new();
    {
        let mut sessions = sqlx::query("SELECT id, model FROM session").fetch(&mut *transaction);
        while let Some(row) = sessions.try_next().await.map_err(|_| ())? {
            let id: String = row.try_get(0).map_err(|_| ())?;
            let model: Option<String> = row.try_get(1).map_err(|_| ())?;
            bounded_nonempty(&id, 512)?;
            if let Some(model) = model.as_deref() {
                bounded_json(model)?;
            }
            if !session_ids.insert(id) {
                return Err(());
            }
        }
    }
    let mut message_ids = BTreeSet::new();
    let mut visible_sessions = BTreeSet::new();
    {
        let mut messages =
            sqlx::query("SELECT id, session_id, data FROM message").fetch(&mut *transaction);
        while let Some(row) = messages.try_next().await.map_err(|_| ())? {
            let id: String = row.try_get(0).map_err(|_| ())?;
            let session_id: String = row.try_get(1).map_err(|_| ())?;
            let data: String = row.try_get(2).map_err(|_| ())?;
            bounded_nonempty(&id, 512)?;
            if !session_ids.contains(&session_id) || !message_ids.insert(id) {
                return Err(());
            }
            bounded_json(&data)?;
        }
    }
    {
        let mut parts = sqlx::query("SELECT id, message_id, session_id, data FROM part")
            .fetch(&mut *transaction);
        while let Some(row) = parts.try_next().await.map_err(|_| ())? {
            let id: String = row.try_get(0).map_err(|_| ())?;
            let message_id: String = row.try_get(1).map_err(|_| ())?;
            let session_id: String = row.try_get(2).map_err(|_| ())?;
            let data: String = row.try_get(3).map_err(|_| ())?;
            bounded_nonempty(&id, 512)?;
            if !message_ids.contains(&message_id) || !session_ids.contains(&session_id) {
                return Err(());
            }
            let part = bounded_json(&data)?;
            if opencode_visible_part(&part) {
                visible_sessions.insert(session_id);
            }
        }
    }
    transaction.rollback().await.map_err(|_| ())?;
    pool.close().await;
    Ok(visible_sessions.len())
}

fn bounded_nonempty(value: &str, max_bytes: usize) -> Result<(), ()> {
    (!value.trim().is_empty() && value.len() <= max_bytes)
        .then_some(())
        .ok_or(())
}

fn bounded_json(value: &str) -> Result<Value, ()> {
    (value.len() <= MAX_JSONL_RECORD_BYTES)
        .then_some(())
        .ok_or(())?;
    serde_json::from_str::<Value>(value).map_err(|_| ())
}

fn opencode_visible_part(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        Some("text") => nonempty_string(value.get("text")),
        Some("tool") => nonempty_string(value.get("tool")),
        _ => false,
    }
}

fn probe_kimi_composite(source: &Source, root: &Path) -> Result<Evaluation, ()> {
    let parts = kimi_wire_parts(root, &source.path).ok_or(())?;
    let index = read_jsonl(&root.join("session_index.jsonl")).map_err(|_| ())?;
    if index.malformed > 0
        || !index
            .kimi_index_entries
            .iter()
            .any(|(session_id, session_dir)| {
                session_id == &parts.session_id
                    && kimi_index_path(root, session_dir)
                        .is_some_and(|path| path == parts.session_dir)
            })
    {
        return Err(());
    }
    let state = read_bounded_json_file(&parts.session_dir.join("state.json"))?;
    let agents = state.get("agents").and_then(Value::as_object).ok_or(())?;
    if !agents.contains_key("main") || !agents.contains_key(&parts.agent_id) {
        return Err(());
    }
    let wire = read_jsonl(&source.path).map_err(|_| ())?;
    (wire.malformed == 0 && wire.records > 0)
        .then_some(Evaluation::Accepted {
            catalog_identity: wire
                .kimi_visible_conversation
                .then(|| format!("{}:{}", parts.session_id, parts.agent_id)),
        })
        .ok_or(())
}

fn read_bounded_json_file(path: &Path) -> Result<Value, ()> {
    let bytes = fs::read(path).map_err(|_| ())?;
    if bytes.len() > MAX_JSONL_RECORD_BYTES {
        return Err(());
    }
    serde_json::from_slice(&bytes).map_err(|_| ())
}

struct KimiWireParts {
    session_dir: PathBuf,
    session_id: String,
    agent_id: String,
}

fn kimi_wire_parts(root: &Path, wire: &Path) -> Option<KimiWireParts> {
    let relative = wire.strip_prefix(root).ok()?;
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    if components.len() != 6
        || components[0] != "sessions"
        || components[3] != "agents"
        || components[5] != "wire.jsonl"
    {
        return None;
    }
    let session_dir = root
        .join(components[0])
        .join(components[1])
        .join(components[2]);
    let canonical = session_dir.canonicalize().ok()?;
    canonical.starts_with(root).then_some(KimiWireParts {
        session_dir: canonical,
        session_id: components[2].to_owned(),
        agent_id: components[4].to_owned(),
    })
}

fn kimi_index_path(root: &Path, session_dir: &str) -> Option<PathBuf> {
    let candidate = Path::new(session_dir);
    let path = if candidate.is_absolute() {
        candidate.to_owned()
    } else {
        root.join(candidate)
    };
    let canonical = path.canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}

fn audit_paths(provider: Provider, roots: &Roots, sources: &[Source]) -> io::Result<Vec<PathBuf>> {
    let mut paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    match provider {
        Provider::Codex => paths.extend([roots.codex.clone(), roots.codex.join("sessions")]),
        Provider::Claude => paths.push(roots.claude.clone()),
        Provider::OpenCode => paths.extend(
            roots
                .opencode_databases
                .iter()
                .filter_map(|path| path.parent().map(Path::to_owned)),
        ),
        Provider::Gemini => paths.push(roots.gemini.clone()),
        Provider::Cursor => paths.push(roots.cursor.clone()),
        Provider::Kimi => paths.extend([roots.kimi.clone(), roots.kimi.join("sessions")]),
    }
    for source in sources {
        if let Some(parent) = source.path.parent() {
            paths.push(parent.to_owned());
        }
    }
    match provider {
        Provider::OpenCode => {
            for source in sources {
                for suffix in ["-wal", "-shm"] {
                    let sidecar = PathBuf::from(format!("{}{}", source.path.display(), suffix));
                    if is_regular_file(&sidecar)? {
                        paths.push(sidecar);
                    }
                }
            }
        }
        Provider::Kimi => {
            let index = roots.kimi.join("session_index.jsonl");
            if is_regular_file(&index)? {
                paths.push(index);
            }
            for source in sources {
                if let Some(session_directory) = kimi_session_directory(&source.path) {
                    let state = session_directory.join("state.json");
                    if is_regular_file(&state)? {
                        paths.push(state);
                    }
                }
            }
        }
        _ => {}
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn fingerprint(paths: &[PathBuf]) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    for path in paths {
        digest.update(path.as_os_str().as_encoded_bytes());
        digest.update([0]);
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                digest.update(b"present");
                update_metadata_digest(&mut digest, &metadata);
                if metadata.is_file() && !metadata.file_type().is_symlink() {
                    let mut file_digest = Sha256::new();
                    let mut reader = BufReader::new(File::open(path)?);
                    let mut buffer = vec![0_u8; 64 * 1024];
                    loop {
                        let read = reader.read(&mut buffer)?;
                        if read == 0 {
                            break;
                        }
                        file_digest.update(&buffer[..read]);
                    }
                    digest.update(file_digest.finalize());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => digest.update(b"absent"),
            Err(error) => return Err(error),
        }
    }
    Ok(digest.finalize().into())
}

fn update_metadata_digest(digest: &mut Sha256, metadata: &fs::Metadata) {
    digest.update(metadata.len().to_le_bytes());
    digest.update([u8::from(metadata.is_file()), u8::from(metadata.is_dir())]);
    if let Ok(modified) = metadata.modified() {
        if let Ok(elapsed) = modified.duration_since(std::time::UNIX_EPOCH) {
            digest.update(elapsed.as_secs().to_le_bytes());
            digest.update(elapsed.subsec_nanos().to_le_bytes());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        digest.update(metadata.ino().to_le_bytes());
        digest.update(metadata.mode().to_le_bytes());
    }
}

fn files_with_extension(root: &Path, extension: &str) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    visit_files(root, &mut |path| {
        if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            paths.push(path.to_owned());
        }
        Ok(())
    })?;
    Ok(paths)
}

fn kimi_wire_files(root: &Path) -> io::Result<Vec<Source>> {
    let mut sources = Vec::new();
    visit_files(&root.join("sessions"), &mut |path| {
        if path.file_name().and_then(|value| value.to_str()) == Some("wire.jsonl") {
            sources.push(Source {
                path: path.to_owned(),
                kind: SourceKind::KimiWire,
            });
        }
        Ok(())
    })?;
    Ok(sources)
}

fn visit_files(root: &Path, visit: &mut dyn FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        return visit(root);
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            visit_files(&path, visit)?;
        } else if metadata.is_file() {
            visit(&path)?;
        }
    }
    Ok(())
}

fn is_regular_file(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn cursor_path_identity(path: &Path) -> Option<String> {
    let parent = path.parent()?.file_name()?.to_str()?;
    (!parent.trim().is_empty()).then_some(parent.to_owned())
}

fn kimi_session_directory(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent()?;
    while current.file_name().and_then(|name| name.to_str()) != Some("sessions") {
        if current.join("agents").is_dir() {
            return Some(current.to_owned());
        }
        current = current.parent()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    use tempfile::TempDir;

    #[test]
    fn measures_six_fixture_providers_without_mutation_or_private_output() {
        let home = test_tempdir();
        write_jsonl(
            &home.path().join(".codex/sessions/a/current.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"codex-current-private"}}"#,
        );
        append_jsonl(
            &home.path().join(".codex/sessions/a/current.jsonl"),
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"private prompt"}}"#,
        );
        write_jsonl(
            &home.path().join(".codex/history.jsonl"),
            r#"{"session_id":"codex-legacy-private","text":"private prompt"}"#,
        );
        write_jsonl(
            &home.path().join(".claude/projects/w/session.jsonl"),
            r#"{"type":"user","sessionId":"claude-private","message":{"content":"private prompt"}}"#,
        );
        write_jsonl(
            &home.path().join(".gemini/tmp/w/chats/a.jsonl"),
            r#"{"sessionId":"gemini-private"}"#,
        );
        append_jsonl(
            &home.path().join(".gemini/tmp/w/chats/a.jsonl"),
            r#"{"id":"user-private","type":"user","content":[{"text":"private prompt"}]}"#,
        );
        write_jsonl(
            &home
                .path()
                .join(".cursor/projects/w/cursor-private/transcript.jsonl"),
            r#"{"role":"user","message":{"content":"private prompt"}}"#,
        );
        write_jsonl(
            &home
                .path()
                .join(".kimi-code/sessions/w/kimi-private/agents/a/wire.jsonl"),
            r#"{"type":"turn.prompt","input":[{"text":"private prompt"}]}"#,
        );
        write_jsonl(
            &home.path().join(".kimi-code/session_index.jsonl"),
            r#"{"sessionId":"kimi-private","sessionDir":"sessions/w/kimi-private"}"#,
        );
        write_jsonl(
            &home
                .path()
                .join(".kimi-code/sessions/w/kimi-private/state.json"),
            r#"{"agents":{"main":{},"a":{"parentAgentId":"main"}}}"#,
        );
        create_opencode_db(&home.path().join(".local/share/opencode/opencode.db"), 2);

        let before = full_tree_digest(home.path());
        let report = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("measurement succeeds");
        let after = full_tree_digest(home.path());

        assert_eq!(
            before, after,
            "the harness must not mutate provider sources"
        );
        assert_eq!(provider(&report, Provider::Codex).candidate_sources, 2);
        assert_eq!(provider(&report, Provider::Codex).accepted_sources, 1);
        assert_eq!(provider(&report, Provider::Codex).catalog_sessions, 1);
        assert_eq!(
            provider(&report, Provider::Codex).reference_legacy_session_ids,
            1
        );
        assert_eq!(
            provider(&report, Provider::Codex).reference_legacy_prompt_records,
            1
        );
        assert_eq!(provider(&report, Provider::Claude).accepted_sources, 1);
        assert_eq!(provider(&report, Provider::Gemini).accepted_sources, 1);
        assert_eq!(provider(&report, Provider::Cursor).accepted_sources, 1);
        assert_eq!(provider(&report, Provider::Kimi).accepted_sources, 1);
        assert_eq!(provider(&report, Provider::Kimi).catalog_sessions, 1);
        assert_eq!(provider(&report, Provider::OpenCode).catalog_sessions, 2);
        assert!(
            report
                .providers
                .iter()
                .all(|result| result.sources_unchanged)
        );
        let output = render_report(&report);
        for forbidden in [
            "private prompt",
            "codex-current-private",
            "kimi-private",
            home.path().to_str().unwrap_or(""),
        ] {
            assert!(
                !output.contains(forbidden),
                "report must not contain private material"
            );
        }
        assert!(!output.contains("SHA-256"));
    }

    #[test]
    fn symlinked_external_root_is_not_followed() {
        let home = test_tempdir();
        let external = test_tempdir();
        write_jsonl(
            &external.path().join("session.jsonl"),
            r#"{"x":"private prompt"}"#,
        );
        #[cfg(unix)]
        {
            fs::create_dir_all(home.path().join(".claude")).expect("create symlink parent");
            std::os::unix::fs::symlink(external.path(), home.path().join(".claude/projects"))
                .expect("create root symlink");
        }

        let report = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("symlinked root is ignored");
        assert_eq!(provider(&report, Provider::Claude).candidate_sources, 0);
    }

    #[test]
    fn distinguishes_absent_and_present_but_empty_roots() {
        let home = test_tempdir();
        let absent = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("absent root measurement");
        assert_eq!(
            provider(&absent, Provider::Claude).note,
            Some("source root absent")
        );

        fs::create_dir_all(home.path().join(".claude/projects")).expect("create empty root");
        let empty = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("empty root measurement");
        assert_eq!(
            provider(&empty, Provider::Claude).note,
            Some("source root present but no candidate source")
        );
    }

    #[test]
    fn identity_only_records_are_not_catalog_sessions() {
        let home = test_tempdir();
        write_jsonl(
            &home.path().join(".codex/sessions/a/current.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"identity-only"}}"#,
        );
        write_jsonl(
            &home.path().join(".claude/projects/w/session.jsonl"),
            r#"{"sessionId":"identity-only"}"#,
        );
        write_jsonl(
            &home.path().join(".gemini/tmp/w/chats/a.jsonl"),
            r#"{"sessionId":"identity-only"}"#,
        );
        write_jsonl(
            &home
                .path()
                .join(".cursor/projects/w/session/transcript.jsonl"),
            r#"{"role":"user","message":{"content":[]}}"#,
        );

        let report = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("identity-only measurement");

        for provider_name in [
            Provider::Codex,
            Provider::Claude,
            Provider::Gemini,
            Provider::Cursor,
        ] {
            let measurement = provider(&report, provider_name);
            assert_eq!(measurement.catalog_sessions, 0);
            assert_eq!(measurement.accepted_sources, 1);
            assert_eq!(measurement.rejected_or_partial_sources, 0);
        }
    }

    #[test]
    fn valid_opencode_schema_without_visible_parts_is_not_catalogued() {
        let home = test_tempdir();
        let database = home.path().join(".local/share/opencode/opencode.db");
        create_opencode_db(&database, 1);
        remove_opencode_visible_parts(&database);

        let report = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("valid non-conversation database");
        let opencode = provider(&report, Provider::OpenCode);
        assert_eq!(opencode.candidate_sources, 1);
        assert_eq!(opencode.accepted_sources, 1);
        assert_eq!(opencode.catalog_sessions, 0);
        assert_eq!(opencode.rejected_or_partial_sources, 0);
    }

    #[test]
    fn catalog_identity_is_scoped_to_its_source() {
        let home = test_tempdir();
        for path in [
            home.path().join(".claude/projects/a/one.jsonl"),
            home.path().join(".claude/projects/b/two.jsonl"),
        ] {
            write_jsonl(
                &path,
                r#"{"type":"user","sessionId":"same-native-id","message":{"content":"visible"}}"#,
            );
        }
        for path in [
            home.path()
                .join(".cursor/projects/a/one/shared/transcript.jsonl"),
            home.path()
                .join(".cursor/projects/b/two/shared/transcript.jsonl"),
        ] {
            write_jsonl(&path, r#"{"role":"user","message":{"content":"visible"}}"#);
        }
        for work_dir in ["a", "b"] {
            let session_dir = home
                .path()
                .join(format!(".kimi-code/sessions/{work_dir}/same-session"));
            write_jsonl(
                &session_dir.join("agents/main/wire.jsonl"),
                r#"{"type":"turn.prompt","input":[{"text":"visible"}]}"#,
            );
            write_jsonl(&session_dir.join("state.json"), r#"{"agents":{"main":{}}}"#);
        }
        write_jsonl(
            &home.path().join(".kimi-code/session_index.jsonl"),
            r#"{"sessionId":"same-session","sessionDir":"sessions/a/same-session"}"#,
        );
        append_jsonl(
            &home.path().join(".kimi-code/session_index.jsonl"),
            r#"{"sessionId":"same-session","sessionDir":"sessions/b/same-session"}"#,
        );

        let report = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("source-scoped identities");

        assert_eq!(provider(&report, Provider::Claude).catalog_sessions, 2);
        assert_eq!(provider(&report, Provider::Cursor).catalog_sessions, 2);
        assert_eq!(provider(&report, Provider::Kimi).catalog_sessions, 2);
    }

    #[test]
    fn composite_and_database_failures_are_isolated() {
        let home = test_tempdir();
        write_jsonl(
            &home
                .path()
                .join(".kimi-code/sessions/w/session-a/agents/main/wire.jsonl"),
            r#"{"type":"event"}"#,
        );
        write_jsonl(
            &home.path().join(".kimi-code/session_index.jsonl"),
            r#"{"sessionId":"different-session","sessionDir":"sessions/w/session-a"}"#,
        );
        write_jsonl(
            &home
                .path()
                .join(".kimi-code/sessions/w/session-a/state.json"),
            r#"{"agents":{"main":{}}}"#,
        );

        let valid = home.path().join(".local/share/opencode/opencode.db");
        let invalid = home
            .path()
            .join("Library/Application Support/opencode/opencode.db");
        create_opencode_db(&valid, 1);
        create_opencode_db(&invalid, 1);
        corrupt_opencode_message_json(&invalid);

        let report = run(&Config {
            home: Some(home.path().to_owned()),
            ..Config::default()
        })
        .expect("isolated source failures");

        let kimi = provider(&report, Provider::Kimi);
        assert_eq!(kimi.candidate_sources, 1);
        assert_eq!(kimi.accepted_sources, 0);
        assert_eq!(kimi.rejected_or_partial_sources, 1);

        let opencode = provider(&report, Provider::OpenCode);
        assert_eq!(opencode.candidate_sources, 2);
        assert_eq!(opencode.accepted_sources, 1);
        assert_eq!(opencode.rejected_or_partial_sources, 1);
        assert_eq!(opencode.catalog_sessions, 1);
    }

    fn provider(report: &Report, wanted: Provider) -> &ProviderMeasurement {
        report
            .providers
            .iter()
            .find(|value| value.provider == wanted)
            .expect("provider result")
    }

    fn write_jsonl(path: &Path, line: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let mut file = File::create(path).expect("create fixture");
        writeln!(file, "{line}").expect("write fixture");
    }

    fn append_jsonl(path: &Path, line: &str) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open fixture");
        writeln!(file, "{line}").expect("append fixture");
    }

    fn create_opencode_db(path: &Path, sessions: usize) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let options = SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true);
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .expect("create database");
            sqlx::query(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    parent_id TEXT,
                    directory TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    agent TEXT,
                    model TEXT
                )",
            )
            .execute(&pool)
            .await
            .expect("create table");
            sqlx::query(
                "CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    data TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .expect("create message table");
            sqlx::query(
                "CREATE TABLE part (
                    id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    data TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .expect("create part table");
            for index in 0..sessions {
                let id = format!("session-{index}");
                let timestamp = i64::try_from(index).expect("fixture index fits i64");
                sqlx::query(
                    "INSERT INTO session
                     (id, directory, time_created, time_updated, model)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(&id)
                .bind("/fixture")
                .bind(timestamp)
                .bind(timestamp)
                .bind(r#"{"id":"fixture-model"}"#)
                .execute(&pool)
                .await
                .expect("insert session");
                let message_id = format!("message-{index}");
                sqlx::query(
                    "INSERT INTO message
                     (id, session_id, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(&message_id)
                .bind(&id)
                .bind(timestamp)
                .bind(timestamp)
                .bind(r#"{"role":"user"}"#)
                .execute(&pool)
                .await
                .expect("insert message");
                sqlx::query(
                    "INSERT INTO part
                     (id, message_id, session_id, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .bind(format!("part-{index}"))
                .bind(&message_id)
                .bind(&id)
                .bind(timestamp)
                .bind(timestamp)
                .bind(r#"{"type":"text","text":"private prompt"}"#)
                .execute(&pool)
                .await
                .expect("insert part");
            }
            pool.close().await;
        });
    }

    fn corrupt_opencode_message_json(path: &Path) {
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let options = SqliteConnectOptions::new().filename(path);
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .expect("open database");
            sqlx::query("UPDATE message SET data = 'not-json'")
                .execute(&pool)
                .await
                .expect("corrupt message data");
            pool.close().await;
        });
    }

    fn remove_opencode_visible_parts(path: &Path) {
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let options = SqliteConnectOptions::new().filename(path);
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .expect("open database");
            sqlx::query("UPDATE part SET data = ?1")
                .bind(r#"{"type":"metadata"}"#)
                .execute(&pool)
                .await
                .expect("remove visible part");
            pool.close().await;
        });
    }

    fn full_tree_digest(root: &Path) -> [u8; 32] {
        let mut paths = Vec::new();
        visit_files(root, &mut |path| {
            paths.push(path.to_owned());
            Ok(())
        })
        .expect("visit files");
        paths.sort();
        fingerprint(&paths).expect("fingerprint")
    }

    fn test_tempdir() -> TempDir {
        tempfile::Builder::new()
            .prefix("taiwalog-m0-test-")
            .tempdir_in(env::current_dir().expect("test current directory"))
            .expect("temporary fixture directory")
    }
}
