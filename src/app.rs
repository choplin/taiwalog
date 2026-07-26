//! The small application workflows available before collectors are installed.

use std::path::PathBuf;

use serde::Serialize;

use crate::{
    paths::AppPaths,
    storage::{SCHEMA_VERSION, open_database},
};

/// Observable result of a shell scan before any collector exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScanSummary {
    pub schema_version: i64,
    pub collectors_installed: u8,
    pub sources_refreshed: u64,
    pub sessions_available: u64,
    pub message: &'static str,
}

/// Initializes the durable catalog but deliberately performs no collection.
///
/// # Errors
///
/// Returns an error when Agentlog-owned storage cannot be created, opened, or
/// queried.
pub async fn scan_shell(paths: &AppPaths) -> anyhow::Result<ScanSummary> {
    paths.ensure_data_dir()?;
    let database = open_database(&paths.database_path).await?;
    let sessions_available = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
        .fetch_one(&database)
        .await?;
    database.close().await;

    Ok(ScanSummary {
        schema_version: SCHEMA_VERSION,
        collectors_installed: 0,
        sources_refreshed: 0,
        sessions_available: u64::try_from(sessions_available)?,
        message: "catalog initialized; no collectors are installed in this build",
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
    pub provider_sources: &'static str,
}

#[must_use]
pub fn paths_report(paths: &AppPaths) -> PathsReport {
    PathsReport {
        agentlog_home: paths.home().to_path_buf(),
        config_file: paths.config_path.clone(),
        data_directory: paths.data_dir.clone(),
        database: paths.database_path.clone(),
        provider_sources: "not inspected: collectors are not installed in this build",
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::{paths::AppPaths, storage::SCHEMA_VERSION};

    use super::scan_shell;

    #[tokio::test]
    async fn shell_scan_creates_catalog_and_reports_no_collection() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let paths =
            AppPaths::resolve(Some(temporary.path().join("agentlog"))).expect("resolve paths");

        let summary = scan_shell(&paths).await.expect("run shell scan");

        assert_eq!(summary.schema_version, SCHEMA_VERSION);
        assert_eq!(summary.collectors_installed, 0);
        assert_eq!(summary.sources_refreshed, 0);
        assert_eq!(summary.sessions_available, 0);
        assert!(paths.database_path.is_file());
    }
}
