//! Agentlog-owned paths and the deliberately small local configuration file.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

const APPLICATION_SUPPORT: &str = "Library/Application Support/Agentlog";
const CONFIG_FILE: &str = "config.toml";
const DATABASE_FILE: &str = "agentlog.sqlite3";

/// Locations that Agentlog owns and may create or modify.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    home: PathBuf,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub database_path: PathBuf,
    provider_roots: ProviderRoots,
}

impl AppPaths {
    /// Resolves the macOS Application Support location, or the explicit test
    /// override. `AGENTLOG_HOME` has the same meaning as `--home`.
    ///
    /// # Errors
    ///
    /// Returns an error when no home directory is available, config is invalid,
    /// or a configured provider root is relative.
    pub fn resolve(home_override: Option<PathBuf>) -> Result<Self, PathError> {
        let home = match home_override.or_else(|| env::var_os("AGENTLOG_HOME").map(PathBuf::from)) {
            Some(home) => home,
            None => default_home()?,
        };

        if home.as_os_str().is_empty() {
            return Err(PathError::EmptyHome);
        }

        let config_path = home.join(CONFIG_FILE);
        let provider_roots = Config::load(&config_path)?;

        Ok(Self {
            config_path,
            data_dir: home.clone(),
            database_path: home.join(DATABASE_FILE),
            provider_roots,
            home,
        })
    }

    /// Creates only directories owned by Agentlog. Provider sources are never
    /// created, inspected, or changed here.
    ///
    /// # Errors
    ///
    /// Returns an error when the Agentlog-owned directory cannot be created.
    pub fn ensure_data_dir(&self) -> Result<(), PathError> {
        fs::create_dir_all(&self.data_dir).map_err(|source| PathError::CreateDirectory {
            path: self.data_dir.clone(),
            source,
        })
    }

    /// The root that contains Agentlog config and data. Exposed for diagnostics.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Explicit provider-owned roots from the optional config. Collectors use
    /// them only as read-only discovery roots in later commits.
    #[must_use]
    pub fn provider_roots(&self) -> &ProviderRoots {
        &self.provider_roots
    }
}

fn default_home() -> Result<PathBuf, PathError> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
        .map(|home| home.join(APPLICATION_SUPPORT))
        .ok_or(PathError::HomeUnavailable)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    providers: ProviderRoots,
}

impl Config {
    fn load(path: &Path) -> Result<ProviderRoots, PathError> {
        match fs::read_to_string(path) {
            Ok(contents) => toml::from_str::<Self>(&contents)
                .map_err(|source| PathError::InvalidConfig {
                    path: path.to_path_buf(),
                    source,
                })?
                .providers
                .validate(),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(ProviderRoots::default())
            }
            Err(source) => Err(PathError::ReadConfig {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRoots {
    #[serde(default, rename = "codex_root")]
    codex: Option<PathBuf>,
    #[serde(default, rename = "claude_root")]
    claude: Option<PathBuf>,
    #[serde(default, rename = "opencode_root")]
    opencode: Option<PathBuf>,
    #[serde(default, rename = "gemini_root")]
    gemini: Option<PathBuf>,
    #[serde(default, rename = "cursor_root")]
    cursor: Option<PathBuf>,
    #[serde(default, rename = "kimi_root")]
    kimi: Option<PathBuf>,
}

impl ProviderRoots {
    fn validate(self) -> Result<Self, PathError> {
        for (provider, root) in [
            ("codex", &self.codex),
            ("claude", &self.claude),
            ("opencode", &self.opencode),
            ("gemini", &self.gemini),
            ("cursor", &self.cursor),
            ("kimi", &self.kimi),
        ] {
            if let Some(root) = root
                && !root.is_absolute()
            {
                return Err(PathError::RelativeProviderRoot {
                    provider,
                    path: root.clone(),
                });
            }
        }

        Ok(self)
    }

    #[must_use]
    pub fn codex_root(&self) -> Option<&Path> {
        self.codex.as_deref()
    }

    #[must_use]
    pub fn claude_root(&self) -> Option<&Path> {
        self.claude.as_deref()
    }

    #[must_use]
    pub fn opencode_root(&self) -> Option<&Path> {
        self.opencode.as_deref()
    }

    #[must_use]
    pub fn gemini_root(&self) -> Option<&Path> {
        self.gemini.as_deref()
    }

    #[must_use]
    pub fn cursor_root(&self) -> Option<&Path> {
        self.cursor.as_deref()
    }

    #[must_use]
    pub fn kimi_root(&self) -> Option<&Path> {
        self.kimi.as_deref()
    }
}

/// Errors while resolving Agentlog-owned local state.
#[derive(Debug, Error)]
pub enum PathError {
    #[error("Agentlog home cannot be empty")]
    EmptyHome,
    #[error("cannot resolve the default Agentlog home because HOME is unavailable")]
    HomeUnavailable,
    #[error("cannot read Agentlog config at {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Agentlog config at {path} is invalid: {source}")]
    InvalidConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("providers.{provider}_root must be an absolute path, got {path}")]
    RelativeProviderRoot {
        provider: &'static str,
        path: PathBuf,
    },
    #[error("cannot create Agentlog data directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::TempDir;

    use super::AppPaths;

    #[test]
    fn explicit_home_keeps_database_inside_agentlog_owned_directory() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let paths =
            AppPaths::resolve(Some(temporary.path().join("agentlog"))).expect("paths resolve");

        assert_eq!(paths.home(), paths.data_dir);
        assert_eq!(paths.database_path, paths.data_dir.join("agentlog.sqlite3"));
        assert_eq!(paths.config_path, paths.data_dir.join("config.toml"));
    }

    #[test]
    fn config_accepts_explicit_provider_roots_without_changing_owned_database_path() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let home = temporary.path().join("agentlog");
        fs::create_dir(&home).expect("create home");
        fs::write(
            home.join("config.toml"),
            "[providers]\ncodex_root = '/private/tmp/codex'\n",
        )
        .expect("write config");

        let paths = AppPaths::resolve(Some(home.clone())).expect("paths resolve");

        assert_eq!(paths.database_path, home.join("agentlog.sqlite3"));
        assert_eq!(
            paths.provider_roots().codex_root(),
            Some(Path::new("/private/tmp/codex"))
        );
    }

    #[test]
    fn config_rejects_unrelated_configuration() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let home = temporary.path().join("agentlog");
        fs::create_dir(&home).expect("create home");
        fs::write(home.join("config.toml"), "[enrichment]\nenabled = true\n")
            .expect("write config");

        let error = AppPaths::resolve(Some(home)).expect_err("unrelated config must fail");

        assert!(error.to_string().contains("invalid"));
    }

    #[test]
    fn config_rejects_relative_provider_roots() {
        let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
        let home = temporary.path().join("agentlog");
        fs::create_dir(&home).expect("create home");
        fs::write(
            home.join("config.toml"),
            "[providers]\nclaude_root = 'relative/path'\n",
        )
        .expect("write config");

        let error = AppPaths::resolve(Some(home)).expect_err("relative root must fail");

        assert!(error.to_string().contains("providers.claude_root"));
    }
}
