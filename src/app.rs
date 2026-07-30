//! The small application workflows available before collectors are installed.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Serialize;

use crate::{
    paths::AppPaths,
    providers::{ProviderScanReport, installed, source_descriptions},
    storage::{
        Catalog, CatalogFileIdentity, CatalogFileState, CatalogSessionPreview,
        CatalogSessionSummary, CatalogSourceDiagnostic, SCHEMA_VERSION, clear_existing_catalog,
        inspect_catalog_file_state, inspect_catalog_snapshot,
    },
};

const DATA_CLEAR_SNAPSHOT_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Observable result of one read-only provider synchronization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SyncSummary {
    pub schema_version: i64,
    pub collectors_installed: u8,
    pub sources_refreshed: u64,
    pub sources_partial: u64,
    pub sources_failed: u64,
    pub sources_missing: u64,
    pub sessions_available: u64,
    pub provider_summaries: Vec<ProviderSyncSummary>,
    pub message: String,
}

/// Aggregate diagnostic counts for one explicit provider collector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderSyncSummary {
    pub provider: String,
    pub candidate_sources: u64,
    pub refreshed_sources: u64,
    pub partial_sources: u64,
    pub failed_sources: u64,
    pub missing_sources: u64,
    pub sessions_available: u64,
    pub failure: Option<String>,
}

/// Initializes the durable catalog and collects read-only provider sources.
///
/// # Errors
///
/// Returns an error when Agentlog-owned storage cannot be created, opened, or
/// queried.
pub async fn sync_shell(paths: &AppPaths) -> anyhow::Result<SyncSummary> {
    paths.ensure_data_dir()?;
    let catalog = Catalog::open(&paths.database_path).await?;
    let mut sync_results = Vec::with_capacity(6);
    for scanner in installed(paths.provider_roots()) {
        match scanner {
            Ok(scanner) => {
                let provider = scanner.provider_id().as_str().to_owned();
                match catalog.scan(&*scanner).await {
                    Ok(report) => sync_results.push((provider, report, None)),
                    Err(error) => sync_results.push((
                        provider,
                        ProviderScanReport::default(),
                        Some(error.to_string()),
                    )),
                }
            }
            Err(error) => sync_results.push((
                error.provider_id().as_str().to_owned(),
                ProviderScanReport::default(),
                Some(error.to_string()),
            )),
        }
    }
    let sessions_available = catalog.session_count().await?;
    let provider_session_counts = catalog.provider_session_counts().await?;
    catalog.close().await;

    let provider_summaries = sync_results
        .iter()
        .map(|(provider, report, failure)| ProviderSyncSummary {
            provider: provider.clone(),
            candidate_sources: report.candidate_sources,
            refreshed_sources: report.refreshed_sources,
            partial_sources: report.partial_sources,
            failed_sources: report.failed_sources,
            missing_sources: report.missing_sources,
            sessions_available: provider_sessions(&provider_session_counts, provider),
            failure: failure.clone(),
        })
        .collect::<Vec<_>>();

    Ok(SyncSummary {
        schema_version: SCHEMA_VERSION,
        collectors_installed: u8::try_from(
            sync_results
                .iter()
                .filter(|(_, _, failure)| failure.is_none())
                .count(),
        )
        .unwrap_or(u8::MAX),
        sources_refreshed: sync_results
            .iter()
            .map(|(_, report, _)| report.refreshed_sources)
            .sum(),
        sources_partial: sync_results
            .iter()
            .map(|(_, report, _)| report.partial_sources)
            .sum(),
        sources_failed: sync_results
            .iter()
            .map(|(_, report, _)| report.failed_sources)
            .sum(),
        sources_missing: sync_results
            .iter()
            .map(|(_, report, _)| report.missing_sources)
            .sum(),
        sessions_available,
        provider_summaries,
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
    let catalog = Catalog::open(&paths.database_path).await?;
    let sessions = catalog.list_sessions(limit).await?;
    catalog.close().await;
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
    let catalog = Catalog::open(&paths.database_path).await?;
    let session = catalog.preview(session_id).await?;
    catalog.close().await;
    Ok(session)
}

/// Reads a current session preview by the stable identity retained in a list row.
///
/// # Errors
///
/// Returns an error when the session is unavailable or the Agentlog catalog
/// cannot be opened.
pub async fn show_shell_by_identity(
    paths: &AppPaths,
    session: &CatalogSessionSummary,
) -> anyhow::Result<CatalogSessionPreview> {
    paths.ensure_data_dir()?;
    let catalog = Catalog::open(&paths.database_path).await?;
    let preview = catalog.preview_by_identity(session).await?;
    catalog.close().await;
    Ok(preview)
}

/// Reads all retained source diagnostics without starting a provider scan.
///
/// # Errors
///
/// Returns an error when the Agentlog catalog cannot be opened or queried.
pub async fn diagnostics_shell(paths: &AppPaths) -> anyhow::Result<Vec<CatalogSourceDiagnostic>> {
    paths.ensure_data_dir()?;
    let catalog = Catalog::open(&paths.database_path).await?;
    let diagnostics = catalog.source_diagnostics().await?;
    catalog.close().await;
    Ok(diagnostics)
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
        provider_sources: source_descriptions(paths.provider_roots()),
    }
}

/// The exact Agentlog-owned catalog target described before a clear operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DataClearSummary {
    pub database: PathBuf,
    pub sessions: u64,
    pub transcript_items: u64,
    pub approximate_bytes: u64,
    pub cleared: bool,
}

