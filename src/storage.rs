//! Fresh `SQLite` schema v1 for Agentlog's normalized local catalog.
//!
//! The schema deliberately has no migration path. It accepts only a new empty
//! database or an existing v1 database, and refuses every other state without
//! modifying it.

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sqlx::{
    QueryBuilder, Row, Sqlite, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 1;
const TRANSCRIPT_ITEM_INSERT_CHUNK: usize = 500;

/// One provider-owned input with its stable, provider-scoped identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIdentity {
    pub provider: &'static str,
    pub source_format: &'static str,
    pub canonical_locator: String,
}

/// A normalized transcript entry that is safe to persist in Agentlog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogItem {
    UserText(String),
    AssistantText(String),
    ToolMarker {
        name: String,
        status: Option<String>,
    },
}

/// One bounded, provider-native session in an accepted source snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSession {
    pub session_key: String,
    pub title: Option<String>,
    pub repository: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub execution_kind: Option<String>,
    pub started_at: Option<i64>,
    pub last_visible_event_at: Option<i64>,
    pub items: Vec<CatalogItem>,
}

/// The accepted snapshot from one provider-owned input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSnapshot {
    pub identity: SourceIdentity,
    pub diagnostic_status: &'static str,
    pub diagnostic_message: Option<&'static str>,
    pub sessions: Vec<CatalogSession>,
}

/// One session row suitable for the shared, read-only catalog list.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogSessionSummary {
    pub id: i64,
    pub provider: String,
    pub source_format: String,
    pub title: Option<String>,
    pub repository: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub execution_kind: Option<String>,
    pub started_at: Option<i64>,
    pub last_visible_event_at: Option<i64>,
    pub source_diagnostic_status: String,
    pub source_last_success_at: Option<i64>,
}

/// The safe, bounded transcript stored for one selected catalog session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogSessionPreview {
    pub session: CatalogSessionSummary,
    pub items: Vec<CatalogItemView>,
    pub items_truncated: bool,
}

/// A display-safe transcript item returned by the catalog preview.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogItemView {
    UserText {
        content: String,
    },
    AssistantText {
        content: String,
    },
    ToolMarker {
        name: String,
        status: Option<String>,
    },
}

/// Opens an Agentlog-owned database and ensures fresh schema v1 exists.
///
/// The rebuildable catalog uses WAL with `synchronous=NORMAL`: a sudden power
/// loss can lose recent derived catalog updates, but does not corrupt the
/// catalog or affect provider-owned logs. A later scan rebuilds the catalog.
///
/// # Errors
///
/// Returns an error when the database cannot be opened, has an unsupported
/// version, or cannot be initialized as a fresh v1 catalog.
pub async fn open_database(path: &Path) -> Result<SqlitePool, StorageError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(StorageError::Connect)?;

    ensure_schema(&pool).await?;
    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(&pool)
        .await
        .map_err(StorageError::Query)?;
    sqlx::query("PRAGMA synchronous = NORMAL")
        .execute(&pool)
        .await
        .map_err(StorageError::Query)?;
    Ok(pool)
}

/// Initializes only a truly empty database, or validates the current v1 marker.
///
/// # Errors
///
/// Returns an error when the schema version is unsupported, a nonempty database
/// has no Agentlog version, or required v1 tables are missing.
pub async fn ensure_schema(pool: &SqlitePool) -> Result<(), StorageError> {
    let version = scalar_i64(pool, "PRAGMA user_version").await?;

    match version {
        SCHEMA_VERSION => verify_v1_tables(pool).await,
        0 if database_has_no_user_objects(pool).await? => create_v1_schema(pool).await,
        0 => Err(StorageError::UnversionedDatabase),
        other => Err(StorageError::UnsupportedSchemaVersion { found: other }),
    }
}

