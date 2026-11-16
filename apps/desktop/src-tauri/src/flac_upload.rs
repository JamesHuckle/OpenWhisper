//! Makes the copy of a microphone recording that is uploaded for
//! transcription. FLAC is lossless: the service hears exactly the saved WAV,
//! in roughly half the bytes for speech, so the wait between stopping and the
//! text appearing is shorter, most of all on slow networks.
//!
//! The WAV stays the saved recording. The FLAC copy sits beside it only while
//! its transcription request runs. It is encoded one frame at a time straight
//! to disk, so a long recording is never held in memory.

use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, Stream, StreamInfo};
use flacenc::error::Verify;
use flacenc::source::{Context, Fill, FrameBuf};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The upload copy's name, in the recording's folder.
pub const FILE_NAME: &str = "audio.flac";
pub const MIME_TYPE: &str = "audio/flac";

/// Samples per channel in each FLAC frame (the reference encoder's default).
const BLOCK_SIZE: usize = 4096;
const BITS_PER_SAMPLE: usize = 16;
/// "fLaC" and the STREAMINFO block. It is written last, once the length and
/// checksum of the audio are known.
const HEADER_BYTES: usize = 42;

const SHARED_AUDIO_RULES_JSON: &str =
    include_str!("../../../../packages/protocol/audio-rules.json");

/// The largest file sent in one transcription request, shared with the worker
/// and the Android app (packages/protocol/audio-rules.json).
pub fn max_upload_bytes() -> u64 {
    serde_json::from_str::<serde_json::Value>(SHARED_AUDIO_RULES_JSON)
        .ok()
        .and_then(|rules| rules["max_upload_bytes"].as_u64())
        .expect("packages/protocol/audio-rules.json names max_upload_bytes")
}

/// Writes a FLAC copy of the 16-bit PCM WAV at `wav_path` to `flac_path`.
///
/// Returns false, and leaves no file, when the copy would be larger than
/// `max_bytes` or the WAV holds no audio: the worker then sends the WAV
/// itself, in parts when it is long.
pub fn write_upload_copy(wav_path: &Path, flac_path: &Path, max_bytes: u64) -> io::Result<bool> {
    let result = encode(wav_path, flac_path, max_bytes);
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(flac_path);
    }
    result
}

struct PcmWav {
    channels: usize,
    sample_rate: usize,
    data_offset: u64,
    data_bytes: u64,
}

fn encode(wav_path: &Path, flac_path: &Path, max_bytes: u64) -> io::Result<bool> {
    let mut wav = File::open(wav_path)?;
    let format = read_pcm16_format(&mut wav)?;
    wav.seek(SeekFrom::Start(format.data_offset))?;
    let mut pcm = BufReader::new(wav).take(format.data_bytes);

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, error)| encode_error(error))?;
    let mut info = StreamInfo::new(format.sample_rate, format.channels, BITS_PER_SAMPLE)
        .map_err(encode_error)?;
    let mut framebuf = FrameBuf::with_size(format.channels, BLOCK_SIZE).map_err(encode_error)?;
    let mut context = Context::new(BITS_PER_SAMPLE, format.channels);

    let mut out = BufWriter::new(File::create(flac_path)?);
    out.write_all(&[0; HEADER_BYTES])?;
    let mut written = HEADER_BYTES as u64;
    let sample_frame_bytes = format.channels * 2;
    let mut block = vec![0_u8; BLOCK_SIZE * sample_frame_bytes];
    loop {
        let filled = read_up_to(&mut pcm, &mut block)?;
        // A recording cut off mid-sample ends at its last whole sample.
        let whole = filled - filled % sample_frame_bytes;
        if whole == 0 {
            break;
        }
        framebuf.fill_le_bytes(&block[..whole], 2).map_err(encode_error)?;
        context.fill_le_bytes(&block[..whole], 2).map_err(encode_error)?;
        let frame_number = context.current_frame_number().unwrap_or(0);
        let frame = flacenc::encode_fixed_size_frame(&config, &framebuf, frame_number, &info)
            .map_err(encode_error)?;
        info.update_frame_info(&frame);
        let mut sink = ByteSink::new();
        frame.write(&mut sink).map_err(encode_error)?;
        written += sink.as_slice().len() as u64;
        if written > max_bytes {
            return Ok(false);
        }
        out.write_all(sink.as_slice())?;
        if filled < block.len() {
            break;
        }
    }
    if info.total_samples() == 0 {
        return Ok(false);
    }

    // Every frame but the last has the full block size.
    info.set_block_sizes(BLOCK_SIZE, BLOCK_SIZE).map_err(encode_error)?;
    info.set_md5_digest(&context.md5_digest());
    let mut header = ByteSink::new();
    Stream::with_stream_info(info)
        .write(&mut header)
        .map_err(encode_error)?;
    if header.as_slice().len() != HEADER_BYTES {
        return Err(io::Error::other("Unexpected FLAC header size"));
    }
    let mut file = out.into_inner().map_err(|error| error.into_error())?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(header.as_slice())?;
    Ok(true)
}

