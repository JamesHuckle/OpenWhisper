//! The dictation rules the Android app also follows: the shortest recording
//! worth sending, the silence level, the prompt-echo check and the largest
//! upload. They live in packages/protocol/audio-rules.json, built into the
//! app, and their examples in packages/protocol/audio-examples/cases.json.
//!
//! This file needs no Tauri, so its tests run in `core-tests`.

use crate::wav_file::{read_up_to, read_wav_format};
use serde::Deserialize;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;

const SHARED_AUDIO_RULES_JSON: &str =
    include_str!("../../../../packages/protocol/audio-rules.json");

/// How much PCM the silence check reads at a time.
const PCM_SCAN_BYTES: usize = 1024 * 1024;
/// How much of the text before it a long recording's next part is prompted with.
pub const PREVIOUS_PART_CHARS: usize = 1200;

#[derive(Debug, Deserialize)]
pub struct AudioRules {
    /// A quick tap of the shortcut captures a fraction of a second of PCM.
    /// Uploading it either fails (the API rejects clips under 0.1 s) or makes
    /// the model echo the dictionary prompt back, so anything this short is
    /// no audio.
    pub min_speech_seconds: f64,
    /// A muted or disconnected microphone produces near-digital silence. Real
    /// speech peaks far above this, even on quiet microphones.
    pub silent_pcm_peak: i32,
    /// A transcript that repeats at least this share of the prompt, and
    /// nothing else, is the model answering silence.
    pub prompt_echo_min_share: f64,
    /// The largest file sent in one transcription request.
    pub max_upload_bytes: u64,
    pub no_audio_message: String,
}

pub fn rules() -> &'static AudioRules {
    static RULES: OnceLock<AudioRules> = OnceLock::new();
    RULES.get_or_init(|| {
        serde_json::from_str(SHARED_AUDIO_RULES_JSON)
            .expect("packages/protocol/audio-rules.json is not a valid rule list")
    })
}

/// The largest file sent in one transcription request, shared with the
/// Android app.
pub fn max_upload_bytes() -> u64 {
    rules().max_upload_bytes
}

/// The loudest 16-bit little-endian sample in `block` (0 when it has none).
pub fn pcm_peak(block: &[u8]) -> i32 {
    block
        .chunks_exact(2)
        .map(|sample| i32::from(i16::from_le_bytes([sample[0], sample[1]])).abs())
        .max()
        .unwrap_or(0)
}

/// True when the 16-bit samples from `source` never reach the silence level.
///
/// The same check decides whether a whole recording is a muted microphone
/// and whether one part of a long recording is worth uploading.
pub fn pcm_is_silent(source: &mut impl Read) -> io::Result<bool> {
    let mut block = vec![0_u8; PCM_SCAN_BYTES];
    loop {
        let filled = read_up_to(source, &mut block)?;
        if filled == 0 {
            return Ok(true);
        }
        if pcm_peak(&block[..filled]) >= rules().silent_pcm_peak {
            return Ok(false);
        }
        if filled < block.len() {
            return Ok(true);
        }
    }
}