/// Atomically replaces the normalized sessions for one successfully read source.
///
/// The replacement is intentionally scoped to the source row. A different
/// source can fail without affecting this snapshot, and failures are recorded
/// through [`record_source_failure`] without deleting a previous good snapshot.
///
/// # Errors
///
/// Returns an error if the snapshot cannot be written as one transaction.
pub async fn replace_source_snapshot(
    pool: &SqlitePool,
    snapshot: &SourceSnapshot,
) -> Result<(), StorageError> {
    let mut transaction = pool.begin().await.map_err(StorageError::Query)?;
    replace_source_snapshot_in_transaction(&mut transaction, snapshot).await?;
    transaction.commit().await.map_err(StorageError::Query)
}

/// Replaces one source snapshot inside an already-open provider scan transaction.
///
/// The caller controls the outer transaction, so every statement that replaces
/// this one source stays atomic while a provider scan can commit many accepted
/// sources together.
pub(crate) async fn replace_source_snapshot_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    snapshot: &SourceSnapshot,
) -> Result<(), StorageError> {
    let now = unix_timestamp()?;

    let source_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO sources (provider, source_format, canonical_locator, last_success_at, diagnostic_status, diagnostic_message, diagnostic_recorded_at)\n         VALUES (?, ?, ?, ?, ?, ?, ?)\n         ON CONFLICT(provider, source_format, canonical_locator) DO UPDATE SET\n           last_success_at = excluded.last_success_at,\n           diagnostic_status = excluded.diagnostic_status,\n           diagnostic_message = excluded.diagnostic_message,\n           diagnostic_recorded_at = excluded.diagnostic_recorded_at\n         RETURNING id",
    )
    .bind(snapshot.identity.provider)
    .bind(snapshot.identity.source_format)
    .bind(&snapshot.identity.canonical_locator)
    .bind(now)
    .bind(snapshot.diagnostic_status)
    .bind(snapshot.diagnostic_message)
    .bind(now)
    .fetch_one(&mut **transaction)
    .await
    .map_err(StorageError::Query)?;

    sqlx::query("DELETE FROM sessions WHERE source_id = ?")
        .bind(source_id)
        .execute(&mut **transaction)
        .await
        .map_err(StorageError::Query)?;

    for session in &snapshot.sessions {
        let session_id = sqlx::query(
            "INSERT INTO sessions (source_id, session_key, title, repository, cwd, model, execution_kind, started_at, last_visible_event_at)\n             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(source_id)
        .bind(&session.session_key)
        .bind(&session.title)
        .bind(&session.repository)
        .bind(&session.cwd)
        .bind(&session.model)
        .bind(&session.execution_kind)
        .bind(session.started_at)
        .bind(session.last_visible_event_at)
        .execute(&mut **transaction)
        .await
        .map_err(StorageError::Query)?
        .last_insert_rowid();

        insert_transcript_items(&mut *transaction, session_id, &session.items).await?;
    }

    Ok(())
}

async fn insert_transcript_items(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: i64,
    items: &[CatalogItem],
) -> Result<(), StorageError> {
    for (chunk_index, chunk) in items.chunks(TRANSCRIPT_ITEM_INSERT_CHUNK).enumerate() {
        let chunk_offset = chunk_index
            .checked_mul(TRANSCRIPT_ITEM_INSERT_CHUNK)
            .ok_or(StorageError::OrdinalOverflow)?;
        let mut builder = QueryBuilder::<Sqlite>::new(
            "INSERT INTO transcript_items (session_id, ordinal, item_kind, content, tool_name, tool_status) ",
        );
        builder.push_values(chunk.iter().enumerate(), |mut row, (within_chunk, item)| {
            let ordinal = chunk_offset
                .checked_add(within_chunk)
                .and_then(|value| i64::try_from(value).ok())
                .expect("source session item ordinal was checked before insertion");
            row.push_bind(session_id).push_bind(ordinal);
            match item {
                CatalogItem::UserText(content) => {
                    row.push_bind("user_text")
                        .push_bind(Some(content.as_str()))
                        .push_bind(Option::<&str>::None)
                        .push_bind(Option::<&str>::None);
                }
                CatalogItem::AssistantText(content) => {
                    row.push_bind("assistant_text")
                        .push_bind(Some(content.as_str()))
                        .push_bind(Option::<&str>::None)
                        .push_bind(Option::<&str>::None);
                }
                CatalogItem::ToolMarker { name, status } => {
                    row.push_bind("tool_marker")
                        .push_bind(Option::<&str>::None)
                        .push_bind(Some(name.as_str()))
                        .push_bind(status.as_deref());
                }
            }
        });
        builder
            .build()
            .execute(&mut **transaction)
            .await
            .map_err(StorageError::Query)?;
    }
    Ok(())
}

