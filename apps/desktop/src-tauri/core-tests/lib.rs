//! Builds the desktop app's Rust files that need no Tauri, so their tests run
//! on any system without the webview libraries. Each file keeps its own tests;
//! add a file here when it gains tests and uses only the dependencies above.
#![allow(dead_code)]

#[path = "../src/api_key_store.rs"]
mod api_key_store;
#[path = "../src/audio_rules.rs"]
mod audio_rules;
#[path = "../src/automation_client.rs"]
mod automation_client;
#[path = "../src/debug_log.rs"]
mod debug_log;
#[path = "../src/flac_upload.rs"]
mod flac_upload;
#[path = "../src/foreground_app.rs"]
mod foreground_app;
#[path = "../src/json_file.rs"]
mod json_file;
#[path = "../src/models.rs"]
mod models;
#[path = "../src/openai_api.rs"]
mod openai_api;
#[path = "../src/overlay_geometry.rs"]
mod overlay_geometry;
#[path = "../src/paste_target.rs"]
mod paste_target;
#[path = "../src/realtime.rs"]
mod realtime;
#[path = "../src/recording_store.rs"]
mod recording_store;
#[path = "../src/recordings.rs"]
mod recordings;
#[path = "../src/saved_parts.rs"]
mod saved_parts;
#[path = "../src/settings.rs"]
mod settings;
#[path = "../src/sync.rs"]
mod sync;
#[path = "../src/transcriber.rs"]
mod transcriber;
#[path = "../src/transcription_sessions.rs"]
mod transcription_sessions;
#[path = "../src/wav_file.rs"]
mod wav_file;
