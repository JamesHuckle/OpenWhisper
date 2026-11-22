//! Makes the copy of a microphone recording that is uploaded for
//! transcription. FLAC is lossless: the service hears exactly the saved WAV,
//! in roughly half the bytes for speech, so the wait between stopping and the
//! text appearing is shorter, most of all on slow networks.
//!
//! The WAV stays the saved recording. The FLAC parts sit beside it only while
//! its transcription request runs. A recording too long for one upload is
//! encoded once, into several parts, each a whole FLAC file below the upload
//! limit, so a long dictation is never sent as raw WAV. It is encoded one
//! frame at a time straight to disk, so a long recording is never held in
//! memory.

use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, Frame, Stream, StreamInfo};
use flacenc::config;
use flacenc::error::{Verified, Verify};
use flacenc::source::{Context, Fill, FrameBuf};
use std::fs::{self, File};
use crate::wav_file::{read_up_to, read_wav_format, WavFormat};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MIME_TYPE: &str = "audio/flac";

/// Samples per channel in each FLAC frame (the reference encoder's default).
const BLOCK_SIZE: usize = 4096;
const BITS_PER_SAMPLE: usize = 16;
/// "fLaC" and the STREAMINFO block. It is written last, once the length and
/// checksum of the part's audio are known.
const HEADER_BYTES: usize = 42;

/// The name of upload part `number` (from 1), in the recording's folder.
pub fn part_file_name(number: usize) -> String {
    format!("audio.part-{number}.flac")
}

/// One FLAC upload part: the recording's audio from byte `pcm_start` to
/// `pcm_end` of the WAV's audio data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadPart {
    pub path: PathBuf,
    pub pcm_start: u64,
    pub pcm_end: u64,
}

/// Writes the 16-bit PCM WAV at `wav_path` as FLAC parts in `dir`, each a
/// whole FLAC file of at most `max_bytes`, in the order they are heard.
///
/// Returns no parts when the WAV holds no audio. On an error no part file is
/// left: the transcriber then sends the WAV itself, in parts when it is long.
pub fn write_upload_parts(
    wav_path: &Path,
    dir: &Path,
    max_bytes: u64,
) -> io::Result<Vec<UploadPart>> {
    let mut parts = Vec::new();
    let result = encode_parts(wav_path, dir, max_bytes, &mut parts);
    if result.is_err() {
        for part in &parts {
            let _ = fs::remove_file(&part.path);
        }
    }
    result.map(|()| parts)
}

/// The FLAC part being written: its file, and the length and checksum of
/// its audio so far, for its STREAMINFO header.
struct PartFile {
    out: BufWriter<File>,
    info: StreamInfo,
    context: Context,
    frames: usize,
    written: u64,
}

impl PartFile {
    fn create(path: &Path, format: &WavFormat) -> io::Result<Self> {
        let channels = usize::from(format.channels);
        let info = StreamInfo::new(format.sample_rate as usize, channels, BITS_PER_SAMPLE)
            .map_err(encode_error)?;
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&[0; HEADER_BYTES])?;
        Ok(Self {
            out,
            info,
            context: Context::new(BITS_PER_SAMPLE, channels),
            frames: 0,
            written: HEADER_BYTES as u64,
        })
    }

    /// Appends `frame`, encoded from the PCM `samples`.
    fn add(&mut self, frame: &Frame, encoded: &[u8], samples: &[u8]) -> io::Result<()> {
        self.context
            .fill_le_bytes(samples, 2)
            .map_err(encode_error)?;
        self.info.update_frame_info(frame);
        self.out.write_all(encoded)?;
        self.frames += 1;
        self.written += encoded.len() as u64;
        Ok(())
    }

    /// Writes the STREAMINFO header now that the part's audio is known.
    fn finish(mut self) -> io::Result<()> {
        // Every frame but the recording's last has the full block size.
        self.info
            .set_block_sizes(BLOCK_SIZE, BLOCK_SIZE)
            .map_err(encode_error)?;
        self.info.set_md5_digest(&self.context.md5_digest());
        let mut header = ByteSink::new();
        Stream::with_stream_info(self.info)
            .write(&mut header)
            .map_err(encode_error)?;
        if header.as_slice().len() != HEADER_BYTES {
            return Err(io::Error::other("Unexpected FLAC header size"));
        }
        let mut file = self.out.into_inner().map_err(|error| error.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(header.as_slice())?;
        Ok(())
    }
}

