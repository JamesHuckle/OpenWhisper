//! The finished parts of a long transcription, saved beside its recording.
//!
//! A long recording is sent in parts. Each part's text is saved as soon as it
//! arrives, so a retry after a failure uploads (and pays for) only the parts
//! that did not finish. The file is tagged with a key made from the audio's
//! size and modified time, the model, the prompt and the chunking version:
//! when any of them changes, the saved parts are ignored and the
//! transcription starts fresh.
//!
//! The desktop names the file (`parts.json` in the recording's folder) and
//! deletes it when the transcription completes or the recording is pruned.
//!
//! This file needs no Tauri, so its tests run in the `core` crate.

use crate::debug_log::debug_log_line;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Raise this when the way a recording is split into parts, or the prompt
/// each part is sent with, changes: older saved parts then no longer match.
/// 2: silent parts are skipped, and a part's prompt quotes the last part that
/// had text.
/// 3: the desktop's FLAC upload parts set the boundaries, and a part that
/// only repeats its prompt is saved with no text.
/// 4: the key is a hash of the inputs as a JSON array.
pub const CHUNKING_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPart {
    /// The part's byte range in the recording's audio data.
    pub start: u64,
    pub end: u64,
    /// Empty for a silent part, which is never sent. The next part's prompt
    /// is rebuilt from it, so no prompt is saved (files from older versions
    /// that have one are still read).
    pub text: String,
}

#[derive(Serialize, Deserialize)]
struct PartsFile {
    key: String,
    parts: Vec<SavedPart>,
}

/// The key saved parts must carry to be used for this audio and these settings.
pub fn parts_key(audio_path: &Path, model: &str, prompt: &str) -> io::Result<String> {
    let metadata = fs::metadata(audio_path)?;
    let modified_ns = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|age| age.as_nanos())
        .unwrap_or(0);
    Ok(parts_key_for(metadata.len(), modified_ns, model, prompt))
}

/// The key for audio of `audio_bytes` last changed at `modified_ns`
/// (nanoseconds since 1970): a SHA-256 hash of every input, in hex.
fn parts_key_for(audio_bytes: u64, modified_ns: u128, model: &str, prompt: &str) -> String {
    let material = serde_json::to_vec(&(
        audio_bytes,
        modified_ns.to_string(),
        model,
        prompt,
        CHUNKING_VERSION,
    ))
    .expect("numbers and strings always serialize");
    Sha256::digest(material)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The parts saved for one key. Without a path nothing is read or saved.
pub struct SavedParts {
    path: Option<PathBuf>,
    key: String,
    parts: Vec<SavedPart>,
}

impl SavedParts {
    pub fn new(path: Option<PathBuf>, key: String) -> Self {
        let parts = path
            .as_deref()
            .map(|path| load(path, &key))
            .unwrap_or_default();
        Self { path, key, parts }
    }

    /// The saved part `index`, when it covers exactly these bytes.
    pub fn get(&self, index: usize, start: u64, end: u64) -> Option<&SavedPart> {
        self.parts
            .get(index)
            .filter(|part| part.start == start && part.end == end)
    }

    /// Keeps a finished part and saves every part so far, atomically.
    ///
    /// A part that cannot be saved is only logged: the transcription goes
    /// on, and a retry sends that part again.
    pub fn add(&mut self, index: usize, part: SavedPart) {
        self.parts.truncate(index);
        self.parts.push(part);
        let Some(path) = &self.path else {
            return;
        };
        let file = PartsFile {
            key: self.key.clone(),
            parts: self.parts.clone(),
        };
        if let Err(error) = write_json_atomic(path, &file) {
            debug_log_line("parts", &format!("save_failed error={:?}", error.kind()));
        }
    }
}

fn load(path: &Path, key: &str) -> Vec<SavedPart> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            debug_log_line("parts", &format!("unreadable error={:?}", error.kind()));
            return Vec::new();
        }
    };
    match serde_json::from_str::<PartsFile>(&text) {
        Ok(saved) if saved.key == key => {
            if !saved.parts.is_empty() {
                debug_log_line(
                    "parts",
                    &format!("resuming saved_parts={}", saved.parts.len()),
                );
            }
            saved.parts
        }
        Ok(_) => Vec::new(),
        // A damaged file is the same as none: start fresh.
        Err(_) => {
            debug_log_line("parts", "unreadable error=json");
            Vec::new()
        }
    }
}

/// Writes `value` beside `path` and moves it into place, so a crash while
/// saving leaves the previous file, never a half-written one.
fn write_json_atomic(path: &Path, value: &PartsFile) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.tmp"));
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_audio_and_settings_always_get_the_same_key() {
        let prompt = "Glossary: Zoon \u{2014} caf\u{e9} \u{1f600}\n\"q\"\t\\";
        let key = parts_key_for(1234, 1_700_000_000_123_456_789, "gpt-transcribe", prompt);
        assert_eq!(
            key,
            parts_key_for(1234, 1_700_000_000_123_456_789, "gpt-transcribe", prompt)
        );
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn another_size_time_model_or_prompt_gets_its_own_key() {
        let key = parts_key_for(10, 20, "gpt-transcribe", "Zoon");
        assert_ne!(key, parts_key_for(10, 20, "gpt-transcribe-next", "Zoon"));
        assert_ne!(
            key,
            parts_key_for(10, 20, "gpt-transcribe", "Zoon Robotics")
        );
        assert_ne!(key, parts_key_for(10, 21, "gpt-transcribe", "Zoon"));
        assert_ne!(key, parts_key_for(11, 20, "gpt-transcribe", "Zoon"));
    }

    #[test]
    fn parts_saved_for_one_key_are_read_back_only_with_that_key() {
        let path =
            std::env::temp_dir().join(format!("openwhisper-parts-{}.json", uuid::Uuid::new_v4()));
        let part = SavedPart {
            start: 0,
            end: 10,
            text: "One.".into(),
        };
        let mut saved = SavedParts::new(Some(path.clone()), "key-1".into());
        saved.add(0, part.clone());
        let written = fs::read_to_string(&path).unwrap();

        let again = SavedParts::new(Some(path.clone()), "key-1".into());
        let other = SavedParts::new(Some(path.clone()), "key-2".into());
        fs::write(&path, "{not json").unwrap();
        let damaged = SavedParts::new(Some(path.clone()), "key-1".into());
        let _ = fs::remove_file(&path);

        assert_eq!(
            written,
            r#"{"key":"key-1","parts":[{"start":0,"end":10,"text":"One."}]}"#
        );
        assert_eq!(again.get(0, 0, 10), Some(&part));
        assert_eq!(again.get(0, 0, 11), None);
        assert_eq!(other.get(0, 0, 10), None);
        assert_eq!(damaged.get(0, 0, 10), None);
    }

    #[test]
    fn a_file_with_spaced_json_a_silent_part_and_an_older_saved_prompt_is_read() {
        let path =
            std::env::temp_dir().join(format!("openwhisper-parts-{}.json", uuid::Uuid::new_v4()));
        fs::write(
            &path,
            r#"{"key": "k", "parts": [{"start": 0, "end": 4, "text": ""},
                {"start": 4, "end": 8, "prompt": "Zoon", "text": "Two."}]}"#,
        )
        .unwrap();
        let saved = SavedParts::new(Some(path.clone()), "k".into());
        let _ = fs::remove_file(&path);

        assert_eq!(saved.get(0, 0, 4).map(|part| part.text.as_str()), Some(""));
        assert_eq!(
            saved.get(1, 4, 8).map(|part| part.text.as_str()),
            Some("Two.")
        );
    }
}
