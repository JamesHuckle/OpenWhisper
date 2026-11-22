//! The app's saved settings and what is sent from them: the settings file
//! (which never holds the API key), the copy the settings menu sees, and the
//! settings each dictation starts with.
//!
//! This file needs no Tauri, so its tests also run in `core-tests`.

use crate::debug_log::debug_log_line;
use crate::json_file;
use crate::models::default_transcription_model;
use crate::transcription_sessions::SessionSettings;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

fn default_transcription_prompt() -> String {
    "I was editing @README.md and checking @package.json in my IDE. Let me also look at @tsconfig.json and the @src directory.".to_string()
}

fn default_refine_enabled() -> bool {
    true
}

fn default_show_idle_pill() -> bool {
    true
}

fn default_hold_to_talk() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// The key in use. It is kept in the OS credential store, never written
    /// to the settings file.
    #[serde(skip)]
    pub openai_api_key: String,
    /// A plain-text key in the settings file, from versions before the
    /// credential store. It is moved out on start and stays only while that
    /// move keeps failing.
    #[serde(
        default,
        rename = "openaiApiKey",
        skip_serializing_if = "String::is_empty"
    )]
    pub plain_openai_api_key: String,
    #[serde(default = "default_transcription_model")]
    pub transcription_model: String,
    #[serde(default = "default_transcription_prompt")]
    pub transcription_prompt: String,
    #[serde(default = "default_refine_enabled")]
    pub refine_enabled: bool,
    #[serde(default)]
    pub refine_prompt: String,
    #[serde(default = "default_show_idle_pill")]
    pub show_idle_pill: bool,
    #[serde(default = "default_hold_to_talk")]
    pub hold_to_talk: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            openai_api_key: String::new(),
            plain_openai_api_key: String::new(),
            transcription_model: default_transcription_model(),
            transcription_prompt: default_transcription_prompt(),
            refine_enabled: true,
            refine_prompt: String::new(),
            show_idle_pill: true,
            hold_to_talk: true,
        }
    }
}

/// Saves the settings file at `path`. An error means the file was not
/// replaced, so the previous settings are still the saved ones.
pub fn save_settings_file(path: &Path, settings: &AppSettings) -> Result<()> {
    json_file::write_json_atomic(path, settings)?;
    // The backup is the previous file. Once the plain-text API key has left
    // the settings it must not stay behind in the backup either. The new
    // settings are saved by now, so a failed cleanup does not fail the save:
    // the backup is checked again, and replaced, on the next save.
    if settings.plain_openai_api_key.is_empty() && backup_may_hold_plain_api_key(path) {
        if let Err(e) = json_file::refresh_backup(path) {
            debug_log_line(
                "settings",
                &format!("saved, but could not remove the old API key from the backup: {e}"),
            );
        }
    }
    Ok(())
}