fn encode_frame(
    config: &Verified<config::Encoder>,
    framebuf: &FrameBuf,
    frame_number: usize,
    info: &StreamInfo,
) -> io::Result<(Frame, ByteSink)> {
    let frame = flacenc::encode_fixed_size_frame(config, framebuf, frame_number, info)
        .map_err(encode_error)?;
    let mut sink = ByteSink::new();
    frame.write(&mut sink).map_err(encode_error)?;
    Ok((frame, sink))
}

fn encode_parts(
    wav_path: &Path,
    dir: &Path,
    max_bytes: u64,
    parts: &mut Vec<UploadPart>,
) -> io::Result<()> {
    let mut wav = File::open(wav_path)?;
    let format = read_pcm16_format(&mut wav)?;
    wav.seek(SeekFrom::Start(format.data_offset))?;
    let mut pcm = BufReader::new(wav).take(format.data_bytes);

    let config = config::Encoder::default()
        .into_verified()
        .map_err(|(_, error)| encode_error(error))?;
    let mut framebuf =
        FrameBuf::with_size(usize::from(format.channels), BLOCK_SIZE).map_err(encode_error)?;
    let mut current: Option<PartFile> = None;
    let mut pcm_offset = 0_u64;
    let sample_frame_bytes = format.frame_bytes() as usize;
    let mut block = vec![0_u8; BLOCK_SIZE * sample_frame_bytes];
    loop {
        let filled = read_up_to(&mut pcm, &mut block)?;
        // A recording cut off mid-sample ends at its last whole sample.
        let whole = filled - filled % sample_frame_bytes;
        if whole == 0 {
            break;
        }
        let samples = &block[..whole];
        framebuf.fill_le_bytes(samples, 2).map_err(encode_error)?;
        let fits = |part: &PartFile, sink: &ByteSink| {
            part.written + sink.as_slice().len() as u64 <= max_bytes
        };
        let mut encoded = None;
        if let Some(part) = &current {
            let next = encode_frame(&config, &framebuf, part.frames, &part.info)?;
            if fits(part, &next.1) {
                encoded = Some(next);
            }
        }
        let (frame, sink) = match encoded {
            Some(next) => next,
            None => {
                // This frame starts the next part: a FLAC file of its own,
                // whose frames are numbered from 0.
                if let Some(part) = current.take() {
                    part.finish()?;
                }
                let path = dir.join(part_file_name(parts.len() + 1));
                parts.push(UploadPart {
                    path: path.clone(),
                    pcm_start: pcm_offset,
                    pcm_end: pcm_offset,
                });
                let part = current.insert(PartFile::create(&path, &format)?);
                let first = encode_frame(&config, &framebuf, 0, &part.info)?;
                if !fits(part, &first.1) {
                    return Err(io::Error::other(
                        "The upload limit is smaller than one FLAC frame",
                    ));
                }
                first
            }
        };
        let part = current.as_mut().expect("a part is open");
        part.add(&frame, sink.as_slice(), samples)?;
        pcm_offset += whole as u64;
        parts.last_mut().expect("a part is open").pcm_end = pcm_offset;
        if filled < block.len() {
            break;
        }
    }
    if let Some(part) = current {
        part.finish()?;
    }
    Ok(())
}

/// Finds the format and audio of a 16-bit PCM WAV file. Leaves `file` at an
/// unspecified position.
fn read_pcm16_format(file: &mut File) -> io::Result<WavFormat> {
    let format = read_wav_format(file)?;
    if format.sample_width != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Not a 16-bit PCM WAV file",
        ));
    }
    Ok(format)
}

