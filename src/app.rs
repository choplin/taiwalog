//! The small application workflows available before collectors are installed.

use std::{collections::BTreeMap, path::PathBuf};

use serde::Serialize;

use crate::{
    claude::collect_claude,
    codex::collect_codex,
    cursor::collect_cursor,
    gemini::collect_gemini,
    kimi::collect_kimi,
    opencode::collect_opencode,
    paths::AppPaths,
    storage::{
        CatalogSessionPreview, CatalogSessionSummary, SCHEMA_VERSION, list_catalog_sessions,
        open_database, read_catalog_preview,
    },
};

/// Observable result of a read-only provider scan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScanSummary {
    pub schema_version: i64,
    pub collectors_installed: u8,
    pub sources_refreshed: u64,
    pub sources_partial: u64,
    pub sources_failed: u64,
    pub sessions_available: u64,
    pub provider_summaries: Vec<ProviderScanSummary>,
    pub message: String,
}

/// Aggregate diagnostic counts for one explicit provider collector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderScanSummary {
    pub provider: &'static str,
    pub candidate_sources: u64,
    pub refreshed_sources: u64,
    pub partial_sources: u64,
    pub failed_sources: u64,
    pub sessions_available: u64,
}

/// Initializes the durable catalog and collects read-only provider sources.
///
/// # Errors
///
/// Returns an error when Agentlog-owned storage cannot be created, opened, or
/// queried.
pub async fn scan_shell(paths: &AppPaths) -> anyhow::Result<ScanSummary> {
    paths.ensure_data_dir()?;
    let database = open_database(&paths.database_path).await?;
    let codex_report = collect_codex(&paths.codex_root()?, &database).await?;
    let claude_report = collect_claude(&paths.claude_root()?, &database).await?;
    let opencode_report = collect_opencode(&paths.opencode_roots()?, &database).await?;
    let gemini_report = collect_gemini(&paths.gemini_root()?, &database).await?;
    let cursor_report = collect_cursor(&paths.cursor_root()?, &database).await?;
    let kimi_report = collect_kimi(&paths.kimi_root()?, &database).await?;
    let sessions_available = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
        .fetch_one(&database)
        .await?;
    let provider_session_counts = sqlx::query_as::<_, (String, i64)>(
        "SELECT sources.provider, COUNT(sessions.id)
         FROM sources
         LEFT JOIN sessions ON sessions.source_id = sources.id
         GROUP BY sources.provider",
    )
    .fetch_all(&database)
    .await?
    .into_iter()
    .map(|(provider, count)| Ok((provider, u64::try_from(count)?)))
    .collect::<Result<BTreeMap<_, _>, std::num::TryFromIntError>>()?;
    database.close().await;

    Ok(ScanSummary {
        schema_version: SCHEMA_VERSION,
        collectors_installed: 6,
        sources_refreshed: codex_report.refreshed_sources
            + claude_report.refreshed_sources
            + opencode_report.refreshed_sources
            + gemini_report.refreshed_sources
            + cursor_report.refreshed_sources
            + kimi_report.refreshed_sources,
        sources_partial: codex_report.partial_sources
            + claude_report.partial_sources
            + opencode_report.partial_sources
            + gemini_report.partial_sources
            + cursor_report.partial_sources
            + kimi_report.partial_sources,
        sources_failed: codex_report.failed_sources
            + claude_report.failed_sources
            + opencode_report.failed_sources
            + gemini_report.failed_sources
            + cursor_report.failed_sources
            + kimi_report.failed_sources,
        sessions_available: u64::try_from(sessions_available)?,
        provider_summaries: vec![
            ProviderScanSummary {
                provider: "codex",
                candidate_sources: codex_report.candidate_sources,
                refreshed_sources: codex_report.refreshed_sources,
                partial_sources: codex_report.partial_sources,
                failed_sources: codex_report.failed_sources,
                sessions_available: provider_sessions(&provider_session_counts, "codex"),
            },
            ProviderScanSummary {
                provider: "claude",
                candidate_sources: claude_report.candidate_sources,
                refreshed_sources: claude_report.refreshed_sources,
                partial_sources: claude_report.partial_sources,
                failed_sources: claude_report.failed_sources,
                sessions_available: provider_sessions(&provider_session_counts, "claude"),
            },
            ProviderScanSummary {
                provider: "opencode",
                candidate_sources: opencode_report.candidate_sources,
                refreshed_sources: opencode_report.refreshed_sources,
                partial_sources: opencode_report.partial_sources,
                failed_sources: opencode_report.failed_sources,
                sessions_available: provider_sessions(&provider_session_counts, "opencode"),
            },
            ProviderScanSummary {
                provider: "gemini",
                candidate_sources: gemini_report.candidate_sources,
                refreshed_sources: gemini_report.refreshed_sources,
                partial_sources: gemini_report.partial_sources,
                failed_sources: gemini_report.failed_sources,
                sessions_available: provider_sessions(&provider_session_counts, "gemini"),
            },
            ProviderScanSummary {
                provider: "cursor",
                candidate_sources: cursor_report.candidate_sources,
                refreshed_sources: cursor_report.refreshed_sources,
                partial_sources: cursor_report.partial_sources,
                failed_sources: cursor_report.failed_sources,
                sessions_available: provider_sessions(&provider_session_counts, "cursor"),
            },
            ProviderScanSummary {
                provider: "kimi",
                candidate_sources: kimi_report.candidate_sources,
                refreshed_sources: kimi_report.refreshed_sources,
                partial_sources: kimi_report.partial_sources,
                failed_sources: kimi_report.failed_sources,
                sessions_available: provider_sessions(&provider_session_counts, "kimi"),
            },
        ],
        message: "Provider sources were read without modifying provider-owned files".to_owned(),
    })
}

