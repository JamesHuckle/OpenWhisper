//! One line of the app's debug log, with the time and the part of the app it
//! came from. Each line goes to stderr, for development, and to
//! `openwhisper.log` in the app's log folder once `init_log_file` has opened
//! it: release builds on Windows have no console, so the file is the only
//! place a user's problem can be read back from.
//!
//! The file gets only what stderr gets: never dictated text, keys or prompts.
//! When it passes `MAX_LOG_BYTES` it is renamed to `openwhisper.log.1`
//! (replacing the older one) and a new file is started, so the folder holds
//! at most about twice that.

use std::fs::{self, File, OpenOptions};
use std::io::{self, LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_FILE_NAME: &str = "openwhisper.log";
pub const OLD_LOG_FILE_NAME: &str = "openwhisper.log.1";
const MAX_LOG_BYTES: u64 = 1024 * 1024;

static LOG: Mutex<Option<LogFile>> = Mutex::new(None);

/// Opens `dir/openwhisper.log` for appending; every later line is also
/// written there. A file already past the size limit is set aside first.
/// A folder that cannot be opened leaves logging on stderr only.
pub fn init_log_file(dir: &Path) {
    match LogFile::open(dir, MAX_LOG_BYTES) {
        Ok(file) => {
            if let Ok(mut log) = LOG.lock() {
                *log = Some(file);
            }
        }
        Err(error) => debug_log_line("log", &format!("could not open the log file: {error}")),
    }
}

pub fn debug_log_line(source: &str, message: &str) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = format!("[openwhisper][{now_ms}][{source}] {message}");
    eprintln!("{line}");
    if let Ok(mut log) = LOG.lock() {
        if let Some(file) = log.as_mut() {
            // A full disk or a removed folder must never stop the app.
            let _ = file.write_line(&line);
        }
    }
}

/// The open log file and how many bytes it holds.
struct LogFile {
    path: PathBuf,
    writer: LineWriter<File>,
    len: u64,
    max_len: u64,
}

impl LogFile {
    fn open(dir: &Path, max_len: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOG_FILE_NAME);
        let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let file = if len >= max_len {
            start_new_file(&path)?
        } else {
            append_to(&path)?
        };
        let len = file.metadata()?.len();
        Ok(Self {
            path,
            writer: LineWriter::new(file),
            len,
            max_len,
        })
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        if self.len >= self.max_len {
            self.writer.flush()?;
            let file = start_new_file(&self.path)?;
            // The old handle closes here, after the rename. Rust opens files
            // with delete sharing on Windows, so renaming one still open works.
            self.writer = LineWriter::new(file);
            self.len = 0;
        }
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.len += line.len() as u64 + 1;
        Ok(())
    }
}

fn append_to(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Moves the full log to `openwhisper.log.1` and opens an empty one. If it
/// cannot be moved, the full log is emptied instead so it never grows
/// without end.
fn start_new_file(path: &Path) -> io::Result<File> {
    let old = path.with_file_name(OLD_LOG_FILE_NAME);
    let _ = fs::remove_file(&old);
    if fs::rename(path, &old).is_ok() {
        append_to(path)
    } else {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("openwhisper-log-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lines_are_written_to_the_log_file() {
        let dir = temp_dir("lines");
        let mut log = LogFile::open(&dir.join("logs"), MAX_LOG_BYTES).unwrap();
        log.write_line("[openwhisper][1][recording] first").unwrap();
        log.write_line("[openwhisper][2][refine] second").unwrap();
        drop(log);

        // Reopening appends rather than starting over.
        let mut log = LogFile::open(&dir.join("logs"), MAX_LOG_BYTES).unwrap();
        log.write_line("[openwhisper][3][window] third").unwrap();

        let text = fs::read_to_string(dir.join("logs").join(LOG_FILE_NAME)).unwrap();
        assert_eq!(
            text,
            "[openwhisper][1][recording] first\n\
             [openwhisper][2][refine] second\n\
             [openwhisper][3][window] third\n"
        );
        assert!(!dir.join("logs").join(OLD_LOG_FILE_NAME).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_full_log_is_kept_as_one_old_file_while_writing() {
        let dir = temp_dir("rotate");
        let mut log = LogFile::open(&dir, 50).unwrap();
        let line = "x".repeat(40);
        log.write_line(&format!("first {line}")).unwrap();
        log.write_line(&format!("second {line}")).unwrap();
        // Past 50 bytes: this line starts a new file, and so does the fifth.
        log.write_line("third").unwrap();
        log.write_line(&format!("fourth {line}")).unwrap();
        log.write_line(&format!("fifth {line}")).unwrap();

        let current = fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap();
        let old = fs::read_to_string(dir.join(OLD_LOG_FILE_NAME)).unwrap();
        // Only one old file is kept: the first two lines are gone.
        assert_eq!(current, format!("fifth {line}\n"));
        assert_eq!(old, format!("third\nfourth {line}\n"));
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_log_already_too_big_at_start_is_set_aside() {
        let dir = temp_dir("startup");
        fs::write(dir.join(LOG_FILE_NAME), vec![b'a'; 100]).unwrap();
        let mut log = LogFile::open(&dir, 64).unwrap();
        log.write_line("new run").unwrap();

        assert_eq!(
            fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap(),
            "new run\n"
        );
        assert_eq!(
            fs::metadata(dir.join(OLD_LOG_FILE_NAME)).unwrap().len(),
            100
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn debug_log_line_writes_to_the_file_once_it_is_open() {
        let dir = temp_dir("global");
        init_log_file(&dir);
        debug_log_line("test", "reaches the log file");
        let text = fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap();
        assert!(text.contains("][test] reaches the log file\n"), "{text}");
        // Other tests' lines may still arrive; stop writing before removing.
        *LOG.lock().unwrap() = None;
        fs::remove_dir_all(dir).unwrap();
    }
}