/// Reads until `buffer` is full or the audio ends; returns the bytes read.
fn read_up_to(source: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match source.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// Finds the format and audio of a 16-bit PCM WAV file. Leaves `file` at an
/// unspecified position.
fn read_pcm16_format(file: &mut File) -> io::Result<PcmWav> {
    let file_bytes = file.metadata()?.len();
    let mut riff = [0_u8; 12];
    file.read_exact(&mut riff)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err(invalid("Not a WAV file"));
    }
    let mut format = None;
    loop {
        let mut chunk = [0_u8; 8];
        file.read_exact(&mut chunk)?;
        let size = u64::from(u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
        let start = file.stream_position()?;
        match &chunk[0..4] {
            b"fmt " if size >= 16 => {
                let mut fmt = [0_u8; 16];
                file.read_exact(&mut fmt)?;
                let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
                let channels = usize::from(u16::from_le_bytes([fmt[2], fmt[3]]));
                let sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]) as usize;
                let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
                if tag != 1 || bits != 16 || channels == 0 || sample_rate == 0 {
                    return Err(invalid("Not a 16-bit PCM WAV file"));
                }
                format = Some((channels, sample_rate));
            }
            b"data" => {
                let (channels, sample_rate) =
                    format.ok_or_else(|| invalid("WAV audio comes before its format"))?;
                return Ok(PcmWav {
                    channels,
                    sample_rate,
                    data_offset: start,
                    // A recording the app did not finish can state less or
                    // more than the file holds.
                    data_bytes: size.min(file_bytes.saturating_sub(start)),
                });
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(start + size + size % 2))?;
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn encode_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("Could not encode FLAC: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording_store::wav_header;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("openwhisper-flac-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Speech-like test audio: a few tones that rise and fall, plus noise.
    fn samples(count: usize) -> Vec<i16> {
        let mut noise = 12_345_u32;
        (0..count)
            .map(|n| {
                let t = n as f64 / 24_000.0;
                let envelope = 0.5 + 0.5 * (t * 3.0).sin();
                let voice = (t * 220.0 * std::f64::consts::TAU).sin() * 6_000.0
                    + (t * 660.0 * std::f64::consts::TAU).sin() * 2_000.0;
                noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let hiss = f64::from((noise >> 16) as i16 % 200);
                (voice * envelope + hiss) as i16
            })
            .collect()
    }

    fn write_wav(path: &Path, samples: &[i16]) {
        let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut bytes = wav_header(pcm.len() as u32, 24_000).to_vec();
        bytes.extend_from_slice(&pcm);
        fs::write(path, bytes).unwrap();
    }

    fn decode(path: &Path) -> (u32, Vec<i16>) {
        let mut reader = claxon::FlacReader::open(path).unwrap();
        let rate = reader.streaminfo().sample_rate;
        let decoded = reader.samples().map(|s| s.unwrap() as i16).collect();
        (rate, decoded)
    }

    #[test]
    fn flac_copy_holds_exactly_the_saved_audio_in_fewer_bytes() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        let flac = dir.0.join(FILE_NAME);
        // Not a whole number of FLAC frames, so the short last frame is covered.
        let original = samples(24_000 * 3 + 1_234);
        write_wav(&wav, &original);

        assert!(write_upload_copy(&wav, &flac, max_upload_bytes()).unwrap());

        let (rate, decoded) = decode(&flac);
        assert_eq!(rate, 24_000);
        assert_eq!(decoded, original);
        let reader = claxon::FlacReader::open(&flac).unwrap();
        assert_eq!(reader.streaminfo().samples, Some(original.len() as u64));
        assert!(fs::metadata(&flac).unwrap().len() < fs::metadata(&wav).unwrap().len() * 3 / 4);
    }

    #[test]
    fn unfinished_recording_is_read_to_its_last_whole_sample() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        let flac = dir.0.join(FILE_NAME);
        let original = samples(5_000);
        write_wav(&wav, &original);
        // The header states less audio than the file holds, and half a sample
        // follows: as after a crash, before the header is repaired.
        let mut bytes = fs::read(&wav).unwrap();
        bytes[40..44].copy_from_slice(&(u32::MAX - 100).to_le_bytes());
        bytes.push(7);
        fs::write(&wav, bytes).unwrap();

        assert!(write_upload_copy(&wav, &flac, max_upload_bytes()).unwrap());
        assert_eq!(decode(&flac).1, original);
    }

    #[test]
    fn no_copy_when_it_would_be_too_large_or_empty() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        let flac = dir.0.join(FILE_NAME);
        write_wav(&wav, &samples(24_000));
        assert!(!write_upload_copy(&wav, &flac, 1_000).unwrap());
        assert!(!flac.exists());

        write_wav(&wav, &[]);
        assert!(!write_upload_copy(&wav, &flac, max_upload_bytes()).unwrap());
        assert!(!flac.exists());
    }

    #[test]
    fn a_file_that_is_not_pcm_wav_is_an_error_and_leaves_no_copy() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        let flac = dir.0.join(FILE_NAME);
        fs::write(&wav, b"ID3 not a wav file at all").unwrap();
        assert!(write_upload_copy(&wav, &flac, max_upload_bytes()).is_err());
        assert!(!flac.exists());
    }

    #[test]
    fn upload_limit_comes_from_the_shared_audio_rules() {
        assert!(max_upload_bytes() > 1_000_000);
    }
}
