//! Reads the format of a PCM WAV file and builds small WAV files from PCM.
//!
//! The FLAC encoder (src/flac_upload.rs) and the transcriber
//! (src/transcriber.rs) both read saved recordings through this one parser.
//! This file needs no Tauri, so its tests run in `core-tests`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

const WAVE_FORMAT_PCM: u16 = 1;
/// Larger files from other programs: the encoding is named by the SubFormat
/// GUID that follows the usual format fields.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// KSDATAFORMAT_SUBTYPE_PCM, as stored in the file. Other subformats (IEEE
/// float, for one) are not integer PCM.
const PCM_SUBFORMAT: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavFormat {
    pub channels: u16,
    /// Bytes per sample: 2 for the app's own 16-bit recordings.
    pub sample_width: u16,
    pub sample_rate: u32,
    /// Where the audio data starts in the file.
    pub data_offset: u64,
    /// The audio data's length in whole frames' bytes. A recording the app
    /// did not finish can state less or more than the file holds; this is
    /// what the file really holds.
    pub data_bytes: u64,
}

impl WavFormat {
    /// The bytes of one sample of every channel.
    pub fn frame_bytes(&self) -> u64 {
        u64::from(self.channels) * u64::from(self.sample_width)
    }

    pub fn seconds(&self) -> f64 {
        self.data_bytes as f64 / (self.frame_bytes() as f64 * f64::from(self.sample_rate))
    }
}

/// Finds the format and audio of a PCM WAV file. Leaves `file` at an
/// unspecified position.
pub fn read_wav_format(file: &mut File) -> io::Result<WavFormat> {
    let file_bytes = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
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
                let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                let sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
                let pcm = match tag {
                    WAVE_FORMAT_PCM => true,
                    WAVE_FORMAT_EXTENSIBLE if size >= 40 => {
                        // cbSize, valid bits and channel mask, then SubFormat.
                        let mut extension = [0_u8; 24];
                        file.read_exact(&mut extension)?;
                        extension[8..24] == PCM_SUBFORMAT
                    }
                    _ => false,
                };
                if !pcm || channels == 0 || sample_rate == 0 || bits == 0 || bits % 8 != 0 {
                    return Err(invalid("Not a PCM WAV file"));
                }
                format = Some((channels, bits / 8, sample_rate));
            }
            b"data" => {
                let (channels, sample_width, sample_rate) =
                    format.ok_or_else(|| invalid("WAV audio comes before its format"))?;
                let frame_bytes = u64::from(channels) * u64::from(sample_width);
                let data_bytes = size.min(file_bytes.saturating_sub(start));
                return Ok(WavFormat {
                    channels,
                    sample_width,
                    sample_rate,
                    data_offset: start,
                    data_bytes: data_bytes - data_bytes % frame_bytes,
                });
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(start + size + size % 2))?;
    }
}

/// A whole WAV file holding `pcm` in the given format.
pub fn wav_bytes(pcm: &[u8], channels: u16, sample_width: u16, sample_rate: u32) -> Vec<u8> {
    let data_bytes = pcm.len() as u32;
    let frame_bytes = channels * sample_width;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * u32::from(frame_bytes)).to_le_bytes());
    wav.extend_from_slice(&frame_bytes.to_le_bytes());
    wav.extend_from_slice(&(sample_width * 8).to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

/// Reads until `buffer` is full or the source ends; returns the bytes read.
pub fn read_up_to(source: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
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

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_wav(bytes: &[u8]) -> (std::path::PathBuf, File) {
        let path =
            std::env::temp_dir().join(format!("openwhisper-wav-{}.wav", uuid::Uuid::new_v4()));
        File::create(&path).unwrap().write_all(bytes).unwrap();
        let file = File::open(&path).unwrap();
        (path, file)
    }

    #[test]
    fn a_built_wav_reads_back_with_its_format() {
        let wav = wav_bytes(&[1, 0, 2, 0, 3, 0, 4, 0], 2, 2, 16_000);
        let (path, mut file) = temp_wav(&wav);
        let format = read_wav_format(&mut file).unwrap();
        let _ = std::fs::remove_file(path);

        assert_eq!(
            format,
            WavFormat {
                channels: 2,
                sample_width: 2,
                sample_rate: 16_000,
                data_offset: 44,
                data_bytes: 8,
            }
        );
        assert_eq!(format.frame_bytes(), 4);
    }

    #[test]
    fn a_header_that_promises_more_than_the_file_holds_is_cut_to_whole_frames() {
        let mut wav = wav_bytes(&[0; 10], 1, 2, 24_000);
        // An unfinished recording: the header says far more, a byte is cut off.
        wav[40..44].copy_from_slice(&1_000_000_u32.to_le_bytes());
        wav.pop();
        let (path, mut file) = temp_wav(&wav);
        let format = read_wav_format(&mut file).unwrap();
        let _ = std::fs::remove_file(path);

        assert_eq!(format.data_bytes, 8);
    }

    /// A WAVE_FORMAT_EXTENSIBLE file of 16-bit mono audio of `subformat`.
    fn extensible_wav(subformat: [u8; 16], audio: &[u8]) -> Vec<u8> {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(60 + audio.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&40_u32.to_le_bytes());
        wav.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&32_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(&22_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(&4_u32.to_le_bytes());
        wav.extend_from_slice(&subformat);
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(audio.len() as u32).to_le_bytes());
        wav.extend_from_slice(audio);
        wav
    }

    fn read_bytes(wav: &[u8]) -> io::Result<WavFormat> {
        let (path, mut file) = temp_wav(wav);
        let result = read_wav_format(&mut file);
        let _ = std::fs::remove_file(path);
        result
    }

    #[test]
    fn an_extensible_wav_is_read_only_when_it_holds_integer_pcm() {
        let pcm = read_bytes(&extensible_wav(PCM_SUBFORMAT, &[1, 0, 2, 0])).unwrap();
        assert_eq!(
            (pcm.sample_width, pcm.data_offset, pcm.data_bytes),
            (2, 68, 4)
        );

        let mut float = PCM_SUBFORMAT;
        float[0] = 0x03; // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        assert!(read_bytes(&extensible_wav(float, &[0; 8])).is_err());
    }

    #[test]
    fn other_files_are_refused() {
        let (path, mut file) = temp_wav(b"fLaC not a wav file");
        let result = read_wav_format(&mut file);
        let _ = std::fs::remove_file(path);

        assert!(result.is_err());
    }
}