/// True when the backup holds a plain-text API key, or cannot be read or
/// parsed to tell. A missing backup holds nothing.
fn backup_may_hold_plain_api_key(settings_path: &Path) -> bool {
    match fs::read(json_file::backup_path(settings_path)) {
        Ok(raw) => serde_json::from_slice::<serde_json::Value>(&raw)
            .map_or(true, |backup| backup.get("openaiApiKey").is_some()),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FrontendSettings {
    pub has_openai_api_key: bool,
    pub openai_api_key_preview: Option<String>,
    pub transcription_model: String,
    pub transcription_prompt: String,
    pub refine_enabled: bool,
    pub refine_prompt: String,
    pub show_idle_pill: bool,
    pub hold_to_talk: bool,
}

pub fn frontend_settings_from(settings: &AppSettings) -> FrontendSettings {
    let openai_api_key_preview = masked_api_key_preview(&settings.openai_api_key);
    FrontendSettings {
        has_openai_api_key: openai_api_key_preview.is_some(),
        openai_api_key_preview,
        transcription_model: settings.transcription_model.clone(),
        transcription_prompt: settings.transcription_prompt.clone(),
        refine_enabled: settings.refine_enabled,
        refine_prompt: settings.refine_prompt.clone(),
        show_idle_pill: settings.show_idle_pill,
        hold_to_talk: settings.hold_to_talk,
    }
}

/// The settings one dictation uses: the key and model as well as the
/// prompts, read when it starts, so a later save applies to the next one.
pub fn session_settings(settings: &AppSettings, profile_id: &str) -> SessionSettings {
    SessionSettings {
        profile_id: profile_id.to_string(),
        openai_api_key: settings.openai_api_key.trim().to_string(),
        transcription_model: settings.transcription_model.clone(),
        prompt: settings.transcription_prompt.clone(),
        refine_enabled: settings.refine_enabled,
        refine_prompt: settings.refine_prompt.clone(),
        standard_only: false,
    }
}

pub fn masked_api_key_preview(api_key: &str) -> Option<String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return None;
    }

    let prefix: String = trimmed.chars().take(4).collect();
    let total_chars = trimmed.chars().count();
    if total_chars <= 4 {
        Some(prefix)
    } else {
        Some(format!("{prefix}{}", "*".repeat(8)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "openwhisper-settings-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn with_plain_key(key: &str) -> AppSettings {
        AppSettings {
            plain_openai_api_key: key.into(),
            ..AppSettings::default()
        }
    }

    #[test]
    fn a_save_without_the_plain_key_clears_it_from_the_backup_too() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        save_settings_file(&path, &with_plain_key("sk-legacy")).unwrap();
        save_settings_file(&path, &with_plain_key("sk-legacy")).unwrap();

        save_settings_file(&path, &AppSettings::default()).unwrap();

        assert!(!fs::read_to_string(&path).unwrap().contains("sk-legacy"));
        let backup = fs::read_to_string(json_file::backup_path(&path)).unwrap();
        assert!(!backup.contains("sk-legacy"), "{backup}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_backup_cleanup_still_saves_the_settings() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        // A backup holding the plain key that cannot be replaced.
        let backup = json_file::backup_path(&path);
        fs::write(&backup, br#"{"openaiApiKey":"sk-legacy"}"#).unwrap();
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o444)).unwrap();
        if fs::OpenOptions::new().write(true).open(&backup).is_ok() {
            println!("skipped: this user can write read-only files");
            return;
        }
        let changed = AppSettings {
            refine_prompt: "Use British spelling.".into(),
            ..AppSettings::default()
        };

        save_settings_file(&path, &changed).unwrap();

        let saved: AppSettings = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.refine_prompt, "Use British spelling.");
        // Still marked for cleanup, so the next save tries again.
        assert!(backup_may_hold_plain_api_key(&path));
    }

    #[test]
    fn a_backup_that_cannot_be_read_or_parsed_needs_cleanup() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        let backup = json_file::backup_path(&path);
        assert!(!backup_may_hold_plain_api_key(&path), "no backup is clean");

        fs::write(&backup, br#"{"refinePrompt":""}"#).unwrap();
        assert!(!backup_may_hold_plain_api_key(&path));
        fs::write(&backup, br#"{"openaiApiKey":"sk-legacy"}"#).unwrap();
        assert!(backup_may_hold_plain_api_key(&path));
        fs::write(&backup, b"{\"openaiApiKey\": \"sk-leg").unwrap();
        assert!(backup_may_hold_plain_api_key(&path), "unparsable");
        fs::remove_file(&backup).unwrap();
        fs::create_dir(&backup).unwrap();
        assert!(backup_may_hold_plain_api_key(&path), "unreadable");
    }

    #[test]
    fn an_unparsable_backup_is_replaced_with_the_clean_file() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        fs::write(json_file::backup_path(&path), b"{\"openaiApiKey\": \"sk-legacy").unwrap();

        save_settings_file(&path, &AppSettings::default()).unwrap();

        let backup = fs::read_to_string(json_file::backup_path(&path)).unwrap();
        assert!(!backup.contains("sk-legacy"), "{backup}");
        assert_eq!(backup, fs::read_to_string(&path).unwrap());
    }

    #[test]
    fn a_save_with_no_backup_creates_none() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");

        save_settings_file(&path, &AppSettings::default()).unwrap();

        assert!(!json_file::backup_path(&path).exists());
    }

    #[test]
    fn a_save_that_keeps_the_plain_key_keeps_the_previous_file_as_backup() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        save_settings_file(&path, &with_plain_key("sk-legacy")).unwrap();
        let changed = AppSettings {
            refine_prompt: "Use British spelling.".into(),
            ..with_plain_key("sk-legacy")
        };

        save_settings_file(&path, &changed).unwrap();

        let backup = fs::read_to_string(json_file::backup_path(&path)).unwrap();
        assert!(backup.contains("sk-legacy"));
        assert!(!backup.contains("British"), "the backup is the previous file");
    }

    #[test]
    fn settings_file_never_contains_the_api_key() {
        let settings = AppSettings {
            openai_api_key: "sk-secret".into(),
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("sk-secret"));
        assert!(!json.contains("openaiApiKey"));
    }

    #[test]
    fn each_dictation_sends_the_saved_key_model_and_prompts() {
        let settings = AppSettings {
            openai_api_key: " sk-saved \n".into(),
            transcription_model: "gpt-live-transcribe".into(),
            transcription_prompt: "Glossary: Zoon".into(),
            refine_enabled: false,
            refine_prompt: "Use British spelling.".into(),
            ..AppSettings::default()
        };
        assert_eq!(
            session_settings(&settings, "default"),
            SessionSettings {
                profile_id: "default".into(),
                openai_api_key: "sk-saved".into(),
                transcription_model: "gpt-live-transcribe".into(),
                prompt: "Glossary: Zoon".into(),
                refine_enabled: false,
                refine_prompt: "Use British spelling.".into(),
                standard_only: false,
            }
        );
        let without_key = session_settings(&AppSettings::default(), "default");
        assert_eq!(without_key.openai_api_key, "");
    }

    #[test]
    fn plain_key_from_older_versions_is_read_and_kept_until_moved() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"openaiApiKey":"sk-legacy","holdToTalk":false}"#).unwrap();
        assert_eq!(settings.plain_openai_api_key, "sk-legacy");
        assert_eq!(settings.openai_api_key, "");
        assert!(!settings.hold_to_talk);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#""openaiApiKey":"sk-legacy""#));
    }

    #[test]
    fn settings_without_idle_pill_preference_keep_showing_it() {
        let settings: AppSettings = serde_json::from_value(json!({})).unwrap();

        assert!(settings.show_idle_pill);
        assert!(frontend_settings_from(&settings).show_idle_pill);
    }

    #[test]
    fn settings_without_shortcut_preference_keep_hold_to_talk() {
        let settings: AppSettings = serde_json::from_value(json!({})).unwrap();

        assert!(settings.hold_to_talk);
        assert!(frontend_settings_from(&settings).hold_to_talk);
    }
}
