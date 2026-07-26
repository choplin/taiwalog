//! The small application workflows available before collectors are installed.

use std::path::PathBuf;

use serde::Serialize;

use crate::{
    codex::collect_codex,
    paths::AppPaths,
    storage::{SCHEMA_VERSION, open_database},
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
    pub message: String,
}

/// Initializes the durable catalog and collects read-only Codex sources.
///
/// # Errors
///
/// Returns an error when Agentlog-owned storage cannot be created, opened, or
/// queried.
pub async fn scan_shell(paths: &AppPaths) -> anyhow::Result<ScanSummary> {
    paths.ensure_data_dir()?;
    let database = open_database(&paths.database_path).await?;
    let codex_report = collect_codex(&paths.codex_root()?, &database).await?;
    let sessions_available = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
        .fetch_one(&database)
        .await?;
    database.close().await;

    Ok(ScanSummary {
        schema_version: SCHEMA_VERSION,
        collectors_installed: 1,
        sources_refreshed: codex_report.refreshed_sources,
        sources_partial: codex_report.partial_sources,
        sources_failed: codex_report.failed_sources,
        sessions_available: u64::try_from(sessions_available)?,
        message: "Codex sources were read without modifying provider-owned files".to_owned(),
    })
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
            "Codex root (read-only): {}",
            paths.codex_root().map_or_else(
                |_| "unavailable".to_owned(),
                |path| path.display().to_string()
            )
        ),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use std::fs;

    use crate::{paths::AppPaths, storage::SCHEMA_VERSION};

    use super::scan_shell;

    #[tokio::test]
    async fn shell_scan_creates_catalog_and_reports_empty_codex_collection() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let agentlog_home = temporary.path().join("agentlog");
        fs::create_dir(&agentlog_home).expect("create Agentlog home");
        fs::write(
            agentlog_home.join("config.toml"),
            format!(
                "[providers]\ncodex_root = \"{}\"\n",
                temporary.path().join("empty-codex").display()
            ),
        )
        .expect("write config");
        let paths = AppPaths::resolve(Some(agentlog_home)).expect("resolve paths");

        let summary = scan_shell(&paths).await.expect("run shell scan");

        assert_eq!(summary.schema_version, SCHEMA_VERSION);
        assert_eq!(summary.collectors_installed, 1);
        assert_eq!(summary.sources_refreshed, 0);
        assert_eq!(summary.sources_partial, 0);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.sessions_available, 0);
        assert!(paths.database_path.is_file());
    }
}
