//! Writes the raw audio backup of a recording and names imported files.
//!
//! A microphone recording is one 24 kHz mono 16-bit WAV file. It stays open
//! while the user speaks: each audio piece is appended at once, and the WAV
//! header (which states the audio length) is rewritten about once a second,
//! not for every piece. A crash therefore loses no audio; the header is at
//! most a second behind and [`repair_wav_header`] corrects it.
//!
//! This file has no dependencies so it can also be tested on its own with
//! `rustc --test`.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// Sample rate of the microphone audio the desktop UI sends.
pub const MICROPHONE_SAMPLE_RATE: u32 = 24_000;
/// How often the open recording's WAV header is brought up to date.
pub const HEADER_SYNC_INTERVAL: Duration = Duration::from_secs(1);
/// Audio files the import picker offers.
pub const IMPORT_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "mp4", "mpeg", "mpga", "webm", "ogg"];

const WAV_HEADER_BYTES: u64 = 44;

/// The 44-byte header of a PCM WAV file holding `pcm_bytes` of audio with
/// `sample_width` bytes per sample. Microphone recordings are mono 16-bit
/// (1 channel, 2 bytes).
pub fn wav_header(pcm_bytes: u32, channels: u16, sample_width: u16, sample_rate: u32) -> [u8; 44] {
    let frame_bytes = channels.saturating_mul(sample_width);
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36_u32.saturating_add(pcm_bytes)).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&channels.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32]
        .copy_from_slice(&(sample_rate.saturating_mul(u32::from(frame_bytes))).to_le_bytes());
    header[32..34].copy_from_slice(&frame_bytes.to_le_bytes());
    header[34..36].copy_from_slice(&(sample_width.saturating_mul(8)).to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&pcm_bytes.to_le_bytes());
    header
}

fn header_for(file_bytes: u64, sample_rate: u32) -> [u8; 44] {
    let pcm_bytes = file_bytes
        .saturating_sub(WAV_HEADER_BYTES)
        .min(u32::MAX as u64) as u32;
    wav_header(pcm_bytes, 1, 2, sample_rate)
}

/// The open WAV file of a microphone recording in progress.
pub struct PcmWavWriter {
    file: File,
    file_bytes: u64,
    sample_rate: u32,
    last_sync: Instant,
}

impl PcmWavWriter {
    /// Creates the file with a header for no audio, so it is a valid WAV at once.
    pub fn create(path: &Path, sample_rate: u32, now: Instant) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(&header_for(WAV_HEADER_BYTES, sample_rate))?;
        Ok(Self {
            file,
            file_bytes: WAV_HEADER_BYTES,
            sample_rate,
            last_sync: now,
        })
    }

    /// Adds audio to the end of the file. Returns true when the header was
    /// also brought up to date.
    pub fn append(&mut self, pcm: &[u8], now: Instant) -> io::Result<bool> {
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(pcm)?;
        self.file_bytes += pcm.len() as u64;
        if now.saturating_duration_since(self.last_sync) < HEADER_SYNC_INTERVAL {
            return Ok(false);
        }
        self.sync(now)?;
        Ok(true)
    }

    /// Size of the file, header included.
    pub fn file_bytes(&self) -> u64 {
        self.file_bytes
    }

    /// Writes the final header and closes the file; returns its size.
    pub fn finish(mut self) -> io::Result<u64> {
        self.sync(Instant::now())?;
        Ok(self.file_bytes)
    }

    fn sync(&mut self, now: Instant) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file
            .write_all(&header_for(self.file_bytes, self.sample_rate))?;
        self.file.flush()?;
        self.last_sync = now;
        Ok(())
    }
}

/// Makes a saved microphone recording's header match its length, for a file
/// that was not closed cleanly (the app stopped while recording). Returns the
/// file's size. A header that already matches is left alone, so the file's
/// modified time stays the same and a retry can reuse its saved parts.
pub fn repair_wav_header(path: &Path, sample_rate: u32) -> io::Result<u64> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let file_bytes = file.metadata()?.len();
    if file_bytes >= WAV_HEADER_BYTES {
        let expected = header_for(file_bytes, sample_rate);
        let mut current = [0; 44];
        file.read_exact(&mut current)?;
        if current != expected {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&expected)?;
            file.flush()?;
        }
    }
    Ok(file_bytes)
}

/// Audio file types by extension. A type's first row names its stored
/// copies and uploads; the later rows are other names for it. A type not
/// listed is treated as WebM.
const AUDIO_TYPES: &[(&str, &str)] = &[
    ("wav", "audio/wav"),
    ("wav", "audio/x-wav"),
    ("mp3", "audio/mpeg"),
    ("mp3", "audio/mp3"),
    ("mpeg", "audio/mpeg"),
    ("mpga", "audio/mpeg"),
    ("m4a", "audio/x-m4a"),
    ("mp4", "audio/mp4"),
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
    ("flac", "audio/x-flac"),
    ("webm", "audio/webm"),
];

/// The audio type of an imported file, from its name.
pub fn mime_type_for_file_name(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    AUDIO_TYPES
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(extension))
        .map_or("audio/webm", |(_, mime_type)| mime_type)
}