fn provider_sessions(counts: &BTreeMap<String, u64>, provider: &str) -> u64 {
    counts.get(provider).copied().unwrap_or(0)
}

/// Reads catalog session summaries without starting a provider scan.
///
/// # Errors
///
/// Returns an error when Agentlog-owned storage cannot be opened or queried.
pub async fn list_shell(
    paths: &AppPaths,
    limit: u32,
) -> anyhow::Result<Vec<CatalogSessionSummary>> {
    paths.ensure_data_dir()?;
    let database = open_database(&paths.database_path).await?;
    let sessions = list_catalog_sessions(&database, limit).await?;
    database.close().await;
    Ok(sessions)
}

/// Reads one catalog preview without starting a provider scan.
///
/// # Errors
///
/// Returns an error when the selected session is unavailable or the Agentlog
/// catalog cannot be opened.
pub async fn show_shell(
    paths: &AppPaths,
    session_id: i64,
) -> anyhow::Result<CatalogSessionPreview> {
    paths.ensure_data_dir()?;
    let database = open_database(&paths.database_path).await?;
    let session = read_catalog_preview(&database, session_id, 80, 4 * 1024).await?;
    database.close().await;
    Ok(session)
}

/// Paths shown by the diagnostic command. All paths except provider sources are
/// owned by Agentlog. Collectors will report provider source paths later.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PathsReport {
    pub agentlog_home: PathBuf,
    pub config_file: PathBuf,
    pub data_directory: PathBuf,
    pub database: PathBuf,
    pub provider_sources: String,
}

