//! One line of the app's debug log, with the part of the app it came from.
//! The app (`start_log` in lib.rs) sends each line, through Tauri's log
//! plugin, to stderr, for development, and to `openwhisper.log` in the app's
//! log folder: release builds on Windows have no console, so the file is the
//! only place a user's problem can be read back from.
//!
//! The file gets only what stderr gets: never dictated text, keys or prompts.

/// The log target of every app line; the app's logger writes only this one.
pub const LOG_TARGET: &str = "openwhisper";

pub fn debug_log_line(source: &str, message: &str) {
    log::info!(target: LOG_TARGET, "[{source}] {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct KeepLines;

    impl log::Log for KeepLines {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            LINES.lock().unwrap().push(format!(
                "{} {} {}",
                record.target(),
                record.level(),
                record.args()
            ));
        }
        fn flush(&self) {}
    }

    #[test]
    fn debug_log_line_is_an_info_line_of_the_app_target() {
        log::set_logger(&KeepLines).unwrap();
        log::set_max_level(log::LevelFilter::Info);
        debug_log_line("test", "reaches the logger");
        assert!(LINES
            .lock()
            .unwrap()
            .contains(&"openwhisper INFO [test] reaches the logger".to_string()));
    }
}
