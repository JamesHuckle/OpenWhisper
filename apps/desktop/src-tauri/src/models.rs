//! The transcription models the settings allow, read from the one shared list
//! in `packages/protocol/models.json` that the settings menu also reads. The list is built into the app, so nothing is read at run time.
//!
//! This file uses only serde so its tests run without Tauri.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const SHARED_MODELS_JSON: &str = include_str!("../../../../packages/protocol/models.json");

#[derive(Debug, Deserialize)]
struct SharedModels {
    default_transcription_model: String,
    live_transcription_model: String,
    transcription_models: Vec<TranscriptionModelEntry>,
    legacy_transcription_models: HashMap<String, String>,
    refine_model: String,
}

#[derive(Debug, Deserialize)]
struct TranscriptionModelEntry {
    id: String,
}

fn shared() -> &'static SharedModels {
    static SHARED: OnceLock<SharedModels> = OnceLock::new();
    SHARED.get_or_init(|| {
        serde_json::from_str(SHARED_MODELS_JSON)
            .expect("packages/protocol/models.json is not a valid model list")
    })
}

pub fn default_transcription_model() -> String {
    shared().default_transcription_model.clone()
}

pub fn is_supported_transcription_model(model: &str) -> bool {
    shared()
        .transcription_models
        .iter()
        .any(|entry| entry.id == model)
}

/// The model that types as the user speaks, over the Realtime websocket.
pub fn live_transcription_model() -> &'static str {
    &shared().live_transcription_model
}

/// The model that tidies a transcript; the settings text names it too.
pub fn refine_model() -> &'static str {
    &shared().refine_model
}

/// The model a dictation uses: a saved name from before the gpt-transcribe
/// family moves to its replacement, and a model the app does not offer is
/// refused.
pub fn session_transcription_model(model: &str) -> Result<String, String> {
    let model = model.trim();
    let model = shared()
        .legacy_transcription_models
        .get(model)
        .map(String::as_str)
        .unwrap_or(model);
    if is_supported_transcription_model(model) {
        Ok(model.to_string())
    } else {
        Err(format!("Unsupported transcription model: {model}"))
    }
}

/// Settings saved before the gpt-transcribe family move to its replacement;
/// any other model the app no longer offers falls back to the default.
pub fn migrate_transcription_model(model: &str) -> String {
    if is_supported_transcription_model(model) {
        return model.to_string();
    }
    match shared().legacy_transcription_models.get(model) {
        Some(replacement) if is_supported_transcription_model(replacement) => replacement.clone(),
        _ => default_transcription_model(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_list_offers_standard_and_live_transcription() {
        assert_eq!(default_transcription_model(), "gpt-transcribe");
        assert!(is_supported_transcription_model("gpt-transcribe"));
        assert!(is_supported_transcription_model("gpt-live-transcribe"));
        assert!(!is_supported_transcription_model("gpt-4o-mini-transcribe"));
        assert!(!is_supported_transcription_model("gpt-5.4"));
        assert!(!is_supported_transcription_model(""));
    }

    #[test]
    fn a_dictation_uses_an_offered_model_or_is_refused() {
        assert_eq!(live_transcription_model(), "gpt-live-transcribe");
        assert_eq!(refine_model(), "gpt-5-nano");
        assert_eq!(session_transcription_model("gpt-transcribe").unwrap(), "gpt-transcribe");
        assert_eq!(
            session_transcription_model("gpt-realtime-transcribe").unwrap(),
            "gpt-live-transcribe"
        );
        assert_eq!(
            session_transcription_model("whisper-1").unwrap_err(),
            "Unsupported transcription model: whisper-1"
        );
    }

    #[test]
    fn saved_legacy_models_move_to_their_replacement() {
        assert_eq!(migrate_transcription_model("gpt-4o-mini-transcribe"), "gpt-transcribe");
        assert_eq!(migrate_transcription_model("gpt-4o-transcribe"), "gpt-transcribe");
        assert_eq!(
            migrate_transcription_model("gpt-realtime-transcribe"),
            "gpt-live-transcribe"
        );
        assert_eq!(
            migrate_transcription_model("gpt-live-transcribe"),
            "gpt-live-transcribe"
        );
        assert_eq!(migrate_transcription_model("whisper-1"), "gpt-transcribe");
    }

    #[test]
    fn every_shared_entry_points_at_an_offered_model() {
        let models = shared();
        assert!(is_supported_transcription_model(&models.default_transcription_model));
        for replacement in models.legacy_transcription_models.values() {
            assert!(is_supported_transcription_model(replacement), "{replacement}");
        }
    }

    #[test]
    fn transcription_model_allowlist_matches_settings_ui() {
        assert!(is_supported_transcription_model("gpt-transcribe"));
        assert!(is_supported_transcription_model("gpt-live-transcribe"));
        assert!(!is_supported_transcription_model("gpt-4o-mini-transcribe"));
        assert!(!is_supported_transcription_model("gpt-5.4"));
    }

    #[test]
    fn saved_legacy_transcription_models_migrate() {
        assert_eq!(migrate_transcription_model("gpt-4o-mini-transcribe"), "gpt-transcribe");
        assert_eq!(migrate_transcription_model("gpt-4o-transcribe"), "gpt-transcribe");
        assert_eq!(
            migrate_transcription_model("gpt-realtime-transcribe"),
            "gpt-live-transcribe"
        );
        assert_eq!(
            migrate_transcription_model("gpt-live-transcribe"),
            "gpt-live-transcribe"
        );
    }
}
