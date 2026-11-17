//! The app's saved settings and what is sent from them: the settings file
//! (which never holds the API key), the copy the settings menu sees, and the
//! settings each dictation starts with.
//!
//! This file needs no Tauri, so its tests also run in `core-tests`.

use crate::models::default_transcription_model;
use crate::transcription_sessions::SessionSettings;
use serde::{Deserialize, Serialize};

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
/// An empty key falls back to the `OPENAI_API_KEY` environment variable.
pub fn session_settings(settings: &AppSettings, profile_id: &str) -> SessionSettings {
    SessionSettings {
        profile_id: profile_id.to_string(),
        openai_api_key: settings.openai_api_key.trim().to_string(),
        transcription_model: settings.transcription_model.clone(),
        prompt: settings.transcription_prompt.clone(),
        refine_enabled: settings.refine_enabled,
        refine_prompt: settings.refine_prompt.clone(),
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