/// Reports, and when explicitly confirmed clears, only the Agentlog-owned
/// catalog contents.
///
/// The database path is fixed by [`AppPaths`]; configuration and provider-owned
/// source paths are neither opened for writing nor selected for deletion.
///
/// # Errors
///
/// Previewing reads a bounded private copy of the `SQLite` main/WAL contents,
/// while fingerprinting the complete main/WAL/SHM file set, so it never opens
/// or writes the live catalog. Confirmation obtains `SQLite` writer
/// serialization and clears rows in one transaction while preserving the
/// schema and database files.
///
/// Returns an error when the database is not a regular Agentlog v1 catalog, a
/// file changes while the private preview snapshot is taken, or an explicitly
/// requested clear cannot acquire `SQLite`'s writer lock.
pub async fn clear_data_shell(
    paths: &AppPaths,
    confirmed: bool,
) -> anyhow::Result<DataClearSummary> {
    if confirmed {
        let Some(target) = inspect_confirmed_clear_target(&paths.database_path)? else {
            return Ok(empty_data_clear_summary(paths));
        };
        let counts = clear_existing_catalog(&paths.database_path, target.database_identity)
            .await
            .with_context(|| {
                format!(
                    "clear Agentlog-owned database {}",
                    paths.database_path.display()
                )
            })?;
        return Ok(DataClearSummary {
            database: paths.database_path.clone(),
            sessions: counts.sessions,
            transcript_items: counts.transcript_items,
            approximate_bytes: target.approximate_bytes,
            cleared: true,
        });
    }

    let Some(target) = inspect_clear_target(&paths.database_path)? else {
        return Ok(empty_data_clear_summary(paths));
    };
    let counts = inspect_catalog_snapshot(&target.snapshot_database)
        .await
        .with_context(|| {
            format!(
                "validate private snapshot of Agentlog-owned database {}",
                paths.database_path.display()
            )
        })?;
    Ok(DataClearSummary {
        database: paths.database_path.clone(),
        sessions: counts.sessions,
        transcript_items: counts.transcript_items,
        approximate_bytes: target.approximate_bytes,
        cleared: false,
    })
}

fn empty_data_clear_summary(paths: &AppPaths) -> DataClearSummary {
    DataClearSummary {
        database: paths.database_path.clone(),
        sessions: 0,
        transcript_items: 0,
        approximate_bytes: 0,
        cleared: false,
    }
}

