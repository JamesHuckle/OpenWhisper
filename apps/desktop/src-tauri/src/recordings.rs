//! The saved recordings: each is a folder holding its audio and a
//! `metadata.json` with its details. Only the newest
//! [`RECORDING_RETENTION_COUNT`] are kept.
//!
//! This file needs no Tauri, so its tests also run in `core-tests`.

use crate::json_file;
use crate::recording_store::{self, PcmWavWriter};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const RECORDING_RETENTION_COUNT: usize = 10;

pub struct ActiveRecording {
    pub writer: PcmWavWriter,
    pub metadata: RecordingMetadata,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMetadata {
    pub id: String,
    pub created_at_ms: u128,
    pub name: String,
    pub source: String,
    pub mime_type: String,
    pub file_name: String,
    pub status: String,
    pub bytes: u64,
    pub transcript: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStartResponse {
    pub id: String,
}

pub fn recording_metadata_path(root: &std::path::Path, id: &str) -> PathBuf {
    root.join(id).join("metadata.json")
}

pub fn read_recording_metadata(root: &std::path::Path, id: &str) -> Result<RecordingMetadata> {
    let path = recording_metadata_path(root, id);
    json_file::read_json_file(&path)?.ok_or_else(|| anyhow!("{} is missing", path.display()))
}

pub fn write_recording_metadata(root: &std::path::Path, metadata: &RecordingMetadata) -> Result<()> {
    json_file::write_json_atomic(&recording_metadata_path(root, &metadata.id), metadata)
}

pub fn list_recording_metadata(root: &std::path::Path) -> Result<Vec<RecordingMetadata>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut recordings = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Ok(metadata) = read_recording_metadata(root, &entry.file_name().to_string_lossy()) {
            recordings.push(metadata);
        }
    }
    recordings.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    Ok(recordings)
}

