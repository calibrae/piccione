//! User-facing application settings.
//!
//! Persisted as JSON at `<data_dir>/settings.json` alongside `.db_key` and
//! the SQLite store. The file is plain text — no encryption — because none
//! of these values are sensitive on their own (privacy toggles, theme
//! preference). The encrypted SQLite store remains the place where
//! anything sensitive belongs.
//!
//! Forwards-compat strategy: missing fields deserialize to their defaults.
//! Unknown fields are dropped silently. Add new fields with `#[serde(default)]`
//! and never rename existing ones without a migration.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Theme selection. `Auto` follows the system's `prefers-color-scheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    Auto,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::Auto
    }
}

/// User preferences.
///
/// All fields default to "the privacy-conservative choice that still feels
/// like a normal Signal client". Disable a feature → we stop emitting the
/// relevant outgoing envelope, but keep accepting incoming envelopes of
/// the same kind. (Reciprocity is a UX policy decision per the Signal
/// app, mirrored here.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Whether to send delivery + read receipts back to senders. When false,
    /// outbound receipts are suppressed entirely; the auto-delivery hook in
    /// the receive loop becomes a no-op and `mark_conversation_read`
    /// short-circuits. Incoming receipts continue to update our own UI.
    #[serde(default = "default_true")]
    pub read_receipts: bool,

    /// Whether to send typing indicators (the "..." bubble). signalui
    /// doesn't generate outgoing typing events yet, so this is a forward
    /// declaration. Incoming typing events are handled regardless.
    #[serde(default = "default_true")]
    pub typing_indicators: bool,

    /// UI theme preference.
    #[serde(default)]
    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            read_receipts: true,
            typing_indicators: true,
            theme: Theme::Auto,
        }
    }
}

fn default_true() -> bool {
    true
}

impl Settings {
    /// Load from `<data_dir>/settings.json`. If the file is missing,
    /// unreadable, or malformed, returns the default settings without
    /// raising — bad JSON shouldn't lock the user out of the app.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Settings>(&s) {
                Ok(settings) => {
                    tracing::debug!(?path, "loaded settings");
                    settings
                }
                Err(e) => {
                    tracing::warn!("settings.json invalid ({}); using defaults", e);
                    Self::default()
                }
            },
            Err(_) => {
                tracing::debug!(?path, "no settings file; using defaults");
                Self::default()
            }
        }
    }

    /// Persist atomically: write to `settings.json.tmp` then rename. Avoids
    /// truncating the existing file mid-write if the process is killed.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let final_path = data_dir.join("settings.json");
        let tmp_path = data_dir.join("settings.json.tmp");
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("signalui-settings-{}", nanos));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn defaults_are_privacy_neutral() {
        let s = Settings::default();
        assert!(s.read_receipts);
        assert!(s.typing_indicators);
        assert_eq!(s.theme, Theme::Auto);
    }

    #[test]
    fn round_trip_through_disk() {
        let dir = tempdir();
        let mut s = Settings::default();
        s.read_receipts = false;
        s.theme = Theme::Dark;
        s.save(&dir).unwrap();
        let loaded = Settings::load(&dir);
        assert!(!loaded.read_receipts);
        assert_eq!(loaded.theme, Theme::Dark);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_returns_defaults() {
        let dir = tempdir();
        let s = Settings::load(&dir);
        assert!(s.read_receipts);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_json_returns_defaults() {
        let dir = tempdir();
        std::fs::write(dir.join("settings.json"), "{not valid json").unwrap();
        let s = Settings::load(&dir);
        assert!(s.read_receipts); // didn't panic, fell back to defaults
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_fields_get_defaults_forward_compat() {
        let dir = tempdir();
        // A future version of the file with only `theme` set.
        std::fs::write(dir.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        let s = Settings::load(&dir);
        assert!(s.read_receipts); // defaulted
        assert!(s.typing_indicators); // defaulted
        assert_eq!(s.theme, Theme::Dark); // honoured
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn atomic_save_via_tempfile() {
        let dir = tempdir();
        let s = Settings::default();
        s.save(&dir).unwrap();
        // No .tmp leftover.
        assert!(!dir.join("settings.json.tmp").exists());
        assert!(dir.join("settings.json").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