#[derive(Debug)]
struct ClearTarget {
    approximate_bytes: u64,
    _snapshot_directory: tempfile::TempDir,
    snapshot_database: PathBuf,
}

struct ConfirmedClearTarget {
    approximate_bytes: u64,
    database_identity: CatalogFileIdentity,
}

fn inspect_confirmed_clear_target(database: &Path) -> anyhow::Result<Option<ConfirmedClearTarget>> {
    let Some(files) = inspect_catalog_file_set(database)? else {
        return Ok(None);
    };
    let approximate_bytes =
        catalog_file_set_size(database, files.iter().map(|file| file.state.length))?;
    let database_identity = files
        .first()
        .context("catalog file set is missing its main database")?
        .state
        .identity;
    Ok(Some(ConfirmedClearTarget {
        approximate_bytes,
        database_identity,
    }))
}

fn inspect_clear_target(database: &Path) -> anyhow::Result<Option<ClearTarget>> {
    inspect_clear_target_with_hook(database, || {})
}

fn inspect_clear_target_with_hook(
    database: &Path,
    after_copy: impl FnOnce(),
) -> anyhow::Result<Option<ClearTarget>> {
    let Some(files) = read_catalog_file_set(database)? else {
        return Ok(None);
    };
    let approximate_bytes =
        catalog_file_set_size(database, files.iter().map(|file| file.state.length))?;
    let snapshot_directory = tempfile::Builder::new()
        .prefix("agentlog-data-clear-")
        .tempdir_in("/private/tmp")
        .context("create private catalog inspection snapshot")?;
    let snapshot_database = snapshot_directory.path().join("agentlog.sqlite3");
    for file in &files {
        if file.suffix == Some("-shm") {
            continue;
        }
        let snapshot_path = if file.is_main {
            snapshot_database.clone()
        } else {
            sqlite_sidecar_path(&snapshot_database, file.suffix.expect("sidecar suffix"))
        };
        fs::write(&snapshot_path, &file.bytes)
            .with_context(|| format!("write private snapshot {}", snapshot_path.display()))?;
    }
    after_copy();
    validate_catalog_file_set(database, &files)?;

    Ok(Some(ClearTarget {
        approximate_bytes,
        _snapshot_directory: snapshot_directory,
        snapshot_database,
    }))
}

