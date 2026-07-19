//! Application configuration: server connection, watch folders, album and
//! metadata behavior, and scan/retry tuning. Persisted as TOML in the
//! platform config directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default interval (seconds) between full catch-up scans.
pub const DEFAULT_SCAN_INTERVAL_SECS: u64 = 300;

/// Default interval (seconds) used as the retry backoff cap for failed uploads.
pub const DEFAULT_RETRY_INTERVAL_SECS: u64 = 60;

/// Application configuration, loaded from / saved to a TOML file in the
/// platform config directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Base URL of the Immich server, e.g. `https://immich.example.com`.
    pub server_url: String,
    /// Immich API key used for `x-api-key` authentication.
    pub api_key: String,
    /// Folders to watch (recursively) for new screenshots.
    pub watch_folders: Vec<PathBuf>,
    /// Whether newly uploaded assets should be added to an album automatically.
    pub album_auto_add: bool,
    /// Name of the album to add uploads to, when `album_auto_add` is enabled.
    pub album_name: Option<String>,
    /// Whether parsed VRChat/VRCX screenshot metadata should be written to the
    /// Immich asset description.
    pub metadata_to_description: bool,
    /// Interval, in seconds, between full catch-up scans.
    pub scan_interval_secs: u64,
    /// Base retry interval, in seconds, for failed uploads (backoff cap).
    pub retry_interval_secs: u64,
    /// File extensions (without the leading dot, lowercase) eligible for upload.
    pub allowed_extensions: Vec<String>,
    /// Whether the app is registered to auto-start at login.
    pub auto_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            api_key: String::new(),
            watch_folders: default_watch_folders(),
            album_auto_add: false,
            album_name: None,
            metadata_to_description: true,
            scan_interval_secs: DEFAULT_SCAN_INTERVAL_SECS,
            retry_interval_secs: DEFAULT_RETRY_INTERVAL_SECS,
            allowed_extensions: default_allowed_extensions(),
            auto_start: false,
        }
    }
}

fn default_allowed_extensions() -> Vec<String> {
    ["png", "jpg", "jpeg", "webp"]
        .into_iter()
        .map(String::from)
        .collect()
}

/// Default watch folders suggested on first run. Empty on non-Windows
/// platforms; the app layer is expected to prompt the user to add folders.
fn default_watch_folders() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(dir) = windows_default_vrchat_folder() {
            return vec![dir];
        }
        Vec::new()
    }

    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Builds the default VRChat screenshot folder suggestion on Windows:
/// `%USERPROFILE%\Pictures\VRChat`. Returns `None` if `USERPROFILE` is unset.
#[cfg(target_os = "windows")]
fn windows_default_vrchat_folder() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|profile| {
        let mut path = PathBuf::from(profile);
        path.push("Pictures");
        path.push("VRChat");
        path
    })
}

/// Returns the platform config file path:
/// `%APPDATA%\vrc-immich-syncer\config.toml` on Windows,
/// `~/.config/vrc-immich-syncer/config.toml` on Linux, and the equivalent
/// `Application Support` path on macOS.
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "vrc-immich-syncer")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

impl Config {
    /// Loads the config from the platform-default path. If the file does not
    /// exist, returns `Config::default()` rather than erroring.
    pub fn load() -> anyhow::Result<Config> {
        match default_config_path() {
            Some(path) => Config::load_from(&path),
            None => {
                tracing::warn!("could not determine platform config directory; using defaults");
                Ok(Config::default())
            }
        }
    }

    /// Saves the config to the platform-default path, creating parent
    /// directories as needed.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = default_config_path()
            .ok_or_else(|| anyhow::anyhow!("could not determine platform config directory"))?;
        self.save_to(&path)
    }

    /// Loads the config from an explicit path. If the file does not exist,
    /// returns `Config::default()` rather than erroring.
    pub fn load_from(path: &Path) -> anyhow::Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let contents = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read config file {path:?}: {e}"))?;
        let config: Config = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse config file {path:?}: {e}"))?;
        Ok(config)
    }

    /// Saves the config to an explicit path, creating parent directories as
    /// needed.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!("failed to create config directory {parent:?}: {e}")
            })?;
        }
        let contents = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("failed to serialize config: {e}"))?;
        std::fs::write(path, contents)
            .map_err(|e| anyhow::anyhow!("failed to write config file {path:?}: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_match_spec() {
        let config = Config::default();
        assert_eq!(config.server_url, "");
        assert_eq!(config.api_key, "");
        assert!(!config.album_auto_add);
        assert_eq!(config.album_name, None);
        assert!(config.metadata_to_description);
        assert_eq!(config.scan_interval_secs, DEFAULT_SCAN_INTERVAL_SECS);
        assert_eq!(config.retry_interval_secs, DEFAULT_RETRY_INTERVAL_SECS);
        assert_eq!(
            config.allowed_extensions,
            vec![
                "png".to_string(),
                "jpg".to_string(),
                "jpeg".to_string(),
                "webp".to_string()
            ]
        );
        assert!(!config.auto_start);
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist").join("config.toml");
        let loaded = Config::load_from(&path).expect("load should not error on missing file");
        assert_eq!(loaded, Config::default());
    }

    #[test]
    fn round_trip_save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");

        let config = Config {
            server_url: "https://immich.example.com".to_string(),
            api_key: "secret-key".to_string(),
            watch_folders: vec![PathBuf::from("/home/user/Pictures/VRChat")],
            album_auto_add: true,
            album_name: Some("VRChat".to_string()),
            metadata_to_description: false,
            scan_interval_secs: 120,
            retry_interval_secs: 30,
            allowed_extensions: vec!["png".to_string()],
            auto_start: true,
        };

        config.save_to(&path).expect("save should succeed");
        assert!(path.exists());

        let loaded = Config::load_from(&path).expect("load should succeed");
        assert_eq!(loaded, config);
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("config.toml");
        assert!(!path.parent().unwrap().exists());

        Config::default()
            .save_to(&path)
            .expect("save should succeed");
        assert!(path.exists());
    }

    #[test]
    fn partial_toml_fills_missing_fields_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "server_url = \"https://example.com\"\n").unwrap();

        let loaded = Config::load_from(&path).expect("load should succeed");
        assert_eq!(loaded.server_url, "https://example.com");
        assert_eq!(loaded.scan_interval_secs, DEFAULT_SCAN_INTERVAL_SECS);
        assert_eq!(loaded.allowed_extensions, default_allowed_extensions());
    }
}