/// The file name extension (without the dot) for an audio type: what a
/// stored copy of an imported file is named, and how the transcription
/// service recognises an upload.
pub fn extension_for_mime_type(mime_type: &str) -> &'static str {
    AUDIO_TYPES
        .iter()
        .find(|(_, known)| *known == mime_type)
        .map_or("webm", |(extension, _)| extension)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempWav(PathBuf);

    impl TempWav {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let name = format!(
                "openwhisper-recording-store-{}-{}.wav",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            Self(std::env::temp_dir().join(name))
        }
    }

    impl Drop for TempWav {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn declared_pcm_bytes(wav: &[u8]) -> u32 {
        u32::from_le_bytes(wav[40..44].try_into().unwrap())
    }

    #[test]
    fn a_new_recording_is_a_valid_empty_wav() {
        let path = TempWav::new();
        let writer = PcmWavWriter::create(&path.0, MICROPHONE_SAMPLE_RATE, Instant::now()).unwrap();

        let wav = fs::read(&path.0).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 24_000);
        assert_eq!(declared_pcm_bytes(&wav), 0);
        assert_eq!(writer.file_bytes(), 44);
    }

    #[test]
    fn audio_is_written_at_once_and_the_header_at_most_once_a_second() {
        let path = TempWav::new();
        let start = Instant::now();
        let mut writer = PcmWavWriter::create(&path.0, MICROPHONE_SAMPLE_RATE, start).unwrap();

        // Pieces arrive about three times a second.
        assert!(!writer
            .append(&[1, 2], start + Duration::from_millis(330))
            .unwrap());
        assert!(!writer
            .append(&[3, 4], start + Duration::from_millis(660))
            .unwrap());
        let wav = fs::read(&path.0).unwrap();
        assert_eq!(&wav[44..], &[1, 2, 3, 4], "audio reaches the disk at once");
        assert_eq!(
            declared_pcm_bytes(&wav),
            0,
            "header waits for the next sync"
        );

        assert!(writer
            .append(&[5, 6], start + Duration::from_millis(1000))
            .unwrap());
        let wav = fs::read(&path.0).unwrap();
        assert_eq!(declared_pcm_bytes(&wav), 6);
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 42);

        assert!(!writer
            .append(&[7, 8], start + Duration::from_millis(1330))
            .unwrap());
        assert_eq!(writer.file_bytes(), 52);
        assert_eq!(writer.finish().unwrap(), 52);
        let wav = fs::read(&path.0).unwrap();
        assert_eq!(
            declared_pcm_bytes(&wav),
            8,
            "finish writes the exact length"
        );
        assert_eq!(&wav[44..], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn a_recording_left_open_by_a_crash_is_repaired() {
        let path = TempWav::new();
        let start = Instant::now();
        let mut writer = PcmWavWriter::create(&path.0, MICROPHONE_SAMPLE_RATE, start).unwrap();
        writer.append(&[0; 10], start).unwrap();
        drop(writer); // The app stopped before finishing the recording.
        assert_eq!(declared_pcm_bytes(&fs::read(&path.0).unwrap()), 0);

        assert_eq!(
            repair_wav_header(&path.0, MICROPHONE_SAMPLE_RATE).unwrap(),
            54
        );
        assert_eq!(declared_pcm_bytes(&fs::read(&path.0).unwrap()), 10);

        // A header that already matches is not written again, so the file
        // keeps its modified time (the key its saved parts are kept under).
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&path.0)
            .unwrap()
            .set_modified(old)
            .unwrap();
        repair_wav_header(&path.0, MICROPHONE_SAMPLE_RATE).unwrap();
        assert_eq!(fs::metadata(&path.0).unwrap().modified().unwrap(), old);
    }

    #[test]
    fn the_microphone_header_is_the_standard_44_bytes() {
        let expected = [
            &b"RIFF"[..],
            &40_u32.to_le_bytes(),
            b"WAVEfmt ",
            &16_u32.to_le_bytes(),
            &1_u16.to_le_bytes(),
            &1_u16.to_le_bytes(),
            &24_000_u32.to_le_bytes(),
            &48_000_u32.to_le_bytes(),
            &2_u16.to_le_bytes(),
            &16_u16.to_le_bytes(),
            b"data",
            &4_u32.to_le_bytes(),
        ]
        .concat();
        assert_eq!(wav_header(4, 1, 2, 24_000).to_vec(), expected);
    }

    #[test]
    fn imported_files_are_stored_under_one_extension_per_type() {
        for (name, stored) in [
            ("a.wav", "wav"),
            ("a.MP3", "mp3"),
            ("a.mpeg", "mp3"),
            ("a.mpga", "mp3"),
            ("a.m4a", "m4a"),
            ("a.mp4", "mp4"),
            ("a.ogg", "ogg"),
            ("a.webm", "webm"),
            ("Imported audio", "webm"),
        ] {
            assert_eq!(
                extension_for_mime_type(mime_type_for_file_name(name)),
                stored,
                "{name}"
            );
        }
    }

    #[test]
    fn imported_files_are_named_by_their_extension() {
        assert_eq!(mime_type_for_file_name("Memo.WAV"), "audio/wav");
        assert_eq!(mime_type_for_file_name("talk.mp3"), "audio/mpeg");
        assert_eq!(mime_type_for_file_name("talk.mpga"), "audio/mpeg");
        assert_eq!(mime_type_for_file_name("voice.m4a"), "audio/x-m4a");
        assert_eq!(mime_type_for_file_name("film.mp4"), "audio/mp4");
        assert_eq!(mime_type_for_file_name("clip.ogg"), "audio/ogg");
        assert_eq!(mime_type_for_file_name("clip.webm"), "audio/webm");
        for extension in IMPORT_EXTENSIONS {
            let name = format!("audio.{extension}");
            let mime_type = mime_type_for_file_name(&name);
            let stored = extension_for_mime_type(mime_type);
            assert_eq!(
                mime_type_for_file_name(&format!("audio.{stored}")),
                mime_type
            );
        }
    }
}