/// Records a source-level failure while preserving its previous session rows.
///
/// # Errors
///
/// Returns an error when the source diagnostic cannot be persisted.
pub async fn record_source_failure(
    pool: &SqlitePool,
    identity: &SourceIdentity,
    diagnostic_message: &'static str,
) -> Result<(), StorageError> {
    record_source_diagnostic(pool, identity, "error", diagnostic_message).await
}

/// Records one failed source inside an already-open provider scan transaction.
pub(crate) async fn record_source_failure_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    identity: &SourceIdentity,
    diagnostic_message: &'static str,
) -> Result<(), StorageError> {
    record_source_diagnostic_in_transaction(transaction, identity, "error", diagnostic_message)
        .await
}

/// Updates a source diagnostic without replacing its last-good snapshot.
///
/// # Errors
///
/// Returns an error when the source diagnostic cannot be persisted.
pub async fn record_source_diagnostic(
    pool: &SqlitePool,
    identity: &SourceIdentity,
    diagnostic_status: &'static str,
    diagnostic_message: &'static str,
) -> Result<(), StorageError> {
    let mut transaction = pool.begin().await.map_err(StorageError::Query)?;
    record_source_diagnostic_in_transaction(
        &mut transaction,
        identity,
        diagnostic_status,
        diagnostic_message,
    )
    .await?;
    transaction.commit().await.map_err(StorageError::Query)
}

async fn record_source_diagnostic_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    identity: &SourceIdentity,
    diagnostic_status: &'static str,
    diagnostic_message: &'static str,
) -> Result<(), StorageError> {
    let now = unix_timestamp()?;
    sqlx::query(
        "INSERT INTO sources (provider, source_format, canonical_locator, diagnostic_status, diagnostic_message, diagnostic_recorded_at)\n         VALUES (?, ?, ?, ?, ?, ?)\n         ON CONFLICT(provider, source_format, canonical_locator) DO UPDATE SET\n           diagnostic_status = excluded.diagnostic_status,\n           diagnostic_message = excluded.diagnostic_message,\n           diagnostic_recorded_at = excluded.diagnostic_recorded_at",
    )
    .bind(identity.provider)
    .bind(identity.source_format)
    .bind(&identity.canonical_locator)
    .bind(diagnostic_status)
    .bind(diagnostic_message)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(StorageError::Query)?;
    Ok(())
}

/// Lists normalized sessions across every successfully retained provider source.
///
/// # Errors
///
/// Returns an error if the catalog cannot be queried.
pub async fn list_catalog_sessions(
    pool: &SqlitePool,
    limit: u32,
) -> Result<Vec<CatalogSessionSummary>, StorageError> {
    let rows = sqlx::query(
        "SELECT sessions.id,
                sources.provider,
                sources.source_format,
                sessions.title,
                sessions.repository,
                sessions.cwd,
                sessions.model,
                sessions.execution_kind,
                sessions.started_at,
                sessions.last_visible_event_at,
                sources.diagnostic_status,
                sources.last_success_at
         FROM sessions
         JOIN sources ON sources.id = sessions.source_id
         ORDER BY sessions.last_visible_event_at DESC,
                  sessions.started_at DESC,
                  sessions.id DESC
         LIMIT ?",
    )
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(StorageError::Query)?;

    rows.iter().map(catalog_session_summary).collect()
}

