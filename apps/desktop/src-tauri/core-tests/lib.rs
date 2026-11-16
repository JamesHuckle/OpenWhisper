//! Builds the desktop app's Rust files that need no Tauri, so their tests run
//! on any system without the webview libraries. Each file keeps its own tests;
//! add a file here when it gains tests and uses only the dependencies above.
#![allow(dead_code)]

#[path = "../src/api_key_store.rs"]
mod api_key_store;
#[path = "../src/flac_upload.rs"]
mod flac_upload;
#[path = "../src/foreground_app.rs"]
mod foreground_app;
#[path = "../src/json_file.rs"]
mod json_file;
#[path = "../src/models.rs"]
mod models;
#[path = "../src/paste_target.rs"]
mod paste_target;
#[path = "../src/recording_store.rs"]
mod recording_store;
#[path = "../src/worker_connection.rs"]
mod worker_connection;