fn catalog_file_set_size(
    database: &Path,
    lengths: impl IntoIterator<Item = u64>,
) -> anyhow::Result<u64> {
    let approximate_bytes = lengths.into_iter().try_fold(0_u64, |total, length| {
        total
            .checked_add(length)
            .context("database size exceeds supported range")
    })?;
    if approximate_bytes > DATA_CLEAR_SNAPSHOT_MAX_BYTES {
        anyhow::bail!(
            "refusing to inspect {}; the SQLite file set exceeds the {} byte snapshot limit",
            database.display(),
            DATA_CLEAR_SNAPSHOT_MAX_BYTES
        );
    }
    Ok(approximate_bytes)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CatalogFile {
    path: PathBuf,
    suffix: Option<&'static str>,
    is_main: bool,
    state: CatalogFileState,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CatalogFileDescriptor {
    path: PathBuf,
    suffix: Option<&'static str>,
    is_main: bool,
    state: CatalogFileState,
}

fn inspect_catalog_file_set(database: &Path) -> anyhow::Result<Option<Vec<CatalogFileDescriptor>>> {
    let Some(main) = inspect_catalog_file_state(database)? else {
        return Ok(None);
    };
    let mut files = vec![CatalogFileDescriptor {
        path: database.to_path_buf(),
        suffix: None,
        is_main: true,
        state: main,
    }];
    for suffix in ["-wal", "-shm"] {
        let sidecar = sqlite_sidecar_path(database, suffix);
        if let Some(state) = inspect_catalog_file_state(&sidecar)? {
            files.push(CatalogFileDescriptor {
                path: sidecar,
                state,
                is_main: false,
                suffix: Some(suffix),
            });
        }
    }
    Ok(Some(files))
}

fn read_catalog_file_set(database: &Path) -> anyhow::Result<Option<Vec<CatalogFile>>> {
    let Some(descriptors) = inspect_catalog_file_set(database)? else {
        return Ok(None);
    };
    catalog_file_set_size(
        database,
        descriptors.iter().map(|descriptor| descriptor.state.length),
    )?;
    descriptors
        .iter()
        .map(read_catalog_file)
        .collect::<anyhow::Result<Vec<_>>>()
        .map(Some)
}

fn read_catalog_file(descriptor: &CatalogFileDescriptor) -> anyhow::Result<CatalogFile> {
    let file = File::open(&descriptor.path)
        .with_context(|| format!("read Agentlog database file {}", descriptor.path.display()))?;
    let opened_metadata = file
        .metadata()
        .with_context(|| format!("inspect opened database file {}", descriptor.path.display()))?;
    let opened_state = CatalogFileState::from_metadata(&descriptor.path, &opened_metadata)?;
    if opened_state != descriptor.state {
        return catalog_changed();
    }

    let read_limit = descriptor
        .state
        .length
        .checked_add(1)
        .context("database size exceeds supported range")?;
    let mut bytes = Vec::new();
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read Agentlog database file {}", descriptor.path.display()))?;
    if u64::try_from(bytes.len()).ok() != Some(descriptor.state.length) {
        return catalog_changed();
    }

    Ok(CatalogFile {
        path: descriptor.path.clone(),
        suffix: descriptor.suffix,
        is_main: descriptor.is_main,
        state: descriptor.state,
        bytes,
    })
}

fn validate_catalog_file_set(database: &Path, expected: &[CatalogFile]) -> anyhow::Result<()> {
    let Some(current) = inspect_catalog_file_set(database)? else {
        return catalog_changed();
    };
    catalog_file_set_size(
        database,
        current.iter().map(|descriptor| descriptor.state.length),
    )?;
    if !catalog_file_descriptors_match(&current, expected) {
        return catalog_changed();
    }

    for (descriptor, expected) in current.iter().zip(expected) {
        if read_catalog_file(descriptor)?.bytes != expected.bytes {
            return catalog_changed();
        }
    }
    let Some(final_state) = inspect_catalog_file_set(database)? else {
        return catalog_changed();
    };
    if !catalog_file_descriptors_match(&final_state, expected) {
        return catalog_changed();
    }
    Ok(())
}

fn catalog_file_descriptors_match(
    current: &[CatalogFileDescriptor],
    expected: &[CatalogFile],
) -> bool {
    current.len() == expected.len()
        && current.iter().zip(expected).all(|(current, expected)| {
            current.path == expected.path
                && current.suffix == expected.suffix
                && current.is_main == expected.is_main
                && current.state == expected.state
        })
}

fn catalog_changed<T>() -> anyhow::Result<T> {
    Err(anyhow::anyhow!(
        "Agentlog database file set changed while preparing a read-only clear preview; retry the command"
    ))
}

fn sqlite_sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use sqlx::{
        SqlitePool,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };

    use crate::{
        paths::AppPaths,
        providers::opencode::create_test_database,
        storage::{SCHEMA_VERSION, open_database},
    };

    use super::{
        DATA_CLEAR_SNAPSHOT_MAX_BYTES, clear_data_shell, diagnostics_shell, inspect_clear_target,
        inspect_clear_target_with_hook, list_shell, sqlite_sidecar_path, sync_shell,
    };

    #[derive(Debug, Eq, PartialEq)]
    struct FileFingerprint {
        bytes: Vec<u8>,
        length: u64,
        modified: std::time::SystemTime,
    }

    fn fingerprint(path: &std::path::Path) -> FileFingerprint {
        let metadata = fs::metadata(path).expect("file metadata");
        FileFingerprint {
            bytes: fs::read(path).expect("file bytes"),
            length: metadata.len(),
            modified: metadata.modified().expect("file modification time"),
        }
    }

    fn directory_fingerprint(directory: &std::path::Path) -> Vec<(String, FileFingerprint)> {
        let mut files = fs::read_dir(directory)
            .expect("read Agentlog data directory")
            .map(|entry| {
                let entry = entry.expect("directory entry");
                (
                    entry
                        .file_name()
                        .into_string()
                        .expect("test filenames are UTF-8"),
                    fingerprint(&entry.path()),
                )
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

    async fn scanned_data_clear_fixture() -> (TempDir, AppPaths, std::path::PathBuf) {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        let gemini_root = temporary.path().join("gemini");
        let source = gemini_root.join("tmp/session.jsonl");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        fs::create_dir_all(source.parent().expect("Gemini source parent"))
            .expect("create Gemini directory");
        fs::write(
            &source,
            "{\"sessionId\":\"clear-session\"}\n{\"type\":\"user\",\"content\":\"retained request\"}\n",
        )
        .expect("write provider source");
        let config = format!(
            "[providers]\ncodex_root = \"{}\"\nclaude_root = \"{}\"\nopencode_root = \"{}\"\ngemini_root = \"{}\"\ncursor_root = \"{}\"\nkimi_root = \"{}\"\n",
            temporary.path().join("empty-codex").display(),
            temporary.path().join("empty-claude").display(),
            temporary.path().join("empty-opencode").display(),
            gemini_root.display(),
            temporary.path().join("empty-cursor").display(),
            temporary.path().join("empty-kimi").display(),
        );
        fs::write(agentlog_home.join("config.toml"), config).expect("write config");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");
        let scan = sync_shell(&paths).await.expect("sync fixture source");
        assert_eq!(scan.sessions_available, 1);

        (temporary, paths, source)
    }

    #[tokio::test]
    async fn diagnostics_shell_opens_the_catalog_without_scanning_sources() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");

        let diagnostics = diagnostics_shell(&paths)
            .await
            .expect("read empty diagnostics");

        assert!(diagnostics.is_empty());
        assert!(paths.database_path.is_file());
    }

    #[tokio::test]
    async fn data_clear_preview_and_missing_confirmation_leave_all_inputs_unchanged() {
        let (_temporary, paths, source) = scanned_data_clear_fixture().await;
        let writer = open_database(&paths.database_path)
            .await
            .expect("open catalog to retain live WAL sidecars");
        sqlx::query(
            "INSERT INTO sources (provider, source_format, canonical_locator) VALUES ('test', 'test', 'test://sidecar')",
        )
        .execute(&writer)
        .await
        .expect("write catalog WAL frame");
        let live_shm = sqlite_sidecar_path(&paths.database_path, "-shm");
        assert!(live_shm.is_file(), "live WAL database has an SHM sidecar");
        let private_target = inspect_clear_target(&paths.database_path)
            .expect("copy private clear target")
            .expect("catalog target exists");
        assert!(
            !sqlite_sidecar_path(&private_target.snapshot_database, "-shm").exists(),
            "live SHM is not copied into the private snapshot"
        );
        drop(private_target);
        let database_before = directory_fingerprint(&paths.data_dir);
        let config_before = fingerprint(&paths.config_path);
        let source_before = fingerprint(&source);

        let dry_run = clear_data_shell(&paths, false)
            .await
            .expect("preview data clear");
        assert_eq!(dry_run.sessions, 1);
        assert_eq!(dry_run.transcript_items, 1);
        assert!(dry_run.approximate_bytes > 0);
        assert!(!dry_run.cleared);
        assert_eq!(directory_fingerprint(&paths.data_dir), database_before);

        let without_yes = clear_data_shell(&paths, false)
            .await
            .expect("reject unconfirmed clear");
        assert!(!without_yes.cleared);
        assert!(paths.database_path.is_file());
        assert_eq!(directory_fingerprint(&paths.data_dir), database_before);
        assert_eq!(fingerprint(&paths.config_path), config_before);
        assert_eq!(fingerprint(&source), source_before);
        writer.close().await;
    }

    #[tokio::test]
    async fn data_clear_preview_rejects_a_file_set_presence_change_during_snapshot() {
        let (temporary, paths, _source) = scanned_data_clear_fixture().await;
        let shm = sqlite_sidecar_path(&paths.database_path, "-shm");
        let moved_shm = temporary.path().join("moved-agentlog-shm");
        let shm_was_present = shm.exists();

        let error = inspect_clear_target_with_hook(&paths.database_path, || {
            if shm_was_present {
                fs::rename(&shm, &moved_shm).expect("move existing SHM during snapshot");
            } else {
                fs::write(&shm, b"appeared during snapshot").expect("create SHM during snapshot");
            }
        })
        .expect_err("reject changed SQLite file-set presence");

        assert!(format!("{error:#}").contains("file set changed"));
    }

    #[test]
    fn data_clear_snapshot_rejects_an_oversized_sparse_file_before_reading_it() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let database = temporary.path().join("agentlog.sqlite3");
        let file = fs::File::create(&database).expect("create sparse database");
        file.set_len(DATA_CLEAR_SNAPSHOT_MAX_BYTES + 1)
            .expect("size sparse database");

        let error =
            inspect_clear_target(&database).expect_err("reject oversized metadata before reading");

        assert!(format!("{error:#}").contains("exceeds the 536870912 byte snapshot limit"));
    }

    #[tokio::test]
    async fn confirmed_data_clear_reports_counts_from_its_clear_transaction() {
        let (_temporary, paths, _source) = scanned_data_clear_fixture().await;
        let preview = clear_data_shell(&paths, false)
            .await
            .expect("preview initial catalog");
        assert_eq!(preview.sessions, 1);
        assert_eq!(preview.transcript_items, 1);

        let writer = open_database(&paths.database_path)
            .await
            .expect("open catalog for intervening write");
        let source_id = sqlx::query(
            "INSERT INTO sources (provider, source_format, canonical_locator)
             VALUES ('test', 'test', 'test://transaction-counts')",
        )
        .execute(&writer)
        .await
        .expect("insert intervening source")
        .last_insert_rowid();
        let session_id = sqlx::query(
            "INSERT INTO sessions (source_id, session_key, title)
             VALUES (?, 'transaction-counts', 'intervening session')",
        )
        .bind(source_id)
        .execute(&writer)
        .await
        .expect("insert intervening session")
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO transcript_items (session_id, ordinal, item_kind, content)
             VALUES (?, 0, 'user_text', 'intervening item')",
        )
        .bind(session_id)
        .execute(&writer)
        .await
        .expect("insert intervening transcript item");
        writer.close().await;

        let cleared = clear_data_shell(&paths, true)
            .await
            .expect("clear current transaction contents");
        assert_eq!(cleared.sessions, 2);
        assert_eq!(cleared.transcript_items, 2);
        assert!(cleared.cleared);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn confirmed_data_clear_rejects_a_final_component_symlink() {
        let (temporary, paths, _source) = scanned_data_clear_fixture().await;
        let real_database = temporary.path().join("real-agentlog.sqlite3");
        fs::rename(&paths.database_path, &real_database).expect("move real database");
        symlink(&real_database, &paths.database_path).expect("link database path");
        let real_before = fingerprint(&real_database);

        let error = clear_data_shell(&paths, true)
            .await
            .expect_err("reject final database symlink");

        assert!(format!("{error:#}").contains("symbolic-link component"));
        assert_eq!(fingerprint(&real_database), real_before);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn confirmed_data_clear_rejects_an_ancestor_symlink() {
        let (temporary, paths, _source) = scanned_data_clear_fixture().await;
        let alias = temporary.path().join("agentlog-alias");
        symlink(&paths.data_dir, &alias).expect("link Agentlog home");
        let aliased_paths = AppPaths::resolve(Some(alias)).expect("resolve aliased paths");
        let before = directory_fingerprint(&paths.data_dir);

        let error = clear_data_shell(&aliased_paths, true)
            .await
            .expect_err("reject Agentlog home symlink");

        assert!(format!("{error:#}").contains("symbolic-link component"));
        assert_eq!(directory_fingerprint(&paths.data_dir), before);
    }

    #[tokio::test]
    async fn confirmed_data_clear_empties_only_catalog_and_a_fresh_scan_rebuilds_it() {
        let (_temporary, paths, source) = scanned_data_clear_fixture().await;
        let config_before = fingerprint(&paths.config_path);
        let source_before = fingerprint(&source);

        let cleared = clear_data_shell(&paths, true)
            .await
            .expect("confirm data clear");
        assert!(cleared.cleared);
        assert_eq!(cleared.sessions, 1);
        assert_eq!(cleared.transcript_items, 1);
        assert!(paths.database_path.is_file());
        assert_eq!(fingerprint(&paths.config_path), config_before);
        assert_eq!(fingerprint(&source), source_before);

        assert!(
            list_shell(&paths, 10)
                .await
                .expect("list cleared catalog")
                .is_empty()
        );
        assert!(
            diagnostics_shell(&paths)
                .await
                .expect("list cleared diagnostics")
                .is_empty()
        );

        let rebuilt = sync_shell(&paths)
            .await
            .expect("fresh scan rebuilds catalog");
        assert_eq!(rebuilt.sessions_available, 1);
        let sessions = list_shell(&paths, 10).await.expect("list rebuilt catalog");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("retained request"));
    }

    #[tokio::test]
    async fn data_clear_rejects_a_lookalike_v1_database_without_changing_it() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&paths.database_path)
                    .create_if_missing(true),
            )
            .await
            .expect("create lookalike database");
        for statement in [
            "CREATE TABLE sources (id INTEGER PRIMARY KEY)",
            "CREATE TABLE sessions (id INTEGER PRIMARY KEY)",
            "CREATE TABLE transcript_items (id INTEGER PRIMARY KEY)",
            "PRAGMA user_version = 1",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create lookalike schema");
        }
        pool.close().await;
        let before = directory_fingerprint(&paths.data_dir);

        let error = clear_data_shell(&paths, false)
            .await
            .expect_err("reject lookalike database");
        assert!(format!("{error:#}").contains("schema v1 fingerprint"));
        assert_eq!(directory_fingerprint(&paths.data_dir), before);
    }

    #[tokio::test]
    async fn confirmed_data_clear_fails_when_another_sqlite_writer_is_active() {
        let (_temporary, paths, _source) = scanned_data_clear_fixture().await;
        let writer: SqlitePool = open_database(&paths.database_path)
            .await
            .expect("open competing writer");
        let transaction = writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("hold SQLite writer lock");

        assert!(clear_data_shell(&paths, true).await.is_err());
        transaction.rollback().await.expect("release writer lock");
        writer.close().await;
        assert_eq!(
            list_shell(&paths, 10)
                .await
                .expect("list retained catalog")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn shell_sync_creates_catalog_and_reports_empty_codex_collection() {
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

        let summary = sync_shell(&paths).await.expect("run shell sync");

        assert_eq!(summary.schema_version, SCHEMA_VERSION);
        assert_eq!(summary.collectors_installed, 6);
        assert_eq!(summary.sources_refreshed, 0);
        assert_eq!(summary.sources_partial, 0);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.sources_missing, 0);
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

        let summary = sync_shell(&paths).await.expect("run shell sync");

        assert_eq!(summary.sources_refreshed, 1);
        assert_eq!(summary.sources_failed, 1);
        assert_eq!(summary.sessions_available, 1);
        assert_eq!(summary.provider_summaries.len(), 6);
        let json = serde_json::to_value(&summary).expect("serialize summary");
        assert_eq!(json["provider_summaries"].as_array().map(Vec::len), Some(6));
    }

    #[tokio::test]
    async fn a_failed_sync_keeps_a_source_last_good_session_in_the_catalog() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        let gemini_root = temporary.path().join("gemini");
        let source = gemini_root.join("tmp/session.jsonl");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        fs::create_dir_all(source.parent().expect("Gemini source parent"))
            .expect("create Gemini directory");
        fs::write(
            &source,
            "{\"sessionId\":\"session\"}\n{\"type\":\"user\",\"content\":\"last good request\"}\n",
        )
        .expect("write valid Gemini source");
        fs::write(
            agentlog_home.join("config.toml"),
            format!(
                "[providers]\ncodex_root = \"{}\"\nclaude_root = \"{}\"\nopencode_root = \"{}\"\ngemini_root = \"{}\"\ncursor_root = \"{}\"\nkimi_root = \"{}\"\n",
                temporary.path().join("empty-codex").display(),
                temporary.path().join("empty-claude").display(),
                temporary.path().join("empty-opencode").display(),
                gemini_root.display(),
                temporary.path().join("empty-cursor").display(),
                temporary.path().join("empty-kimi").display(),
            ),
        )
        .expect("write config");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");

        let first = sync_shell(&paths).await.expect("initial sync");
        assert_eq!(first.sessions_available, 1);
        fs::write(&source, "not JSON\n").expect("make source malformed");

        let refresh = sync_shell(&paths).await.expect("failing sync");
        assert_eq!(refresh.sources_failed, 1);
        assert_eq!(refresh.sessions_available, 1);
        let sessions = super::list_shell(&paths, 10)
            .await
            .expect("list last-good session");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].source_diagnostic_status, "error");
        assert_eq!(sessions[0].title.as_deref(), Some("last good request"));
    }

    #[tokio::test]
    async fn a_disappeared_source_is_retained_and_diagnosed_missing() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        let gemini_root = temporary.path().join("gemini");
        let source = gemini_root.join("tmp/session.jsonl");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        fs::create_dir_all(source.parent().expect("Gemini source parent"))
            .expect("create Gemini directory");
        fs::write(
            &source,
            "{\"sessionId\":\"session\"}\n{\"type\":\"user\",\"content\":\"retained request\"}\n",
        )
        .expect("write valid Gemini source");
        fs::write(
            agentlog_home.join("config.toml"),
            format!(
                "[providers]\ncodex_root = \"{}\"\nclaude_root = \"{}\"\nopencode_root = \"{}\"\ngemini_root = \"{}\"\ncursor_root = \"{}\"\nkimi_root = \"{}\"\n",
                temporary.path().join("empty-codex").display(),
                temporary.path().join("empty-claude").display(),
                temporary.path().join("empty-opencode").display(),
                gemini_root.display(),
                temporary.path().join("empty-cursor").display(),
                temporary.path().join("empty-kimi").display(),
            ),
        )
        .expect("write config");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");

        assert_eq!(
            sync_shell(&paths)
                .await
                .expect("initial sync")
                .sessions_available,
            1
        );
        fs::remove_file(&source).expect("remove provider-owned fixture");

        let summary = sync_shell(&paths).await.expect("sync after disappearance");
        assert_eq!(summary.sources_missing, 1);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.sessions_available, 1);
        let json = serde_json::to_value(&summary).expect("serialize sync summary");
        assert_eq!(json["sources_missing"], 1);

        let sessions = list_shell(&paths, 10).await.expect("list retained session");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].source_diagnostic_status, "missing");
        assert_eq!(sessions[0].title.as_deref(), Some("retained request"));
        let diagnostics = diagnostics_shell(&paths).await.expect("read diagnostics");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.provider == "gemini"
                && diagnostic.diagnostic_status == "missing"
                && diagnostic.diagnostic_message.as_deref()
                    == Some("source was absent from a completed provider discovery")
        }));
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

        let summary = sync_shell(&paths).await.expect("run sync");

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
    async fn one_sync_accepts_one_representative_source_from_each_provider() {
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
        let opencode_pool = create_test_database(&opencode_source).await;
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
        let summary = sync_shell(&paths).await.expect("sync");
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