/// Reads one normalized catalog session without scanning any provider sources.
///
/// # Errors
///
/// Returns an error if the session is absent or the catalog cannot be queried.
pub async fn read_catalog_preview(
    pool: &SqlitePool,
    session_id: i64,
    item_limit: u32,
    snippet_bytes: usize,
) -> Result<CatalogSessionPreview, StorageError> {
    let row = sqlx::query(
        "SELECT sessions.id,
                sources.provider,
                sources.source_format,
                sessions.title,
                sessions.repository,
                sessions.cwd,
                sessions.model,
                sessions.execution_kind,
                sessions.started_at,
                sessions.last_visible_event_at,
                sources.diagnostic_status,
                sources.last_success_at
         FROM sessions
         JOIN sources ON sources.id = sessions.source_id
         WHERE sessions.id = ?",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::Query)?
    .ok_or(StorageError::CatalogSessionNotFound { session_id })?;
    let session = catalog_session_summary(&row)?;

    let rows = sqlx::query(
        "SELECT item_kind,
                content,
                tool_name,
                tool_status
         FROM transcript_items
         WHERE session_id = ?
         ORDER BY ordinal ASC
         LIMIT ?",
    )
    .bind(session_id)
    .bind(i64::from(item_limit.saturating_add(1)))
    .fetch_all(pool)
    .await
    .map_err(StorageError::Query)?;
    let items_truncated = rows.len() > usize::try_from(item_limit).unwrap_or(usize::MAX);
    let items = rows
        .iter()
        .take(usize::try_from(item_limit).unwrap_or(usize::MAX))
        .map(catalog_item_view)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|item| truncate_item_view(item, snippet_bytes))
        .collect();

    Ok(CatalogSessionPreview {
        session,
        items,
        items_truncated,
    })
}

fn catalog_session_summary(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<CatalogSessionSummary, StorageError> {
    Ok(CatalogSessionSummary {
        id: row.try_get("id").map_err(StorageError::Query)?,
        provider: row.try_get("provider").map_err(StorageError::Query)?,
        source_format: row.try_get("source_format").map_err(StorageError::Query)?,
        title: row.try_get("title").map_err(StorageError::Query)?,
        repository: row.try_get("repository").map_err(StorageError::Query)?,
        cwd: row.try_get("cwd").map_err(StorageError::Query)?,
        model: row.try_get("model").map_err(StorageError::Query)?,
        execution_kind: row.try_get("execution_kind").map_err(StorageError::Query)?,
        started_at: row.try_get("started_at").map_err(StorageError::Query)?,
        last_visible_event_at: row
            .try_get("last_visible_event_at")
            .map_err(StorageError::Query)?,
        source_diagnostic_status: row
            .try_get("diagnostic_status")
            .map_err(StorageError::Query)?,
        source_last_success_at: row
            .try_get("last_success_at")
            .map_err(StorageError::Query)?,
    })
}

fn truncate_item_view(item: CatalogItemView, snippet_bytes: usize) -> CatalogItemView {
    match item {
        CatalogItemView::UserText { content } => CatalogItemView::UserText {
            content: truncate_display_text(content, snippet_bytes),
        },
        CatalogItemView::AssistantText { content } => CatalogItemView::AssistantText {
            content: truncate_display_text(content, snippet_bytes),
        },
        marker @ CatalogItemView::ToolMarker { .. } => marker,
    }
}

fn truncate_display_text(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    if max_bytes == 0 {
        return String::new();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn catalog_item_view(row: &sqlx::sqlite::SqliteRow) -> Result<CatalogItemView, StorageError> {
    let item_kind = row
        .try_get::<String, _>("item_kind")
        .map_err(StorageError::Query)?;
    match item_kind.as_str() {
        "user_text" => Ok(CatalogItemView::UserText {
            content: row.try_get("content").map_err(StorageError::Query)?,
        }),
        "assistant_text" => Ok(CatalogItemView::AssistantText {
            content: row.try_get("content").map_err(StorageError::Query)?,
        }),
        "tool_marker" => Ok(CatalogItemView::ToolMarker {
            name: row.try_get("tool_name").map_err(StorageError::Query)?,
            status: row.try_get("tool_status").map_err(StorageError::Query)?,
        }),
        _ => Err(StorageError::InvalidCatalogItemKind { item_kind }),
    }
}

fn unix_timestamp() -> Result<i64, StorageError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(StorageError::Clock)?;
    i64::try_from(duration.as_secs()).map_err(|_| StorageError::ClockOutOfRange)
}

async fn scalar_i64(pool: &SqlitePool, query: &str) -> Result<i64, StorageError> {
    sqlx::query_scalar::<_, i64>(query)
        .fetch_one(pool)
        .await
        .map_err(StorageError::Query)
}

async fn database_has_no_user_objects(pool: &SqlitePool) -> Result<bool, StorageError> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)\n         FROM sqlite_master\n         WHERE type IN ('table', 'index', 'trigger', 'view')\n           AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(pool)
    .await
    .map_err(StorageError::Query)?;

    Ok(count == 0)
}

