//! Settings in `settings.json` (folder from `net::config_dir`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Interface language code; empty — the system language.
    pub language: String,
    pub auto_update: bool,
    /// Last update check (Unix seconds).
    pub last_update_check: u64,
    /// Version the user asked not to offer again.
    pub skipped_version: String,
    /// Speed test size in MB: 100, 1000 or 5000.
    pub speed_mb: u64,
    /// Capacity check mode: `quick` (raw probe) or `full` (fill free space).
    pub capacity_mode: String,
}

pub const SPEED_SIZES: [u64; 3] = [100, 1000, 5000];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: String::new(),
            auto_update: true,
            last_update_check: 0,
            skipped_version: String::new(),
            speed_mb: 1000,
            capacity_mode: "quick".into(),
        }
    }
}

fn file() -> std::path::PathBuf {
    crate::net::config_dir().join("settings.json")
}

impl Settings {
    pub fn from_json(text: &str) -> Self {
        let mut s: Settings = serde_json::from_str(text).unwrap_or_default();
        if !SPEED_SIZES.contains(&s.speed_mb) {
            s.speed_mb = 1000;
        }
        if s.capacity_mode != "full" {
            s.capacity_mode = "quick".into();
        }
        s
    }

    pub fn load() -> Self {
        std::fs::read_to_string(file()).map(|t| Self::from_json(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_unknown_fields() {
        let s = Settings::from_json(r#"{"autoUpdate":false,"skippedVersion":"0.4.0","speedMb":5000,"x":1}"#);
        assert!(!s.auto_update);
        assert_eq!((s.skipped_version.as_str(), s.speed_mb), ("0.4.0", 5000));
        assert_eq!(s.capacity_mode, "quick");
        assert_eq!(Settings::from_json(r#"{"speedMb":7}"#).speed_mb, 1000);
        assert_eq!(Settings::from_json("not json"), Settings::default());
        let back = Settings::from_json(&serde_json::to_string(&s).unwrap());
        assert_eq!(back, s);
    }
}
