//! The desktop app's Rust files that need no Tauri. The app (../src/lib.rs)
//! uses them from here, and their tests run on any system without the webview
//! libraries. A new file that needs neither Tauri nor the windows crate is
//! declared here, not in the app.

#[path = "../src/api_key_store.rs"]
pub mod api_key_store;
#[path = "../src/audio_rules.rs"]
pub mod audio_rules;
// Keeps the UI Automation client the Windows writer reuses.
#[path = "../src/automation_client.rs"]
pub mod automation_client;
#[path = "../src/debug_log.rs"]
pub mod debug_log;
#[path = "../src/flac_upload.rs"]
pub mod flac_upload;
#[path = "../src/json_file.rs"]
pub mod json_file;
#[path = "../src/models.rs"]
pub mod models;
#[path = "../src/openai_api.rs"]
pub mod openai_api;
#[path = "../src/overlay_geometry.rs"]
pub mod overlay_geometry;
#[path = "../src/paste_target.rs"]
pub mod paste_target;
#[path = "../src/realtime.rs"]
pub mod realtime;
#[path = "../src/recording_store.rs"]
pub mod recording_store;
#[path = "../src/recordings.rs"]
pub mod recordings;
#[path = "../src/saved_parts.rs"]
pub mod saved_parts;
#[path = "../src/settings.rs"]
pub mod settings;
#[path = "../src/sync.rs"]
pub mod sync;
#[path = "../src/transcriber.rs"]
pub mod transcriber;
// Dictation sessions: OpenAI is called from here, with no helper process.
#[path = "../src/transcription_sessions.rs"]
pub mod transcription_sessions;
#[path = "../src/wav_file.rs"]
pub mod wav_file;