async fn create_v1_schema(pool: &SqlitePool) -> Result<(), StorageError> {
    let mut transaction = pool.begin().await.map_err(StorageError::Query)?;

    sqlx::query(
        "CREATE TABLE sources (\n             id INTEGER PRIMARY KEY,\n             provider TEXT NOT NULL,\n             source_format TEXT NOT NULL,\n             canonical_locator TEXT NOT NULL,\n             last_success_at INTEGER,\n             diagnostic_status TEXT NOT NULL DEFAULT 'unknown' CHECK(diagnostic_status IN ('unknown', 'ok', 'partial', 'error')),\n             diagnostic_message TEXT,\n             diagnostic_recorded_at INTEGER,\n             UNIQUE(provider, source_format, canonical_locator)\n         )",
    )
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::Query)?;

    sqlx::query(
        "CREATE TABLE sessions (\n             id INTEGER PRIMARY KEY,\n             source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,\n             session_key TEXT NOT NULL,\n             title TEXT,\n             repository TEXT,\n             cwd TEXT,\n             model TEXT,\n             execution_kind TEXT,\n             started_at INTEGER,\n             last_visible_event_at INTEGER,\n             UNIQUE(source_id, session_key)\n         )",
    )
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::Query)?;

    sqlx::query(
        "CREATE TABLE transcript_items (\n             id INTEGER PRIMARY KEY,\n             session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,\n             ordinal INTEGER NOT NULL CHECK(ordinal >= 0),\n             item_kind TEXT NOT NULL CHECK(item_kind IN ('user_text', 'assistant_text', 'tool_marker')),\n             content TEXT,\n             tool_name TEXT,\n             tool_status TEXT,\n             UNIQUE(session_id, ordinal),\n             CHECK((item_kind IN ('user_text', 'assistant_text') AND content IS NOT NULL AND tool_name IS NULL AND tool_status IS NULL)\n                OR (item_kind = 'tool_marker' AND content IS NULL AND tool_name IS NOT NULL))\n         )",
    )
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::Query)?;

    sqlx::query(
        "CREATE INDEX sessions_recent ON sessions(last_visible_event_at DESC, started_at DESC)",
    )
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::Query)?;
    sqlx::query("PRAGMA user_version = 1")
        .execute(&mut *transaction)
        .await
        .map_err(StorageError::Query)?;

    transaction.commit().await.map_err(StorageError::Query)
}

async fn verify_v1_tables(pool: &SqlitePool) -> Result<(), StorageError> {
    for table in ["sources", "sessions", "transcript_items"] {
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(\n                 SELECT 1\n                 FROM sqlite_master\n                 WHERE type = 'table'\n                   AND name = ?\n             )",
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .map_err(StorageError::Query)?;

        if exists == 0 {
            return Err(StorageError::IncompleteV1Schema { table });
        }
    }

    Ok(())
}