#[must_use]
pub fn paths_report(paths: &AppPaths) -> PathsReport {
    PathsReport {
        agentlog_home: paths.home().to_path_buf(),
        config_file: paths.config_path.clone(),
        data_directory: paths.data_dir.clone(),
        database: paths.database_path.clone(),
        provider_sources: format!(
            "Codex root (read-only): {}; Claude root (read-only): {}; OpenCode roots (read-only): {}; Gemini root (read-only): {}; Cursor root (read-only): {}; Kimi root (read-only): {}",
            paths.codex_root().map_or_else(
                |_| "unavailable".to_owned(),
                |path| path.display().to_string()
            ),
            paths.claude_root().map_or_else(
                |_| "unavailable".to_owned(),
                |path| path.display().to_string()
            ),
            paths.opencode_roots().map_or_else(
                |_| "unavailable".to_owned(),
                |paths| paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            paths.gemini_root().map_or_else(
                |_| "unavailable".to_owned(),
                |path| path.display().to_string()
            ),
            paths.cursor_root().map_or_else(
                |_| "unavailable".to_owned(),
                |path| path.display().to_string()
            ),
            paths.kimi_root().map_or_else(
                |_| "unavailable".to_owned(),
                |path| path.display().to_string()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::{
        SqlitePool,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    use tempfile::TempDir;

    use std::fs;

    use crate::{paths::AppPaths, storage::SCHEMA_VERSION};

    use super::scan_shell;

    async fn create_opencode_fixture(path: &std::path::Path) -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await
            .expect("OpenCode fixture database");
        for statement in [
            "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL, model TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL)",
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL)",
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL)",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("fixture schema");
        }
        sqlx::query("INSERT INTO session VALUES ('open-session', '/work', 'model', 1, 1)")
            .execute(&pool)
            .await
            .expect("session");
        sqlx::query(
            "INSERT INTO message VALUES ('message', 'open-session', 1, '{\"role\":\"user\"}')",
        )
        .execute(&pool)
        .await
        .expect("message");
        sqlx::query("INSERT INTO part VALUES ('part', 'message', 'open-session', 1, '{\"type\":\"text\",\"text\":\"open visible\"}')")
            .execute(&pool).await.expect("part");
        pool
    }

    #[tokio::test]
    async fn shell_scan_creates_catalog_and_reports_empty_codex_collection() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        fs::write(
            agentlog_home.join("config.toml"),
            format!(
                "[providers]\ncodex_root = \"{}\"\nclaude_root = \"{}\"\nopencode_root = \"{}\"\ngemini_root = \"{}\"\ncursor_root = \"{}\"\nkimi_root = \"{}\"\n",
                temporary.path().join("empty-codex").display(),
                temporary.path().join("empty-claude").display(),
                temporary.path().join("empty-opencode").display(),
                temporary.path().join("empty-gemini").display(),
                temporary.path().join("empty-cursor").display(),
                temporary.path().join("empty-kimi").display(),
            ),
        )
        .expect("write config");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");

        let summary = scan_shell(&paths).await.expect("run shell scan");

        assert_eq!(summary.schema_version, SCHEMA_VERSION);
        assert_eq!(summary.collectors_installed, 6);
        assert_eq!(summary.sources_refreshed, 0);
        assert_eq!(summary.sources_partial, 0);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.sessions_available, 0);
        assert_eq!(summary.provider_summaries.len(), 6);
        assert!(
            summary
                .provider_summaries
                .iter()
                .all(|summary| summary.candidate_sources == 0)
        );
        assert!(paths.database_path.is_file());
    }

    #[tokio::test]
    async fn a_gemini_failure_does_not_block_a_cursor_source() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        let gemini_root = temporary.path().join("gemini");
        let cursor_root = temporary.path().join("cursor");
        fs::create_dir_all(gemini_root.join("tmp")).expect("Gemini directory");
        fs::create_dir_all(cursor_root.join("projects/project/session")).expect("Cursor directory");
        fs::write(gemini_root.join("tmp/broken.jsonl"), "not JSON\n").expect("Gemini source");
        fs::write(
            cursor_root.join("projects/project/session/capture.jsonl"),
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"visible\"}]}}\n",
        )
        .expect("Cursor source");
        fs::write(
            agentlog_home.join("config.toml"),
            format!(
                "[providers]\ncodex_root = \"{}\"\nclaude_root = \"{}\"\nopencode_root = \"{}\"\ngemini_root = \"{}\"\ncursor_root = \"{}\"\nkimi_root = \"{}\"\n",
                temporary.path().join("empty-codex").display(),
                temporary.path().join("empty-claude").display(),
                temporary.path().join("empty-opencode").display(),
                gemini_root.display(),
                cursor_root.display(),
                temporary.path().join("empty-kimi").display(),
            ),
        )
        .expect("write config");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");

        let summary = scan_shell(&paths).await.expect("run shell scan");

        assert_eq!(summary.sources_refreshed, 1);
        assert_eq!(summary.sources_failed, 1);
        assert_eq!(summary.sessions_available, 1);
        assert_eq!(summary.provider_summaries.len(), 6);
        let json = serde_json::to_value(&summary).expect("serialize summary");
        assert_eq!(json["provider_summaries"].as_array().map(Vec::len), Some(6));
    }

    #[tokio::test]
    async fn kimi_and_cursor_keep_identical_native_session_keys_in_distinct_sources() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        let cursor_root = temporary.path().join("cursor");
        let kimi_root = temporary.path().join("kimi");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        fs::create_dir_all(cursor_root.join("projects/project/collision"))
            .expect("Cursor directory");
        fs::write(
            cursor_root.join("projects/project/collision/capture.jsonl"),
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"cursor visible\"}]}}\n",
        )
        .expect("Cursor source");
        let kimi_session = kimi_root.join("sessions/work/collision");
        fs::create_dir_all(kimi_session.join("agents/main")).expect("Kimi directory");
        fs::write(
            kimi_session.join("agents/main/wire.jsonl"),
            "{\"type\":\"turn.prompt\",\"input\":[{\"text\":\"kimi visible\"}]}\n",
        )
        .expect("Kimi wire");
        fs::write(
            kimi_session.join("state.json"),
            "{\"agents\":{\"main\":{}}}",
        )
        .expect("Kimi state");
        fs::write(
            kimi_root.join("session_index.jsonl"),
            "{\"sessionId\":\"collision\",\"sessionDir\":\"sessions/work/collision\"}\n",
        )
        .expect("Kimi index");
        fs::write(
            agentlog_home.join("config.toml"),
            format!(
                "[providers]\ncodex_root = \"{}\"\nclaude_root = \"{}\"\nopencode_root = \"{}\"\ngemini_root = \"{}\"\ncursor_root = \"{}\"\nkimi_root = \"{}\"\n",
                temporary.path().join("empty-codex").display(),
                temporary.path().join("empty-claude").display(),
                temporary.path().join("empty-opencode").display(),
                temporary.path().join("empty-gemini").display(),
                cursor_root.display(),
                kimi_root.display(),
            ),
        )
        .expect("write config");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");

        let summary = scan_shell(&paths).await.expect("run scan");

        assert_eq!(summary.sessions_available, 2);
        assert_eq!(
            summary
                .provider_summaries
                .iter()
                .find(|provider| provider.provider == "cursor")
                .map(|provider| provider.sessions_available),
            Some(1)
        );
        assert_eq!(
            summary
                .provider_summaries
                .iter()
                .find(|provider| provider.provider == "kimi")
                .map(|provider| provider.sessions_available),
            Some(1)
        );
    }

    #[tokio::test]
    async fn one_scan_accepts_one_representative_source_from_each_provider() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let home = temporary.path().join("agentlog");
        let codex = temporary.path().join("codex");
        let claude = temporary.path().join("claude");
        let opencode = temporary.path().join("opencode");
        let gemini = temporary.path().join("gemini");
        let cursor = temporary.path().join("cursor");
        let kimi = temporary.path().join("kimi");
        fs::create_dir(&home).expect("home");
        let codex_source = codex.join("sessions/a/source.jsonl");
        fs::create_dir_all(codex_source.parent().expect("parent")).expect("Codex dirs");
        fs::write(&codex_source, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-session\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"codex visible\"}}\n").expect("Codex source");
        let claude_source = claude.join("projects/project/source.jsonl");
        fs::create_dir_all(claude_source.parent().expect("parent")).expect("Claude dirs");
        fs::write(&claude_source, "{\"type\":\"user\",\"sessionId\":\"claude-session\",\"message\":{\"role\":\"user\",\"content\":\"claude visible\"}}\n").expect("Claude source");
        fs::create_dir_all(&opencode).expect("OpenCode directory");
        let opencode_source = opencode.join("opencode.db");
        let opencode_pool = create_opencode_fixture(&opencode_source).await;
        let gemini_source = gemini.join("tmp/source.jsonl");
        fs::create_dir_all(gemini_source.parent().expect("parent")).expect("Gemini dirs");
        fs::write(&gemini_source, "{\"sessionId\":\"gemini-session\"}\n{\"type\":\"user\",\"content\":\"gemini visible\"}\n").expect("Gemini source");
        let cursor_source = cursor.join("projects/project/cursor-session/capture.jsonl");
        fs::create_dir_all(cursor_source.parent().expect("parent")).expect("Cursor dirs");
        fs::write(&cursor_source, "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"cursor visible\"}]}}\n").expect("Cursor source");
        let kimi_session = kimi.join("sessions/work/kimi-session");
        fs::create_dir_all(kimi_session.join("agents/main")).expect("Kimi dirs");
        let kimi_wire = kimi_session.join("agents/main/wire.jsonl");
        fs::write(
            &kimi_wire,
            "{\"type\":\"turn.prompt\",\"input\":[{\"text\":\"kimi visible\"}]}\n",
        )
        .expect("Kimi wire");
        let kimi_state = kimi_session.join("state.json");
        fs::write(&kimi_state, "{\"agents\":{\"main\":{}}}").expect("Kimi state");
        let kimi_index = kimi.join("session_index.jsonl");
        fs::write(
            &kimi_index,
            "{\"sessionId\":\"kimi-session\",\"sessionDir\":\"sessions/work/kimi-session\"}\n",
        )
        .expect("Kimi index");
        fs::write(home.join("config.toml"), format!("[providers]\ncodex_root = \"{}\"\nclaude_root = \"{}\"\nopencode_root = \"{}\"\ngemini_root = \"{}\"\ncursor_root = \"{}\"\nkimi_root = \"{}\"\n", codex.display(), claude.display(), opencode.display(), gemini.display(), cursor.display(), kimi.display())).expect("config");
        opencode_pool.close().await;
        let mut inputs = vec![
            codex_source,
            claude_source,
            opencode_source.clone(),
            gemini_source,
            cursor_source,
            kimi_wire,
            kimi_state,
            kimi_index,
        ];
        for suffix in ["-wal", "-shm"] {
            let sidecar =
                std::path::PathBuf::from(format!("{}{suffix}", opencode_source.display()));
            if sidecar.is_file() {
                inputs.push(sidecar);
            }
        }
        let before = inputs
            .iter()
            .map(|path| {
                (
                    fs::read(path).expect("bytes"),
                    fs::metadata(path).expect("metadata").len(),
                    fs::metadata(path).expect("metadata").modified().ok(),
                )
            })
            .collect::<Vec<_>>();
        let paths = AppPaths::resolve(Some(home)).expect("paths");
        let summary = scan_shell(&paths).await.expect("scan");
        assert_eq!(summary.sources_refreshed, 6);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.sessions_available, 6);
        assert!(
            summary
                .provider_summaries
                .iter()
                .all(|provider| provider.candidate_sources == 1
                    && provider.refreshed_sources == 1
                    && provider.failed_sources == 0
                    && provider.sessions_available == 1)
        );
        for (path, (bytes, length, modified)) in inputs.iter().zip(before) {
            assert_eq!(fs::read(path).expect("bytes"), bytes);
            let metadata = fs::metadata(path).expect("metadata");
            assert_eq!(metadata.len(), length);
            assert_eq!(metadata.modified().ok(), modified);
        }
    }
}
