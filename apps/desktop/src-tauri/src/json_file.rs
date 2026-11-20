//! Saves and reads the app's small JSON files (settings, recording details)
//! so a crash or power cut while saving never loses them.
//!
//! A save writes the new text to `<name>.tmp` next to the file, flushes it to
//! the disk, then renames it over the file, so the file is always either the
//! old version or the new one, never half written. (`std::fs::rename`
//! replaces an existing file on Windows too.) The old version is kept as
//! `<name>.bak`. A read that finds the file damaged or missing uses the
//! backup instead. A `.tmp` left by a crash is never read and is overwritten
//! by the next save.

use crate::sync::lock;
use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Every save of a file uses the same `.tmp` path, so saves run one at a time.
static SAVING: Mutex<()> = Mutex::new(());

/// What a read found.
#[derive(Debug)]
pub enum JsonRead<T> {
    /// Neither the file nor its backup exists.
    Missing,
    Read(T),
    /// The file could not be read, so its backup was used. `problem` says
    /// what was wrong with the file.
    Recovered { value: T, problem: String },
}

pub fn backup_path(path: &Path) -> PathBuf {
    with_suffix(path, ".bak")
}

fn temp_path(path: &Path) -> PathBuf {
    with_suffix(path, ".tmp")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Saves `value` as pretty JSON at `path`, keeping the previous file as the
/// backup. A previous file that does not read back as `T` never replaces a
/// good backup.
pub fn write_json_atomic<T: Serialize + DeserializeOwned>(path: &Path, value: &T) -> Result<()> {
    let _saving = lock(&SAVING);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(value)?;
    let temp = temp_path(path);
    {
        let mut file = File::create(&temp)?;
        file.write_all(payload.as_bytes())?;
        file.sync_all()?;
    }
    if let Ok(previous) = fs::read(path) {
        if serde_json::from_slice::<T>(&previous).is_ok() {
            fs::copy(path, backup_path(path))?;
        }
    }
    fs::rename(&temp, path)?;
    Ok(())
}

/// Makes the backup a copy of the file as it is now, for when the previous
/// version holds something that must not stay on disk.
pub fn refresh_backup(path: &Path) -> Result<()> {
    let _saving = lock(&SAVING);
    fs::copy(path, backup_path(path))?;
    Ok(())
}

/// Reads the JSON file at `path`, or its backup if the file is damaged or
/// missing.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<JsonRead<T>> {
    let problem = match fs::read(path) {
        Ok(raw) => match serde_json::from_slice(&raw) {
            Ok(value) => return Ok(JsonRead::Read(value)),
            Err(e) => format!("{} is damaged: {e}", path.display()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            format!("{} is missing", path.display())
        }
        Err(e) => format!("could not read {}: {e}", path.display()),
    };
    let backup = backup_path(path);
    match fs::read(&backup) {
        Ok(raw) => match serde_json::from_slice(&raw) {
            Ok(value) => Ok(JsonRead::Recovered { value, problem }),
            Err(e) => Err(anyhow!("{problem}, and its backup is damaged too: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.exists() => {
            Ok(JsonRead::Missing)
        }
        Err(_) => Err(anyhow!("{problem}, and it has no backup")),
    }
}

/// Reads a JSON file saved with [`write_json_atomic`], falling
/// back to its backup (and saying so in the log) when it is damaged. Returns
/// None when the file has never been saved.
pub fn read_json_file<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    Ok(match read_json(path)? {
        JsonRead::Missing => None,
        JsonRead::Read(value) => Some(value),
        JsonRead::Recovered { value, problem } => {
            crate::debug_log::debug_log_line("storage", &format!("{problem}; using its backup"));
            Some(value)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Details {
        name: String,
        bytes: u64,
    }

    fn details(name: &str, bytes: u64) -> Details {
        Details {
            name: name.to_string(),
            bytes,
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "openwhisper-json-file-{}-{}",
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

    fn read(path: &Path) -> JsonRead<Details> {
        read_json(path).unwrap()
    }

    #[test]
    fn a_save_replaces_the_file_and_keeps_the_old_one_as_backup() {
        let dir = TempDir::new();
        let path = dir.0.join("nested").join("metadata.json");
        assert!(matches!(read(&path), JsonRead::Missing));

        write_json_atomic(&path, &details("first", 1)).unwrap();
        write_json_atomic(&path, &details("second", 2)).unwrap();

        assert!(matches!(read(&path), JsonRead::Read(d) if d == details("second", 2)));
        let backup: Details = serde_json::from_slice(&fs::read(backup_path(&path)).unwrap()).unwrap();
        assert_eq!(backup, details("first", 1));
        assert!(!temp_path(&path).exists(), "the temp file is renamed away");
        // The format stays the readable JSON the app always wrote.
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            serde_json::to_string_pretty(&details("second", 2)).unwrap()
        );
    }

    #[test]
    fn a_temp_file_left_by_a_crash_is_ignored() {
        let dir = TempDir::new();
        let path = dir.0.join("metadata.json");
        write_json_atomic(&path, &details("saved", 5)).unwrap();
        fs::write(temp_path(&path), b"{\"name\": \"half wri").unwrap();

        assert!(matches!(read(&path), JsonRead::Read(d) if d == details("saved", 5)));
        write_json_atomic(&path, &details("next", 6)).unwrap();
        assert!(matches!(read(&path), JsonRead::Read(d) if d == details("next", 6)));
    }

    #[test]
    fn a_damaged_file_is_recovered_from_its_backup() {
        let dir = TempDir::new();
        let path = dir.0.join("metadata.json");
        write_json_atomic(&path, &details("old", 1)).unwrap();
        write_json_atomic(&path, &details("new", 2)).unwrap();
        fs::write(&path, b"{\"name\": \"ne").unwrap();

        match read(&path) {
            JsonRead::Recovered { value, problem } => {
                assert_eq!(value, details("old", 1));
                assert!(problem.contains("damaged"), "{problem}");
            }
            other => panic!("expected a recovery, got {other:?}"),
        }
    }

    #[test]
    fn a_damaged_file_never_overwrites_a_good_backup() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        write_json_atomic(&path, &details("old", 1)).unwrap();
        write_json_atomic(&path, &details("good", 2)).unwrap();
        fs::write(&path, b"").unwrap();

        write_json_atomic(&path, &details("newest", 3)).unwrap();

        let backup: Details = serde_json::from_slice(&fs::read(backup_path(&path)).unwrap()).unwrap();
        assert_eq!(backup, details("old", 1));
        assert!(matches!(read(&path), JsonRead::Read(d) if d == details("newest", 3)));
    }

    #[test]
    fn valid_json_of_the_wrong_shape_never_overwrites_a_good_backup() {
        let dir = TempDir::new();
        let path = dir.0.join("metadata.json");
        write_json_atomic(&path, &details("old", 1)).unwrap();
        write_json_atomic(&path, &details("good", 2)).unwrap();
        fs::write(&path, b"null").unwrap();

        write_json_atomic(&path, &details("newest", 3)).unwrap();

        let backup: Details = serde_json::from_slice(&fs::read(backup_path(&path)).unwrap()).unwrap();
        assert_eq!(backup, details("old", 1));
    }

    #[test]
    fn saves_of_one_file_at_the_same_time_all_succeed() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        std::thread::scope(|scope| {
            for thread in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    for round in 0..50 {
                        write_json_atomic(path, &details(&"x".repeat(round * 100), thread)).unwrap();
                    }
                });
            }
        });
        assert!(matches!(read(&path), JsonRead::Read(d) if d.name.len() == 4900));
    }

    #[test]
    fn a_file_with_no_good_copy_is_an_error_not_missing() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        fs::write(&path, b"not json").unwrap();
        assert!(read_json::<Details>(&path).is_err());

        fs::write(backup_path(&path), b"also not json").unwrap();
        assert!(read_json::<Details>(&path).is_err());
    }

    #[test]
    fn refreshing_the_backup_drops_the_previous_version() {
        let dir = TempDir::new();
        let path = dir.0.join("settings.json");
        write_json_atomic(&path, &details("secret", 1)).unwrap();
        write_json_atomic(&path, &details("clean", 2)).unwrap();

        refresh_backup(&path).unwrap();

        let backup = fs::read_to_string(backup_path(&path)).unwrap();
        assert!(!backup.contains("secret"));
        assert!(backup.contains("clean"));
    }
}
