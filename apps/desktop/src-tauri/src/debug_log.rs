//! One line of the app's debug log, written to stderr with the time and the
//! part of the app it came from.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn debug_log_line(source: &str, message: &str) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    eprintln!("[openwhisper][{now_ms}][{source}] {message}");
}