/// Failures while opening or validating Agentlog's `SQLite` catalog.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("cannot open Agentlog SQLite database: {0}")]
    Connect(sqlx::Error),
    #[error("SQLite operation failed: {0}")]
    Query(sqlx::Error),
    #[error("database has tables but no Agentlog schema version; refusing to modify it")]
    UnversionedDatabase,
    #[error("unsupported Agentlog schema version {found}; refusing to modify it")]
    UnsupportedSchemaVersion { found: i64 },
    #[error("database claims schema v1 but is missing required table {table}")]
    IncompleteV1Schema { table: &'static str },
    #[error("catalog transcript ordinal exceeds SQLite integer range")]
    OrdinalOverflow,
    #[error("system clock is before the Unix epoch")]
    Clock(std::time::SystemTimeError),
    #[error("system clock is outside the catalog timestamp range")]
    ClockOutOfRange,
    #[error("catalog session {session_id} was not found")]
    CatalogSessionNotFound { session_id: i64 },
    #[error("catalog contains an invalid transcript item kind {item_kind}")]
    InvalidCatalogItemKind { item_kind: String },
    #[error("cannot inspect a provider-owned source: {0}")]
    SourceIo(std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use sqlx::{
        Row, SqlitePool,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    use tempfile::TempDir;

    use super::{
        CatalogItem, CatalogSession, SCHEMA_VERSION, SourceIdentity, SourceSnapshot, StorageError,
        list_catalog_sessions, open_database, read_catalog_preview, record_source_failure,
        replace_source_snapshot,
    };

    async fn user_tables(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT name\n             FROM sqlite_master\n             WHERE type = 'table'\n               AND name NOT LIKE 'sqlite_%'\n             ORDER BY name",
        )
        .fetch_all(pool)
        .await
        .expect("list tables")
    }

    async fn open_temporary_database(temporary: &TempDir, name: &str) -> SqlitePool {
        open_database(&temporary.path().join(name))
            .await
            .expect("open database")
    }

    async fn create_foreign_database(path: &Path, user_version: i64, table: bool) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await
            .expect("open foreign database");
        if table {
            sqlx::query("CREATE TABLE foreign_data (id INTEGER PRIMARY KEY)")
                .execute(&pool)
                .await
                .expect("create foreign table");
        }
        sqlx::query(&format!("PRAGMA user_version = {user_version}"))
            .execute(&pool)
            .await
            .expect("set foreign version");
        pool.close().await;
    }

    #[derive(Debug, Eq, PartialEq)]
    struct DatabaseFiles {
        database: Vec<u8>,
        wal: Option<Vec<u8>>,
        shm: Option<Vec<u8>>,
    }

    fn database_files(path: &Path) -> DatabaseFiles {
        fn read_optional(path: PathBuf) -> Option<Vec<u8>> {
            match fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("read database sidecar: {error}"),
            }
        }

        DatabaseFiles {
            database: fs::read(path).expect("read database"),
            wal: read_optional(PathBuf::from(format!("{}-wal", path.display()))),
            shm: read_optional(PathBuf::from(format!("{}-shm", path.display()))),
        }
    }

    #[tokio::test]
    async fn fresh_database_creates_only_v1_catalog_tables() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let database = open_temporary_database(&temporary, "agentlog.sqlite3").await;

        let version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&database)
            .await
            .expect("schema version");
        let synchronous = sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
            .fetch_one(&database)
            .await
            .expect("synchronous mode");
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(synchronous, 1, "derived catalog uses WAL/NORMAL");
        assert_eq!(
            user_tables(&database).await,
            vec!["sessions", "sources", "transcript_items",]
        );
        database.close().await;
    }

    #[tokio::test]
    async fn unknown_version_is_not_changed() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let path = temporary.path().join("future.sqlite3");
        create_foreign_database(&path, 99, false).await;
        let before = database_files(&path);

        let error = open_database(&path)
            .await
            .expect_err("future version must fail");

        assert!(matches!(
            error,
            StorageError::UnsupportedSchemaVersion { found: 99 }
        ));
        assert_eq!(database_files(&path), before);
    }

    #[tokio::test]
    async fn unversioned_nonempty_database_is_not_adopted() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let path = temporary.path().join("foreign.sqlite3");
        create_foreign_database(&path, 0, true).await;
        let before = database_files(&path);

        let error = open_database(Path::new(&path))
            .await
            .expect_err("unversioned database must fail");

        assert!(matches!(error, StorageError::UnversionedDatabase));
        assert_eq!(database_files(&path), before);
    }

    #[tokio::test]
    async fn schema_enforces_source_identity_and_cascades_source_snapshots() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let database = open_temporary_database(&temporary, "catalog.sqlite3").await;

        sqlx::query(
            "INSERT INTO sources (provider, source_format, canonical_locator)\n             VALUES ('codex', 'session_jsonl', '/private/tmp/session.jsonl')",
        )
        .execute(&database)
        .await
        .expect("insert source");
        let duplicate = sqlx::query(
            "INSERT INTO sources (provider, source_format, canonical_locator)\n             VALUES ('codex', 'session_jsonl', '/private/tmp/session.jsonl')",
        )
        .execute(&database)
        .await;
        assert!(duplicate.is_err());

        sqlx::query(
            "INSERT INTO sessions (source_id, session_key, title)\n             VALUES (1, 'native-session', 'A session')",
        )
        .execute(&database)
        .await
        .expect("insert session");
        sqlx::query(
            "INSERT INTO transcript_items (session_id, ordinal, item_kind, content)\n             VALUES (1, 0, 'user_text', 'visible request')",
        )
        .execute(&database)
        .await
        .expect("insert visible item");

        sqlx::query("DELETE FROM sources WHERE id = 1")
            .execute(&database)
            .await
            .expect("delete source snapshot");
        let sessions = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(&database)
            .await
            .expect("count sessions");
        let transcript_items =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM transcript_items")
                .fetch_one(&database)
                .await
                .expect("count transcript items");

        assert_eq!(sessions, 0);
        assert_eq!(transcript_items, 0);
        database.close().await;
    }

    #[tokio::test]
    async fn schema_rejects_invalid_visible_transcript_shapes() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let database = open_temporary_database(&temporary, "transcript.sqlite3").await;
        sqlx::query(
            "INSERT INTO sources (provider, source_format, canonical_locator)\n             VALUES ('claude', 'project_jsonl', '/private/tmp/project.jsonl')",
        )
        .execute(&database)
        .await
        .expect("insert source");
        sqlx::query("INSERT INTO sessions (source_id, session_key) VALUES (1, 'native-session')")
            .execute(&database)
            .await
            .expect("insert session");

        let missing_text = sqlx::query(
            "INSERT INTO transcript_items (session_id, ordinal, item_kind)\n             VALUES (1, 0, 'user_text')",
        )
        .execute(&database)
        .await;
        let tool_with_text = sqlx::query(
            "INSERT INTO transcript_items (session_id, ordinal, item_kind, content, tool_name)\n             VALUES (1, 1, 'tool_marker', 'tool output', 'shell')",
        )
        .execute(&database)
        .await;

        assert!(missing_text.is_err());
        assert!(tool_with_text.is_err());
        database.close().await;
    }

    #[tokio::test]
    async fn shared_catalog_list_and_preview_expose_provider_and_stale_source_status() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let database = open_temporary_database(&temporary, "catalog.sqlite3").await;
        for (provider, locator, session_key, text) in [
            (
                "codex",
                "/private/tmp/codex.jsonl",
                "codex-session",
                "Codex visible text",
            ),
            (
                "claude",
                "/private/tmp/claude.jsonl",
                "claude-session",
                "Claude visible text",
            ),
        ] {
            replace_source_snapshot(
                &database,
                &SourceSnapshot {
                    identity: SourceIdentity {
                        provider,
                        source_format: "session_jsonl",
                        canonical_locator: locator.to_owned(),
                    },
                    diagnostic_status: "ok",
                    diagnostic_message: None,
                    sessions: vec![CatalogSession {
                        session_key: session_key.to_owned(),
                        title: Some(text.to_owned()),
                        repository: None,
                        cwd: None,
                        model: None,
                        execution_kind: None,
                        started_at: Some(1),
                        last_visible_event_at: Some(1),
                        items: vec![CatalogItem::UserText(text.to_owned())],
                    }],
                },
            )
            .await
            .expect("store snapshot");
        }
        let claude_identity = SourceIdentity {
            provider: "claude",
            source_format: "session_jsonl",
            canonical_locator: "/private/tmp/claude.jsonl".to_owned(),
        };
        record_source_failure(&database, &claude_identity, "source became malformed")
            .await
            .expect("record stale failure");

        let sessions = list_catalog_sessions(&database, 50)
            .await
            .expect("list shared catalog");
        assert_eq!(sessions.len(), 2);
        let claude = sessions
            .iter()
            .find(|session| session.provider == "claude")
            .expect("Claude session");
        assert_eq!(claude.source_diagnostic_status, "error");
        assert!(claude.source_last_success_at.is_some());
        let preview = read_catalog_preview(&database, claude.id, 1, 5)
            .await
            .expect("read bounded preview");
        assert_eq!(preview.session.provider, "claude");
        assert_eq!(preview.session.source_diagnostic_status, "error");
        assert!(
            matches!(preview.items.as_slice(), [super::CatalogItemView::UserText { content }] if content == "Claud…")
        );
        assert!(!preview.items_truncated);
        database.close().await;
    }

    #[tokio::test]
    async fn transcript_batch_insert_preserves_all_item_shapes_across_chunk_boundary() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let database = open_temporary_database(&temporary, "catalog.sqlite3").await;
        let items = (0..501)
            .map(|index| match index % 3 {
                0 => CatalogItem::UserText(format!("user {index}")),
                1 => CatalogItem::AssistantText(format!("assistant {index}")),
                _ => CatalogItem::ToolMarker {
                    name: format!("tool_{index}"),
                    status: Some("requested".to_owned()),
                },
            })
            .collect::<Vec<_>>();
        replace_source_snapshot(
            &database,
            &SourceSnapshot {
                identity: SourceIdentity {
                    provider: "claude",
                    source_format: "project_jsonl",
                    canonical_locator: "/private/tmp/chunk.jsonl".to_owned(),
                },
                diagnostic_status: "ok",
                diagnostic_message: None,
                sessions: vec![CatalogSession {
                    session_key: "chunked".to_owned(),
                    title: None,
                    repository: None,
                    cwd: None,
                    model: None,
                    execution_kind: None,
                    started_at: None,
                    last_visible_event_at: None,
                    items,
                }],
            },
        )
        .await
        .expect("store chunked snapshot");

        let rows = sqlx::query(
            "SELECT ordinal, item_kind, content, tool_name, tool_status
             FROM transcript_items
             ORDER BY ordinal ASC",
        )
        .fetch_all(&database)
        .await
        .expect("read inserted items");
        assert_eq!(rows.len(), 501);
        for (expected_ordinal, row) in rows.iter().enumerate() {
            assert_eq!(
                row.try_get::<i64, _>("ordinal").expect("ordinal"),
                i64::try_from(expected_ordinal).expect("ordinal range")
            );
            assert_batch_item_shape(row, expected_ordinal % 3);
        }
        database.close().await;
    }

    fn assert_batch_item_shape(row: &sqlx::sqlite::SqliteRow, item_type: usize) {
        let kind = row.try_get::<String, _>("item_kind").expect("kind");
        let content = row
            .try_get::<Option<String>, _>("content")
            .expect("content");
        let tool_name = row
            .try_get::<Option<String>, _>("tool_name")
            .expect("tool name");
        let tool_status = row
            .try_get::<Option<String>, _>("tool_status")
            .expect("tool status");
        match item_type {
            0 => {
                assert_eq!(kind, "user_text");
                assert!(content.is_some());
                assert!(tool_name.is_none());
                assert!(tool_status.is_none());
            }
            1 => {
                assert_eq!(kind, "assistant_text");
                assert!(content.is_some());
                assert!(tool_name.is_none());
                assert!(tool_status.is_none());
            }
            _ => {
                assert_eq!(kind, "tool_marker");
                assert!(content.is_none());
                assert!(tool_name.is_some());
                assert_eq!(tool_status, Some("requested".to_owned()));
            }
        }
    }
}