fn encode_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("Could not encode FLAC: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_rules::max_upload_bytes;
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
        let mut bytes = wav_header(pcm.len() as u32, 1, 2, 24_000).to_vec();
        bytes.extend_from_slice(&pcm);
        fs::write(path, bytes).unwrap();
    }

    fn decode(path: &Path) -> (u32, Vec<i16>) {
        let mut reader = claxon::FlacReader::open(path).unwrap();
        let rate = reader.streaminfo().sample_rate;
        let decoded = reader.samples().map(|s| s.unwrap() as i16).collect();
        (rate, decoded)
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_dictation_is_one_flac_part_holding_exactly_the_saved_audio_in_fewer_bytes() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        // Not a whole number of FLAC frames, so the short last frame is covered.
        let original = samples(24_000 * 3 + 1_234);
        write_wav(&wav, &original);

        let parts = write_upload_parts(&wav, &dir.0, max_upload_bytes()).unwrap();

        let flac = dir.0.join("audio.part-1.flac");
        assert_eq!(
            parts,
            vec![UploadPart {
                path: flac.clone(),
                pcm_start: 0,
                pcm_end: original.len() as u64 * 2
            }]
        );
        let (rate, decoded) = decode(&flac);
        assert_eq!(rate, 24_000);
        assert_eq!(decoded, original);
        let reader = claxon::FlacReader::open(&flac).unwrap();
        assert_eq!(reader.streaminfo().samples, Some(original.len() as u64));
        assert!(fs::metadata(&flac).unwrap().len() < fs::metadata(&wav).unwrap().len() * 3 / 4);
    }

    #[test]
    fn a_long_recording_is_encoded_once_into_parts_below_the_limit() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        let original = samples(24_000 * 3 + 1_234);
        write_wav(&wav, &original);
        let limit = 40_000;

        let parts = write_upload_parts(&wav, &dir.0, limit).unwrap();

        assert!(parts.len() > 2, "{} parts", parts.len());
        let mut joined = Vec::new();
        let mut next_start = 0;
        for (index, part) in parts.iter().enumerate() {
            assert_eq!(
                part.path,
                dir.0.join(format!("audio.part-{}.flac", index + 1))
            );
            assert_eq!(part.pcm_start, next_start);
            assert!(fs::metadata(&part.path).unwrap().len() <= limit);
            // Each part is a whole FLAC file holding exactly its own audio.
            let (rate, decoded) = decode(&part.path);
            assert_eq!(rate, 24_000);
            let range = part.pcm_start as usize / 2..part.pcm_end as usize / 2;
            assert_eq!(decoded, original[range]);
            let reader = claxon::FlacReader::open(&part.path).unwrap();
            assert_eq!(reader.streaminfo().samples, Some(decoded.len() as u64));
            joined.extend(decoded);
            next_start = part.pcm_end;
        }
        assert_eq!(next_start, original.len() as u64 * 2);
        assert_eq!(joined, original);
    }

    #[test]
    fn unfinished_recording_is_read_to_its_last_whole_sample() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        let original = samples(5_000);
        write_wav(&wav, &original);
        // The header states less audio than the file holds, and half a sample
        // follows: as after a crash, before the header is repaired.
        let mut bytes = fs::read(&wav).unwrap();
        bytes[40..44].copy_from_slice(&(u32::MAX - 100).to_le_bytes());
        bytes.push(7);
        fs::write(&wav, bytes).unwrap();

        let parts = write_upload_parts(&wav, &dir.0, max_upload_bytes()).unwrap();

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].pcm_end, original.len() as u64 * 2);
        assert_eq!(decode(&parts[0].path).1, original);
    }

    #[test]
    fn a_recording_without_audio_has_no_parts() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        write_wav(&wav, &[]);

        assert_eq!(
            write_upload_parts(&wav, &dir.0, max_upload_bytes()).unwrap(),
            vec![]
        );
        assert_eq!(files_in(&dir.0), vec!["audio.wav"]);
    }

    #[test]
    fn errors_leave_no_part_files() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        // A limit too small for one frame fails after a part file was created.
        write_wav(&wav, &samples(24_000));
        assert!(write_upload_parts(&wav, &dir.0, 1_000).is_err());
        assert_eq!(files_in(&dir.0), vec!["audio.wav"]);

        fs::write(&wav, b"ID3 not a wav file at all").unwrap();
        assert!(write_upload_parts(&wav, &dir.0, max_upload_bytes()).is_err());
        assert_eq!(files_in(&dir.0), vec!["audio.wav"]);
    }

    /// How long the upload copy of a long dictation takes to make. Run it in
    /// release mode, as the app is built:
    /// cargo test --release --manifest-path apps/desktop/src-tauri/core/Cargo.toml -- --ignored --nocapture five_minutes
    #[test]
    #[ignore]
    fn five_minutes_of_speech_encode_time() {
        let dir = TempDir::new();
        let wav = dir.0.join("audio.wav");
        let minutes = 5;
        write_wav(&wav, &samples(24_000 * 60 * minutes));

        let started = std::time::Instant::now();
        let parts = write_upload_parts(&wav, &dir.0, max_upload_bytes()).unwrap();
        let elapsed = started.elapsed();

        let flac_bytes: u64 = parts
            .iter()
            .map(|part| fs::metadata(&part.path).unwrap().len())
            .sum();
        println!(
            "encoded {minutes} min in {} ms ({} ms per minute): {} parts, wav {} bytes, flac {} bytes",
            elapsed.as_millis(),
            elapsed.as_millis() / minutes as u128,
            parts.len(),
            fs::metadata(&wav).unwrap().len(),
            flac_bytes
        );
    }

    #[test]
    fn upload_limit_comes_from_the_shared_audio_rules() {
        assert!(max_upload_bytes() > 1_000_000);
    }
}