/// Lower case letters and digits of any script only, one space between words.
fn normalize_words(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Transcription models answer silent or near-silent audio by repeating the
/// prompt they were sent. Such a transcript is no speech, not dictation.
pub fn echoes_prompt(text: &str, prompt: &str) -> bool {
    let spoken = normalize_words(text);
    let hints = normalize_words(prompt);
    if spoken.is_empty() || hints.is_empty() {
        return false;
    }
    hints.contains(&spoken)
        && spoken.chars().count() as f64
            >= hints.chars().count() as f64 * rules().prompt_echo_min_share
}

/// The text a long recording's next part is prompted with: the end of the
/// last part that had text.
pub fn previous_part_text(transcripts: &[String]) -> String {
    let previous = transcripts
        .iter()
        .rev()
        .find(|text| !text.is_empty())
        .map(String::as_str)
        .unwrap_or("");
    let chars = previous.chars().count();
    previous
        .chars()
        .skip(chars.saturating_sub(PREVIOUS_PART_CHARS))
        .collect()
}

/// A part's text, or nothing when it only repeats the prompt it was sent
/// with or the earlier text that prompt quotes. A part that is mostly
/// silence makes the model say its prompt back, which would paste a sentence
/// twice. The rules and their examples are shared with the Android app
/// (packages/protocol/audio-examples/cases.json, long_recording_parts).
pub fn kept_part_text(text: &str, prompt: &str, previous: &str) -> String {
    if echoes_prompt(text, prompt) || echoes_prompt(text, previous) {
        String::new()
    } else {
        text.to_string()
    }
}

/// The sample rate a raw PCM type names (`audio/pcm;rate=24000`), if any.
pub fn pcm_rate(mime_type: &str) -> Option<u32> {
    if !mime_type.starts_with("audio/pcm") {
        return None;
    }
    mime_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        let name = name.trim().to_ascii_lowercase();
        (name == "rate" || name == "samplerate")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

fn too_short(seconds: f64) -> Option<String> {
    (seconds < rules().min_speech_seconds).then(|| format!("too_short={seconds:.2}s"))
}

/// Why raw 16-bit mono PCM of `size` bytes at `rate` Hz, whose loudest sample
/// is `peak`, holds no speech, or None. Live sessions count their appended
/// chunks this way instead of keeping them.
pub fn no_speech_reason_from_counters(size: u64, peak: i32, rate: u32) -> Option<String> {
    let seconds = size as f64 / (f64::from(rate) * 2.0);
    too_short(seconds).or_else(|| (peak < rules().silent_pcm_peak).then(|| "silent".to_string()))
}

/// Why a saved 16-bit PCM WAV holds no speech, or None.
///
/// WAV is how the desktop saves a microphone recording. Other audio, and a
/// file that is not a plain PCM WAV, is left to the model.
pub fn no_speech_reason(path: &Path, mime_type: &str) -> Option<String> {
    if !matches!(mime_type, "audio/wav" | "audio/x-wav") {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let format = read_wav_format(&mut file).ok()?;
    if format.sample_width != 2 {
        return None;
    }
    if let Some(reason) = too_short(format.seconds()) {
        return Some(reason);
    }
    file.seek(SeekFrom::Start(format.data_offset)).ok()?;
    let mut audio = BufReader::new(file).take(format.data_bytes);
    pcm_is_silent(&mut audio)
        .ok()?
        .then(|| "silent".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;

    /// The shared examples, found from the app or the core-tests crate.
    fn examples_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .map(|dir| dir.join("packages/protocol/audio-examples"))
            .find(|dir| dir.is_dir())
            .expect("packages/protocol/audio-examples")
    }

    fn shared_cases() -> Value {
        serde_json::from_str(include_str!(
            "../../../../packages/protocol/audio-examples/cases.json"
        ))
        .unwrap()
    }

    #[test]
    fn the_app_uses_exactly_the_shared_rules() {
        let shared: Value = serde_json::from_str(SHARED_AUDIO_RULES_JSON).unwrap();
        let rules = rules();
        assert_eq!(shared["min_speech_seconds"], rules.min_speech_seconds);
        assert_eq!(shared["silent_pcm_peak"], rules.silent_pcm_peak);
        assert_eq!(shared["prompt_echo_min_share"], rules.prompt_echo_min_share);
        assert_eq!(shared["max_upload_bytes"], rules.max_upload_bytes);
        assert_eq!(shared["no_audio_message"], rules.no_audio_message.as_str());
        assert!(max_upload_bytes() > 1_000_000);
    }

    fn expected_answer(reason: Option<String>) -> &'static str {
        match reason.as_deref() {
            None => "speech",
            Some("silent") => "silent",
            Some(reason) if reason.starts_with("too_short=") => "too_short",
            Some(_) => "unknown",
        }
    }

    #[test]
    fn every_example_recording_gets_its_expected_answer() {
        let cases = shared_cases();
        let cases = cases["audio"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let path = examples_dir().join(case["file"].as_str().unwrap());
            assert_eq!(
                expected_answer(no_speech_reason(&path, "audio/wav")),
                case["expected"],
                "{}",
                case["about"]
            );
        }
    }

    #[test]
    fn example_recordings_sent_as_raw_pcm_get_the_same_answer() {
        let cases = shared_cases();
        for case in cases["audio"].as_array().unwrap() {
            let path = examples_dir().join(case["file"].as_str().unwrap());
            let mut file = File::open(&path).unwrap();
            let format = read_wav_format(&mut file).unwrap();
            file.seek(SeekFrom::Start(format.data_offset)).unwrap();
            let mut pcm = vec![0; format.data_bytes as usize];
            file.read_exact(&mut pcm).unwrap();
            // A Live session counts its chunks instead of keeping them.
            let peak = pcm.chunks(4_800).map(pcm_peak).max().unwrap_or(0);
            let reason = no_speech_reason_from_counters(
                pcm.len() as u64,
                peak,
                pcm_rate(&format!("audio/pcm;rate={}", format.sample_rate)).unwrap(),
            );
            assert_eq!(
                expected_answer(reason),
                case["expected"],
                "{}",
                case["about"]
            );
        }
    }

    #[test]
    fn every_example_transcript_gets_its_expected_answer() {
        let cases = shared_cases();
        for case in cases["prompt_echo"].as_array().unwrap() {
            assert_eq!(
                echoes_prompt(
                    case["text"].as_str().unwrap(),
                    case["prompt"].as_str().unwrap()
                ),
                case["expected"].as_bool().unwrap(),
                "{}",
                case["about"]
            );
        }
    }

    #[test]
    fn other_audio_is_left_to_the_model() {
        let path = examples_dir().join("muted.wav");
        assert_eq!(no_speech_reason(&path, "audio/mpeg"), None);
        assert_eq!(
            no_speech_reason(Path::new("missing.wav"), "audio/wav"),
            None
        );
    }

    #[test]
    fn raw_pcm_types_name_their_rate() {
        assert_eq!(pcm_rate("audio/pcm;rate=24000"), Some(24_000));
        assert_eq!(pcm_rate("audio/pcm; SampleRate=16000"), Some(16_000));
        assert_eq!(pcm_rate("audio/pcm"), None);
        assert_eq!(pcm_rate("audio/wav;rate=24000"), None);
    }

    #[test]
    fn the_loudest_sample_counts_either_sign() {
        assert_eq!(pcm_peak(&[]), 0);
        assert_eq!(pcm_peak(&(-32768_i16).to_le_bytes()), 32768);
        assert_eq!(pcm_peak(&[0x10, 0x00, 0x05]), 16);
    }

    #[test]
    fn the_next_part_quotes_the_end_of_the_last_part_with_text() {
        let long = "é".repeat(PREVIOUS_PART_CHARS + 5);
        assert_eq!(
            previous_part_text(&["First.".into(), long.clone(), String::new()]),
            "é".repeat(PREVIOUS_PART_CHARS)
        );
        assert_eq!(previous_part_text(&[]), "");
    }
}