pub fn prune_recordings(root: &std::path::Path, keep: usize) -> Result<()> {
    for recording in list_recording_metadata(root)?.into_iter().skip(keep) {
        let path = root.join(recording.id);
        if path.starts_with(root) {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub fn lock_recordings(
    recordings: &std::sync::Mutex<HashMap<String, ActiveRecording>>,
) -> Result<std::sync::MutexGuard<'_, HashMap<String, ActiveRecording>>, String> {
    recordings
        .lock()
        .map_err(|_| "Failed to lock recordings".to_string())
}

/// Writes the final header and size of a recording still being written, and
/// closes its file. Does nothing for a recording that is not open.
pub fn close_active_recording(
    recordings: &std::sync::Mutex<HashMap<String, ActiveRecording>>,
    root: &std::path::Path,
    recording_id: &str,
) -> Result<(), String> {
    let Some(active) = lock_recordings(recordings)?.remove(recording_id) else {
        return Ok(());
    };
    let mut metadata = active.metadata;
    metadata.bytes = active.writer.finish().map_err(|e| e.to_string())?;
    write_recording_metadata(root, &metadata).map_err(|e| e.to_string())
}

/// Copies an audio file from disk into a new saved recording.
pub fn import_recording(
    root: &std::path::Path,
    source: &std::path::Path,
) -> Result<RecordingStartResponse, String> {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Imported audio".to_string());
    let size = fs::metadata(source)
        .map_err(|e| format!("Could not read the selected audio file: {e}"))?
        .len();
    if size == 0 {
        return Err("The selected audio file is empty".to_string());
    }
    let mime_type = recording_store::mime_type_for_file_name(&name);
    let file_name = format!("audio.{}", recording_store::safe_extension(&name, mime_type));
    let id = Uuid::new_v4().to_string();
    let dir = root.join(&id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let bytes = match fs::copy(source, dir.join(&file_name)) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = fs::remove_dir_all(&dir);
            return Err(format!("Could not copy the selected audio file: {error}"));
        }
    };
    let metadata = RecordingMetadata {
        id: id.clone(),
        created_at_ms: now_ms(),
        name,
        source: "import".to_string(),
        mime_type: mime_type.to_string(),
        file_name,
        status: "saved".to_string(),
        bytes,
        transcript: String::new(),
        error: String::new(),
    };
    write_recording_metadata(root, &metadata).map_err(|e| e.to_string())?;
    prune_recordings(root, RECORDING_RETENTION_COUNT).map_err(|e| e.to_string())?;
    Ok(RecordingStartResponse { id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn recording_store_keeps_ten_readable_wavs() {
        let root = std::env::temp_dir().join(format!("openwhisper-store-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for index in 0..12_u128 {
            let id = format!("recording-{index}");
            let directory = root.join(&id);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("audio.wav"), recording_store::wav_header(0, 24_000)).unwrap();
            write_recording_metadata(
                &root,
                &RecordingMetadata {
                    id: id.clone(),
                    created_at_ms: index,
                    name: format!("Recording {index}"),
                    source: "microphone".to_string(),
                    mime_type: "audio/wav".to_string(),
                    file_name: "audio.wav".to_string(),
                    status: "saved".to_string(),
                    bytes: 44,
                    transcript: String::new(),
                    error: String::new(),
                },
            )
            .unwrap();
        }

        prune_recordings(&root, RECORDING_RETENTION_COUNT).unwrap();
        let retained = list_recording_metadata(&root).unwrap();
        assert_eq!(retained.len(), RECORDING_RETENTION_COUNT);
        assert_eq!(retained.first().unwrap().id, "recording-11");
        assert_eq!(retained.last().unwrap().id, "recording-2");
        for recording in retained {
            let path = root.join(recording.id).join(recording.file_name);
            assert_eq!(&fs::read(path).unwrap()[0..4], b"RIFF");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_recording_with_damaged_details_is_listed_from_its_backup() {
        let root = std::env::temp_dir().join(format!("openwhisper-store-test-{}", Uuid::new_v4()));
        let mut metadata = RecordingMetadata {
            id: "recording".to_string(),
            created_at_ms: 1,
            name: "Recording".to_string(),
            source: "microphone".to_string(),
            mime_type: "audio/wav".to_string(),
            file_name: "audio.wav".to_string(),
            status: "recording".to_string(),
            bytes: 44,
            transcript: String::new(),
            error: String::new(),
        };
        write_recording_metadata(&root, &metadata).unwrap();
        metadata.status = "saved".to_string();
        write_recording_metadata(&root, &metadata).unwrap();
        // The app stopped part way through writing the details.
        fs::write(recording_metadata_path(&root, "recording"), b"{\"id\": \"rec").unwrap();

        let listed = list_recording_metadata(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "recording");
        assert_eq!(listed[0].status, "recording");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn closing_an_open_recording_writes_its_final_length() {
        let root = std::env::temp_dir().join(format!("openwhisper-wav-test-{}", Uuid::new_v4()));
        let id = "active";
        fs::create_dir_all(root.join(id)).unwrap();
        let started = Instant::now();
        let mut writer =
            PcmWavWriter::create(&root.join(id).join("audio.wav"), 24_000, started).unwrap();
        let metadata = RecordingMetadata {
            id: id.to_string(),
            created_at_ms: 1,
            name: "Active".to_string(),
            source: "microphone".to_string(),
            mime_type: "audio/wav".to_string(),
            file_name: "audio.wav".to_string(),
            status: "recording".to_string(),
            bytes: writer.file_bytes(),
            transcript: String::new(),
            error: String::new(),
        };
        write_recording_metadata(&root, &metadata).unwrap();
        writer.append(&[1, 2, 3, 4], started).unwrap();
        let recordings = std::sync::Mutex::new(HashMap::new());
        recordings
            .lock()
            .unwrap()
            .insert(id.to_string(), ActiveRecording { writer, metadata });

        close_active_recording(&recordings, &root, id).unwrap();

        let wav = fs::read(root.join(id).join("audio.wav")).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[1, 2, 3, 4]);
        assert_eq!(read_recording_metadata(&root, id).unwrap().bytes, 48);
        assert!(recordings.lock().unwrap().is_empty());
        // Closing again (transcribe, then finish) is harmless.
        close_active_recording(&recordings, &root, id).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_audio_is_copied_from_disk_into_saved_recordings() {
        let folder = std::env::temp_dir().join(format!("openwhisper-import-test-{}", Uuid::new_v4()));
        let root = folder.join("recordings");
        fs::create_dir_all(&folder).unwrap();
        let source = folder.join("Voice memo.M4A");
        fs::write(&source, b"m4a audio").unwrap();

        let stored = import_recording(&root, &source).unwrap();

        let metadata = read_recording_metadata(&root, &stored.id).unwrap();
        assert_eq!(metadata.name, "Voice memo.M4A");
        assert_eq!(metadata.source, "import");
        assert_eq!(metadata.mime_type, "audio/x-m4a");
        assert_eq!(metadata.file_name, "audio.m4a");
        assert_eq!(metadata.bytes, 9);
        assert_eq!(fs::read(root.join(&stored.id).join("audio.m4a")).unwrap(), b"m4a audio");
        assert_eq!(fs::read(&source).unwrap(), b"m4a audio", "the original is untouched");

        let empty = folder.join("empty.wav");
        fs::write(&empty, b"").unwrap();
        assert_eq!(
            import_recording(&root, &empty).err().as_deref(),
            Some("The selected audio file is empty")
        );
        assert_eq!(list_recording_metadata(&root).unwrap().len(), 1);
        fs::remove_dir_all(folder).unwrap();
    }
}
