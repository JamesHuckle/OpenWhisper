//! Turns a saved recording into text and tidies it, over the OpenAI API.
//!
//! A dictation's single FLAC upload part goes in one request. A long
//! recording goes in parts, in order, each prompted with the end of the text
//! before it; each finished part is saved (src/saved_parts.rs), so a retry
//! sends only the rest. The tidy pass splits a long transcript at sentences
//! and tidies up to five pieces at once.
//!
//! The part rules are shared with the Android app
//! (packages/protocol/audio-examples/cases.json). This file needs no Tauri,
//! so its tests run in the `core` crate.

use crate::audio_rules::{kept_part_text, max_upload_bytes, pcm_is_silent, previous_part_text};
use crate::debug_log::debug_log_line;
use crate::flac_upload::{self, UploadPart};
use crate::openai_api::{ApiError, OpenAiApi};
use crate::recording_store::WAV_HEADER_BYTES;
use crate::saved_parts::{parts_key, SavedPart, SavedParts};
use crate::sync::lock;
use crate::wav_file::{read_wav_format, read_wav_part, WavFormat};
use futures_util::future::try_join_all;
use regex::Regex;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tempfile::TempDir;

const MIN_SENTENCES_PER_POLISH_CHUNK: usize = 4;
const MAX_PARALLEL_POLISH_CHUNKS: usize = 5;
const MAX_POLISH_CHUNK_CHARS: usize = 12_000;
/// Transcripts this short that already read cleanly are pasted without a
/// tidy call.
const MAX_SKIP_POLISH_WORDS: usize = 11;
/// Tidying a transcript needs no hidden thinking, which is billed and delays
/// the first word. A model that rejects "minimal" is asked again with "low".
pub const DEFAULT_POLISH_REASONING_EFFORT: &str = "minimal";
const FALLBACK_POLISH_REASONING_EFFORT: &str = "low";

const POLISH_SYSTEM: &str = "\
You are a master transcriber and transcript editor.
Return only the transcribed text, with only the modifications requested below.
Do not answer the speaker, continue the conversation, acknowledge instructions, add suggestions,
or add new content.

Rules (follow strictly):
- Strip filler words and verbal tics: um, uh, like (when used as filler), you know, sort of, \
kind of, basically, literally, right (at sentence ends), so (as a standalone sentence opener)
- Remove false starts and self-corrections (e.g. \"I want to\u{2014} I think we should\" \u{2192} \
\"I think we should\")
- Remove redundant repetition caused by mid-thought restarts
- Fix sentence boundaries, capitalisation, and punctuation
- Use a markdown bullet list ONLY if the speaker clearly enumerates three or more distinct items
- Do NOT paraphrase, add explanations, summarise, or change the speaker's meaning
- Never output acknowledgements such as \"Understood\", \"Got it\", or similar
- If the speaker includes meta-instructions about formatting or cleanup, apply them but do not
  include those instructions in the returned transcript unless they are clearly part of the
  dictated content
- Return ONLY the cleaned text \u{2014} no preamble, labels, or commentary";

#[derive(Debug)]
pub enum TranscribeError {
    Api(ApiError),
    Io(io::Error),
    Message(String),
}

impl TranscribeError {
    /// The error without its message text, for logs.
    pub fn describe(&self) -> String {
        match self {
            TranscribeError::Api(error) => error.describe(),
            TranscribeError::Io(error) => format!("IoError kind={:?}", error.kind()),
            TranscribeError::Message(_) => "TranscriptionError".into(),
        }
    }

    /// Whether the audio sent could not be used (HTTP 400) or its file could
    /// not be read: the saved WAV is sent instead.
    fn upload_refused(&self) -> bool {
        match self {
            TranscribeError::Api(error) => error.is_bad_request(),
            TranscribeError::Io(_) => true,
            TranscribeError::Message(_) => false,
        }
    }
}

impl fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranscribeError::Api(error) => error.fmt(f),
            TranscribeError::Io(error) => error.fmt(f),
            TranscribeError::Message(message) => f.write_str(message),
        }
    }
}

impl From<ApiError> for TranscribeError {
    fn from(error: ApiError) -> Self {
        TranscribeError::Api(error)
    }
}

impl From<io::Error> for TranscribeError {
    fn from(error: io::Error) -> Self {
        TranscribeError::Io(error)
    }
}

/// The tidy model and the reasoning effort it accepts. The effort is shared
/// by every dictation, so once a model refuses "minimal" later ones ask with
/// "low" at once.
#[derive(Clone)]
pub struct PolishSettings {
    pub model: String,
    pub effort: Arc<Mutex<String>>,
}

pub struct Transcriber<A> {
    api: A,
    model: String,
    polish: PolishSettings,
}

/// One part of a long recording: its byte range in the WAV's audio, and the
/// FLAC copy to upload in its place, if any.
#[derive(Clone)]
struct Part {
    start: u64,
    end: u64,
    upload: Option<PathBuf>,
}

impl<A: OpenAiApi> Transcriber<A> {
    pub fn new(api: A, model: &str, polish: PolishSettings) -> Self {
        Self {
            api,
            model: model.to_string(),
            polish,
        }
    }

    /// Transcribes a whole recording in one request.
    pub async fn transcribe_bytes(
        &self,
        audio: Vec<u8>,
        mime_type: &str,
        prompt: &str,
    ) -> Result<String, TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::Message("No audio bytes provided".into()));
        }
        Ok(self
            .api
            .transcribe(audio, mime_type, &self.model, prompt)
            .await?)
    }

    /// Transcribes a saved recording. A microphone recording goes as its
    /// FLAC upload parts (src/flac_upload.rs), made here and deleted when
    /// this returns: a dictation's single part as it is, a long recording's
    /// parts with the text before each as context. The saved file itself
    /// goes, in parts when it is long, for any other recording, when no
    /// parts could be made or the service refuses one.
    pub async fn transcribe_recording(
        &self,
        audio: &SavedAudio,
        prompt: &str,
        on_progress: &(dyn Fn(usize, usize) + Send + Sync),
    ) -> Result<String, TranscribeError> {
        // Holds the parts' folder until the transcription ends.
        let uploads = if audio.microphone {
            flac_upload_parts(&audio.audio_path).await
        } else {
            None
        };
        let upload_parts = uploads.as_ref().map_or(&[][..], |(_, parts)| parts);
        self.transcribe_with_uploads(audio, prompt, on_progress, upload_parts)
            .await
    }

    /// Transcribes a saved recording, sending `upload_parts` in place of
    /// its WAV when they are usable.
    async fn transcribe_with_uploads(
        &self,
        audio: &SavedAudio,
        prompt: &str,
        on_progress: &(dyn Fn(usize, usize) + Send + Sync),
        upload_parts: &[UploadPart],
    ) -> Result<String, TranscribeError> {
        let mut parts = usable_upload_parts(upload_parts);
        if parts.len() == 1 {
            let sent = match read_file(parts[0].path.clone()).await {
                Ok(upload) => {
                    self.transcribe_bytes(upload, flac_upload::MIME_TYPE, prompt)
                        .await
                }
                Err(error) => Err(error.into()),
            };
            match sent {
                Err(error) if error.upload_refused() => debug_log_line(
                    "transcriber",
                    &format!(
                        "upload part refused error={}; sending the saved file",
                        error.describe()
                    ),
                ),
                sent => return sent,
            }
            parts.clear();
        }
        self.transcribe_file(
            &audio.audio_path,
            &audio.mime_type,
            prompt,
            on_progress,
            audio.parts_path.clone(),
            &parts,
        )
        .await
    }

    /// Transcribes a file on disk. A WAV too large for one request goes in
    /// parts; with `upload_parts`, a WAV goes as those smaller copies, one
    /// request each. With `parts_path`, each finished part is saved there
    /// and a later call with the same audio and settings sends only the rest.
    async fn transcribe_file(
        &self,
        path: &Path,
        mime_type: &str,
        prompt: &str,
        on_progress: &(dyn Fn(usize, usize) + Send + Sync),
        parts_path: Option<PathBuf>,
        upload_parts: &[UploadPart],
    ) -> Result<String, TranscribeError> {
        let size = fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if size == 0 {
            return Err(TranscribeError::Message("No audio bytes provided".into()));
        }
        let limit = max_upload_bytes();
        if is_wav(mime_type) && (size > limit || !upload_parts.is_empty()) {
            let saved = SavedParts::new(parts_path, parts_key(path, &self.model, prompt)?);
            return self
                .transcribe_wav_file(path, prompt, on_progress, saved, upload_parts)
                .await;
        }
        if size > limit {
            return Err(TranscribeError::Message(format!(
                "This compressed audio file is larger than {} MB. \
                 Convert it to a smaller MP3/M4A or split it before importing.",
                limit / (1 << 20)
            )));
        }
        let audio = read_file(path.to_path_buf()).await?;
        self.transcribe_bytes(audio, mime_type, prompt).await
    }

    /// Sends a saved WAV in parts, in order, each prompted with the end of
    /// the text before it. The upload parts, when they cover the whole WAV,
    /// are sent in place of its bytes; once the service refuses one, the
    /// rest of the recording goes as WAV parts.
    async fn transcribe_wav_file(
        &self,
        path: &Path,
        prompt: &str,
        on_progress: &(dyn Fn(usize, usize) + Send + Sync),
        mut saved: SavedParts,
        upload_parts: &[UploadPart],
    ) -> Result<String, TranscribeError> {
        let format = read_wav_format(&mut File::open(path)?)?;
        let audio_bytes = format.data_bytes;
        let frame_bytes = format.frame_bytes();
        let wav_part_bytes = ((max_upload_bytes() - 128) / frame_bytes).max(1) * frame_bytes;
        let wav_parts = |start: u64| -> Vec<Part> {
            (start..audio_bytes)
                .step_by(wav_part_bytes as usize)
                .map(|offset| Part {
                    start: offset,
                    end: (offset + wav_part_bytes).min(audio_bytes),
                    upload: None,
                })
                .collect()
        };

        let mut parts: Vec<Part> = upload_parts
            .iter()
            .map(|part| Part {
                start: part.pcm_start,
                end: part.pcm_end,
                upload: Some(part.path.clone()),
            })
            .collect();
        if !parts.is_empty() && !parts_cover(&parts, audio_bytes, frame_bytes) {
            debug_log_line(
                "parts",
                "upload parts do not match the recording; sending it as WAV",
            );
            parts.clear();
        }
        if parts.is_empty() {
            parts = wav_parts(0);
        }

        let mut transcripts: Vec<String> = Vec::new();
        let mut index = 0;
        while index < parts.len() {
            let Part {
                start,
                mut end,
                upload,
            } = parts[index].clone();
            // A part finished on an earlier try is looked up before its audio
            // is read, so a retry reads only the parts still to send.
            let mut done = saved.get(index, start, end).map(|part| part.text.clone());
            let mut wav = Vec::new();
            if done.is_none() && upload.is_none() {
                wav = read_wav_file_part(path.to_path_buf(), format, start, end).await?;
                let audio_read = wav.len() as u64 - WAV_HEADER_BYTES;
                if audio_read == 0 {
                    break;
                }
                // A read that ended early (the file shrank after its format
                // was read) is looked up again with the range it read.
                end = start + audio_read;
                done = saved.get(index, start, end).map(|part| part.text.clone());
            }
            if let Some(text) = done {
                // Its text is reused, and builds the next part's prompt as
                // before.
                transcripts.push(text);
                index += 1;
                continue;
            }
            let silent = format.sample_width == 2
                && if upload.is_none() {
                    pcm_is_silent(&mut &wav[WAV_HEADER_BYTES as usize..])?
                } else {
                    audio_is_silent(path.to_path_buf(), format, start, end).await?
                };
            if silent {
                // A silent part is never uploaded: it costs a request and the
                // model answers it by repeating its prompt. It is saved as
                // finished and empty, so a retry skips it too.
                saved.add(
                    index,
                    SavedPart {
                        start,
                        end,
                        text: String::new(),
                    },
                );
                transcripts.push(String::new());
                index += 1;
                continue;
            }
            on_progress(index + 1, parts.len());
            let previous = previous_part_text(&transcripts);
            let contextual_prompt = if previous.is_empty() {
                prompt.to_string()
            } else {
                format!("{prompt}\nPrevious segment transcript: {previous}")
                    .trim()
                    .to_string()
            };
            let text = match &upload {
                None => {
                    self.transcribe_bytes(wav, "audio/wav", &contextual_prompt)
                        .await?
                }
                Some(upload) => {
                    let sent = match read_file(upload.clone()).await {
                        Ok(audio) => {
                            self.transcribe_bytes(audio, flac_upload::MIME_TYPE, &contextual_prompt)
                                .await
                        }
                        Err(error) => Err(error.into()),
                    };
                    match sent {
                        Ok(text) => text,
                        Err(error) if error.upload_refused() => {
                            debug_log_line(
                                "parts",
                                &format!(
                                    "upload part refused part={} error={}; sending the rest as WAV",
                                    index + 1,
                                    error.describe()
                                ),
                            );
                            parts.splice(index.., wav_parts(start));
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
            };
            let text = kept_part_text(text.trim(), &contextual_prompt, &previous);
            saved.add(
                index,
                SavedPart {
                    start,
                    end,
                    text: text.clone(),
                },
            );
            transcripts.push(text);
            index += 1;
        }
        Ok(transcripts
            .iter()
            .filter(|text| !text.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string())
    }

    /// Runs a fast model pass to strip filler words, fix sentence structure
    /// and add basic markdown formatting. Fails on API errors; the caller
    /// keeps the raw text.
    pub async fn polish(&self, text: &str, custom_prompt: &str) -> Result<String, ApiError> {
        let stripped = text.trim();
        if stripped.is_empty() {
            return Ok(text.to_string());
        }
        let system = build_polish_system(custom_prompt);
        let chunks = chunk_for_polish(stripped);
        debug_log_line(
            "polish",
            &format!(
                "chunk_plan model={} chunks={} input_chars={} custom_prompt_chars={}",
                self.polish.model,
                chunks.len(),
                stripped.chars().count(),
                custom_prompt.trim().chars().count()
            ),
        );
        let total = chunks.len();
        if total == 1 {
            return self.polish_chunk(&system, &chunks[0], 1, 1).await;
        }
        let limit = tokio::sync::Semaphore::new(MAX_PARALLEL_POLISH_CHUNKS);
        let jobs = chunks.iter().enumerate().map(|(index, chunk)| {
            let (limit, system) = (&limit, &system);
            async move {
                let _turn = limit.acquire().await;
                self.polish_chunk(system, chunk, index + 1, total).await
            }
        });
        let results = try_join_all(jobs).await?;
        Ok(join_polish_chunks(&results))
    }

    async fn polish_chunk(
        &self,
        system: &str,
        text: &str,
        chunk: usize,
        total: usize,
    ) -> Result<String, ApiError> {
        // Log only sizes, timings and error codes: the prompt, the dictated
        // words and the tidied result are private and must never reach the
        // log.
        let started = Instant::now();
        let chars = text.chars().count();
        match self.request_polish(system, text).await {
            Ok((result, effort)) => {
                debug_log_line(
                    "polish",
                    &format!(
                        "chunk_done chunk={chunk}/{total} system_chars={} input_chars={chars} \
                         output_chars={} effort={effort} elapsed_ms={}",
                        system.chars().count(),
                        result.chars().count(),
                        started.elapsed().as_millis()
                    ),
                );
                Ok(result)
            }
            Err(error) => {
                debug_log_line(
                    "polish",
                    &format!(
                        "chunk_failed chunk={chunk}/{total} input_chars={chars} elapsed_ms={} error={}",
                        started.elapsed().as_millis(),
                        error.describe()
                    ),
                );
                Err(error)
            }
        }
    }

    /// The tidied text and the reasoning effort used.
    async fn request_polish(&self, system: &str, text: &str) -> Result<(String, String), ApiError> {
        let mut effort = lock(&self.polish.effort).clone();
        let answer = match self
            .api
            .respond(&self.polish.model, system, text, &effort)
            .await
        {
            Err(error)
                if error.is_bad_request()
                    && error.param() == Some("reasoning.effort")
                    && effort != FALLBACK_POLISH_REASONING_EFFORT =>
            {
                // Remembered, so later chunks and dictations ask with "low" at once.
                effort = FALLBACK_POLISH_REASONING_EFFORT.to_string();
                *lock(&self.polish.effort) = effort.clone();
                debug_log_line(
                    "polish",
                    &format!(
                        "effort_fallback model={} effort={effort} error={}",
                        self.polish.model,
                        error.describe()
                    ),
                );
                self.api
                    .respond(&self.polish.model, system, text, &effort)
                    .await?
            }
            answer => answer?,
        };
        let answer = answer.trim();
        let result = if answer.is_empty() { text } else { answer };
        Ok((result.to_string(), effort))
    }
}

/// A saved recording to transcribe. Nothing here is changed or removed.
pub struct SavedAudio {
    pub audio_path: PathBuf,
    pub mime_type: String,
    /// A 16-bit PCM WAV from the microphone, uploaded as smaller lossless
    /// FLAC parts.
    pub microphone: bool,
    /// Where the finished parts of a long recording are saved, so a retry
    /// after a failure sends only the rest.
    pub parts_path: Option<PathBuf>,
}

/// Writes the smaller, lossless FLAC parts of a microphone recording that are
/// uploaded in place of its WAV, one request each, in a new folder inside
/// the recording's. None when they could not be made, and no parts when the
/// WAV holds no audio: the WAV is then sent, as it is for the rest of a
/// recording once the service refuses a part.
///
/// The parts come with their folder, deleted with them when it is dropped:
/// when the transcription returns, fails or is cancelled, even while it
/// encodes, and a retry meanwhile writes to a folder of its own.
///
/// Logs how long the encoding took and the sizes before and after, so the
/// wait it adds between stopping and the text appearing can be checked.
async fn flac_upload_parts(wav_path: &Path) -> Option<(TempDir, Vec<UploadPart>)> {
    let wav_bytes = fs::metadata(wav_path).map(|m| m.len()).unwrap_or(0);
    let started = Instant::now();
    let path = wav_path.to_path_buf();
    let written = blocking(move || {
        let written = flac_upload::write_upload_parts(&path, max_upload_bytes());
        #[cfg(test)]
        tests::pause_encoder(&path);
        written
    })
    .await;
    let encode_ms = started.elapsed().as_millis();
    match written {
        Ok(uploads) => {
            let parts = &uploads.1;
            let flac_bytes: u64 = parts
                .iter()
                .filter_map(|part| fs::metadata(&part.path).ok())
                .map(|m| m.len())
                .sum();
            debug_log_line(
                "recording",
                &format!(
                    "FLAC upload parts: {} parts, encode_ms={encode_ms} wav_bytes={wav_bytes} flac_bytes={flac_bytes}",
                    parts.len()
                ),
            );
            Some(uploads)
        }
        Err(error) => {
            debug_log_line("recording", &format!("FLAC upload parts failed: {error}"));
            None
        }
    }
}

fn is_wav(mime_type: &str) -> bool {
    matches!(mime_type, "audio/wav" | "audio/x-wav")
}

/// The upload parts, in order, or none when any is missing, empty, too large
/// for one request or not where the one before it ended: the WAV is sent
/// instead.
fn usable_upload_parts(parts: &[UploadPart]) -> Vec<UploadPart> {
    let mut usable: Vec<UploadPart> = Vec::new();
    for part in parts {
        let size = fs::metadata(&part.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let expected_start = usable.last().map_or(0, |last| last.pcm_end);
        if part.pcm_start != expected_start
            || part.pcm_end <= part.pcm_start
            || size == 0
            || size > max_upload_bytes()
        {
            return Vec::new();
        }
        usable.push(part.clone());
    }
    usable
}

/// Whether `parts` cover the audio, in order, each from a whole sample.
fn parts_cover(parts: &[Part], audio_bytes: u64, frame_bytes: u64) -> bool {
    let mut expected = 0;
    for part in parts {
        if part.start != expected || part.end <= part.start || part.start % frame_bytes != 0 {
            return false;
        }
        expected = part.end;
    }
    expected == audio_bytes
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| io::Error::other(error.to_string()))?
}

async fn read_file(path: PathBuf) -> io::Result<Vec<u8>> {
    blocking(move || fs::read(path)).await
}

/// The WAV's audio from byte `start` to `end` (or its end), as the WAV
/// file uploaded for it.
async fn read_wav_file_part(
    path: PathBuf,
    format: WavFormat,
    start: u64,
    end: u64,
) -> io::Result<Vec<u8>> {
    #[cfg(test)]
    lock(&tests::AUDIO_READS).push((path.clone(), start, end));
    blocking(move || read_wav_part(File::open(path)?, format, start, end)).await
}

/// Whether the WAV's audio from byte `start` to `end` is silent, read a
/// block at a time so a long part is never held in memory whole.
async fn audio_is_silent(
    path: PathBuf,
    format: WavFormat,
    start: u64,
    end: u64,
) -> io::Result<bool> {
    blocking(move || {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(format.data_offset + start))?;
        pcm_is_silent(&mut BufReader::new(file).take(end - start))
    })
    .await
}

fn build_polish_system(custom_prompt: &str) -> String {
    let extra = custom_prompt.trim();
    if extra.is_empty() {
        return POLISH_SYSTEM.to_string();
    }
    format!(
        "{POLISH_SYSTEM}\n\nAdditional modifications requested by the user (apply these too, \
         while still returning only the cleaned transcript text):\n{extra}"
    )
}

/// The transcript in sentences: each ends at `.`, `!` or `?`, with any
/// closing quotes or brackets after it, followed by white space.
fn split_sentences(text: &str) -> Vec<String> {
    let text = text.trim();
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        while let Some(&(_, next)) = chars.peek() {
            if matches!(next, '"' | '\'' | '\u{201d}' | '\u{2019}' | ')' | ']') {
                chars.next();
            } else {
                break;
            }
        }
        let end = chars.peek().map_or(text.len(), |&(at, _)| at);
        let mut spaced = false;
        while let Some(&(_, next)) = chars.peek() {
            if next.is_whitespace() {
                spaced = true;
                chars.next();
            } else {
                break;
            }
        }
        if spaced {
            sentences.push(text[start..end].trim().to_string());
            start = chars.peek().map_or(text.len(), |&(at, _)| at);
        }
    }
    sentences.push(text[start..].trim().to_string());
    sentences.retain(|sentence| !sentence.is_empty());
    if sentences.is_empty() {
        vec![text.to_string()]
    } else {
        sentences
    }
}

fn chunk_for_polish(text: &str) -> Vec<String> {
    let sentences = split_sentences(text);
    if sentences.len() <= MIN_SENTENCES_PER_POLISH_CHUNK
        && text.chars().count() <= MAX_POLISH_CHUNK_CHARS
    {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_chars = 0;
    for sentence in sentences {
        let chars = sentence.chars().count();
        let added = chars + usize::from(!current.is_empty());
        if !current.is_empty() && current_chars + added > MAX_POLISH_CHUNK_CHARS {
            chunks.push(current.join(" "));
            current.clear();
            current_chars = 0;
        }
        current.push(sentence);
        current_chars += chars + usize::from(current.len() > 1);
    }
    if !current.is_empty() {
        chunks.push(current.join(" "));
    }
    if chunks.is_empty() {
        vec![text.to_string()]
    } else {
        chunks
    }
}

fn join_polish_chunks(chunks: &[String]) -> String {
    let cleaned: Vec<&str> = chunks
        .iter()
        .map(|chunk| chunk.trim())
        .filter(|chunk| !chunk.is_empty())
        .collect();
    let separator = if cleaned.iter().any(|chunk| chunk.contains('\n')) {
        "\n\n"
    } else {
        " "
    };
    cleaned.join(separator)
}

/// Signs that the default tidy pass would change a transcript: filler words
/// and verbal tics, a sentence opened with "so", a tag "right" at the end,
/// self-corrections marked by dashes, what may be a spoken formatting command
/// ("Put this in capitals: hello.") or an enumeration the list rule could
/// format (two or more commas). Stuttered repeats ("the the") are checked in
/// `has_repeated_word`.
fn polish_needed() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(concat!(
            r"(?i)\b(?:um+|uh+|uhm|erm|er|ah|hmm+|mm+|you know|i mean|sort of|kind of|basically|literally)\b",
            r"|(?:^|,\s*)like\b|\blike,",
            r"|^so\b",
            r"|,\s*right\W*$",
            r"|--|\x{2014}|\x{2013}|\s-\s|\.\.\.|\x{2026}",
            r"|[:;]|,[^,]*,",
            r"|\b(?:capitals?|capitali[sz]e\w*|upper ?case|lower ?case|all caps|bold|italics?",
            r"|bullets?|bullet points?|numbered|list|new line|new paragraph|full stop|comma",
            r"|question mark|exclamation (?:mark|point)|quotes?|quotation marks?|format\w*)\b",
        ))
        .expect("the tidy-needed pattern")
    })
}

/// A word said twice in a row ("the the"), in any case.
fn has_repeated_word(text: &str) -> bool {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| Regex::new(r"\w+").expect("the word pattern"));
    let words: Vec<_> = word.find_iter(text).collect();
    words.windows(2).any(|pair| {
        let between = &text[pair[0].end()..pair[1].start()];
        !between.is_empty()
            && between.chars().all(char::is_whitespace)
            && pair[0].as_str().to_lowercase() == pair[1].as_str().to_lowercase()
    })
}

/// Whether the default tidy pass would leave this transcript as it is.
///
/// True for one short, already punctuated sentence with no filler words or
/// false starts, so the caller can paste it at once instead of paying for a
/// model call and waiting for it.
pub fn is_short_clean_transcript(text: &str) -> bool {
    let stripped = text.trim();
    if stripped.is_empty() || stripped.split_whitespace().count() > MAX_SKIP_POLISH_WORDS {
        return false;
    }
    let first = stripped.chars().next().unwrap_or(' ');
    if !(first.is_uppercase() || first.is_numeric()) {
        return false;
    }
    let last = stripped.chars().last().unwrap_or(' ');
    let body = &stripped[..stripped.len() - last.len_utf8()];
    let mut body_chars = body.chars().peekable();
    let mut inner_sentence_end = false;
    while let Some(c) = body_chars.next() {
        if matches!(c, '.' | '?' | '!')
            && body_chars.peek().is_some_and(|next| next.is_whitespace())
        {
            inner_sentence_end = true;
        }
    }
    if !matches!(last, '.' | '?' | '!') || inner_sentence_end || stripped.contains('\n') {
        return false;
    }
    !polish_needed().is_match(stripped) && !has_repeated_word(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_rules::rules;
    use crate::wav_file::wav_bytes;
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::sync::mpsc;
    use std::time::Duration;

    const SPEECH: i16 = 8_000;

    /// Every WAV byte range `read_wav_file_part` read, with its file.
    pub(super) static AUDIO_READS: Mutex<Vec<(PathBuf, u64, u64)>> = Mutex::new(Vec::new());

    /// The WAVs whose encoder, once it has written its parts, tells the test
    /// and waits for it before handing them over.
    #[allow(clippy::type_complexity)]
    static ENCODER_PAUSES: Mutex<Vec<(PathBuf, mpsc::Sender<()>, mpsc::Receiver<()>)>> =
        Mutex::new(Vec::new());

    pub(super) fn pause_encoder(wav_path: &Path) {
        let pause = {
            let mut pauses = lock(&ENCODER_PAUSES);
            let index = pauses.iter().position(|(path, _, _)| path == wav_path);
            index.map(|index| pauses.remove(index))
        };
        if let Some((_, written, release)) = pause {
            written.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }

    fn part_bytes() -> u64 {
        (max_upload_bytes() - 128) / 2 * 2
    }

    /// A stand-in for OpenAI: answers transcriptions and tidy requests from
    /// queues, and keeps what it was sent.
    #[derive(Default)]
    struct FakeApi {
        transcripts: Mutex<VecDeque<Result<String, ApiError>>>,
        responses: Mutex<VecDeque<Result<String, ApiError>>>,
        /// Each upload's type, prompt and (for FLAC) bytes.
        sent: Mutex<Vec<(String, String, Option<Vec<u8>>)>>,
        /// Each WAV upload's bytes.
        wavs: Mutex<Vec<Vec<u8>>>,
        /// Each tidy request's text and effort.
        tidied: Mutex<Vec<(String, String)>>,
    }

    impl FakeApi {
        fn transcribing(replies: Vec<Result<String, ApiError>>) -> Self {
            Self {
                transcripts: Mutex::new(replies.into()),
                ..Self::default()
            }
        }
    }

    impl OpenAiApi for FakeApi {
        async fn transcribe(
            &self,
            audio: Vec<u8>,
            mime_type: &str,
            _model: &str,
            prompt: &str,
        ) -> Result<String, ApiError> {
            let flac = if mime_type == "audio/wav" {
                assert_eq!(&audio[..4], b"RIFF");
                assert!(audio.len() as u64 <= max_upload_bytes());
                lock(&self.wavs).push(audio);
                None
            } else {
                (mime_type == "audio/flac").then_some(audio)
            };
            lock(&self.sent).push((mime_type.into(), prompt.into(), flac));
            lock(&self.transcripts)
                .pop_front()
                .expect("a queued transcript")
        }

        async fn respond(
            &self,
            _model: &str,
            _system: &str,
            text: &str,
            effort: &str,
        ) -> Result<String, ApiError> {
            lock(&self.tidied).push((text.into(), effort.into()));
            lock(&self.responses)
                .pop_front()
                .expect("a queued response")
        }
    }

    fn transcriber(api: FakeApi) -> Transcriber<FakeApi> {
        Transcriber::new(
            api,
            "gpt-transcribe",
            PolishSettings {
                model: "gpt-5-nano".into(),
                effort: Arc::new(Mutex::new(DEFAULT_POLISH_REASONING_EFFORT.into())),
            },
        )
    }

    fn bad_request(param: Option<&str>) -> ApiError {
        ApiError::Status {
            status: 400,
            code: Some("invalid_input".into()),
            param: param.map(str::to_string),
            message: "Could not process input".into(),
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("openwhisper-transcriber-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A saved desktop recording just long enough to be sent in one part per
    /// entry of `peaks`. Each part is silence with one sample at its level
    /// (0 leaves it silent). The file is sparse, so it costs no disk space.
    fn long_wav(dir: &Path, peaks: &[i16]) -> PathBuf {
        let parts = peaks.len() as u64;
        let pcm_bytes = (parts - 1) * part_bytes() + 24_000 * 2;
        let path = dir.join("audio.wav");
        let mut file = File::create(&path).unwrap();
        file.write_all(&crate::recording_store::wav_header(
            pcm_bytes as u32,
            1,
            2,
            24_000,
        ))
        .unwrap();
        file.set_len(44 + pcm_bytes).unwrap();
        for (index, &peak) in peaks.iter().enumerate() {
            if peak != 0 {
                file.seek(SeekFrom::Start(44 + index as u64 * part_bytes() + 1_000))
                    .unwrap();
                file.write_all(&(-peak).to_le_bytes()).unwrap();
            }
        }
        path
    }

    /// Stand-ins for the desktop's FLAC upload parts of `path`, split at
    /// `bounds` (byte offsets in its audio).
    fn flac_parts(path: &Path, bounds: &[u64]) -> Vec<UploadPart> {
        let audio_bytes = fs::metadata(path).unwrap().len() - 44;
        let mut edges = vec![0];
        edges.extend_from_slice(bounds);
        edges.push(audio_bytes);
        edges
            .windows(2)
            .enumerate()
            .map(|(index, edge)| {
                let part = path.with_file_name(format!("audio.part-{}.flac", index + 1));
                fs::write(&part, format!("fLaC part {}", index + 1)).unwrap();
                UploadPart {
                    path: part,
                    pcm_start: edge[0],
                    pcm_end: edge[1],
                }
            })
            .collect()
    }

    fn saved_texts(path: &Path) -> Vec<(u64, u64, String)> {
        let saved: Value =
            serde_json::from_str(&fs::read_to_string(path.with_file_name("parts.json")).unwrap())
                .unwrap();
        saved["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| {
                (
                    part["start"].as_u64().unwrap(),
                    part["end"].as_u64().unwrap(),
                    part["text"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    struct LongRun {
        result: Result<String, TranscribeError>,
        prompts: Vec<String>,
        sent: Vec<(String, Option<Vec<u8>>)>,
        progress: Vec<(usize, usize)>,
        /// The WAV byte ranges read, in order.
        reads: Vec<(u64, u64)>,
        /// Each WAV upload's bytes.
        wavs: Vec<Vec<u8>>,
    }

    /// Transcribes `path` with its parts saved beside it, and FLAC upload
    /// parts when given.
    async fn transcribe_long(
        path: &Path,
        replies: Vec<Result<String, ApiError>>,
        prompt: &str,
        upload_parts: &[UploadPart],
    ) -> LongRun {
        let count = replies.len();
        let transcriber = transcriber(FakeApi::transcribing(replies));
        let progress = Mutex::new(Vec::new());
        // Each test has its own folder, so reads of `path` are this run's.
        let reads_before = audio_reads(path).len();
        let result = transcriber
            .transcribe_file(
                path,
                "audio/wav",
                prompt,
                &|current, total| lock(&progress).push((current, total)),
                Some(path.with_file_name("parts.json")),
                upload_parts,
            )
            .await;
        assert!(
            lock(&transcriber.api.transcripts).is_empty() || result.is_err(),
            "every one of {count} replies was used"
        );
        let sent = lock(&transcriber.api.sent).clone();
        let progress = lock(&progress).clone();
        let wavs = std::mem::take(&mut *lock(&transcriber.api.wavs));
        LongRun {
            result,
            prompts: sent.iter().map(|(_, prompt, _)| prompt.clone()).collect(),
            sent: sent
                .into_iter()
                .map(|(mime, _, flac)| (mime, flac))
                .collect(),
            progress,
            reads: audio_reads(path).split_off(reads_before),
            wavs,
        }
    }

    /// The WAV file a part of `path`'s audio is uploaded as: the header for
    /// its length, then its audio as the file holds it.
    fn wav_part(path: &Path, start: u64, end: u64) -> Vec<u8> {
        let audio = fs::read(path).unwrap();
        let pcm = &audio[44 + start as usize..44 + end as usize];
        [
            &crate::recording_store::wav_header(pcm.len() as u32, 1, 2, 24_000)[..],
            pcm,
        ]
        .concat()
    }

    /// The byte ranges read from `path`'s audio.
    fn audio_reads(path: &Path) -> Vec<(u64, u64)> {
        lock(&AUDIO_READS)
            .iter()
            .filter(|(read, _, _)| read == path)
            .map(|&(_, start, end)| (start, end))
            .collect()
    }

    fn ok(texts: &[&str]) -> Vec<Result<String, ApiError>> {
        texts.iter().map(|text| Ok(text.to_string())).collect()
    }

    #[tokio::test]
    async fn a_sixty_minute_wav_is_transcribed_in_bounded_ordered_chunks() {
        let dir = TempDir::new();
        let pcm_bytes: u64 = 60 * 60 * 24_000 * 2;
        let path = dir.0.join("sixty-minutes.wav");
        let mut file = File::create(&path).unwrap();
        file.write_all(&crate::recording_store::wav_header(
            pcm_bytes as u32,
            1,
            2,
            24_000,
        ))
        .unwrap();
        file.set_len(44 + pcm_bytes).unwrap();
        // A loud sample every megabyte, so no part is silent.
        for offset in (0..pcm_bytes).step_by(1 << 20) {
            file.seek(SeekFrom::Start(44 + offset)).unwrap();
            file.write_all(&SPEECH.to_le_bytes()).unwrap();
        }
        let parts = pcm_bytes.div_ceil(part_bytes()) as usize;
        let replies = (1..=parts).map(|n| Ok(format!("segment-{n}"))).collect();
        let transcriber = transcriber(FakeApi::transcribing(replies));
        let progress = Mutex::new(Vec::new());

        let text = transcriber
            .transcribe_file(
                &path,
                "audio/wav",
                "",
                &|current, total| lock(&progress).push((current, total)),
                None,
                &[],
            )
            .await
            .unwrap();

        assert!(parts > 1);
        assert_eq!(lock(&transcriber.api.sent).len(), parts);
        assert_eq!(
            text,
            (1..=parts)
                .map(|n| format!("segment-{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(
            *lock(&progress),
            (1..=parts).map(|n| (n, parts)).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn a_long_recording_is_sent_as_its_flac_parts_with_chained_prompts() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);
        let audio_bytes = fs::metadata(&path).unwrap().len() - 44;
        let parts = flac_parts(&path, &[2 * part_bytes()]);

        let run = transcribe_long(&path, ok(&["First.", "Second."]), "Zoon", &parts).await;
        let saved = saved_texts(&path);
        // A retry after both finished uploads nothing.
        let retry = transcribe_long(&path, vec![], "Zoon", &parts).await;

        assert_eq!(run.result.unwrap(), "First. Second.");
        assert_eq!(
            run.sent,
            vec![
                ("audio/flac".to_string(), Some(b"fLaC part 1".to_vec())),
                ("audio/flac".to_string(), Some(b"fLaC part 2".to_vec())),
            ]
        );
        assert_eq!(
            run.prompts,
            ["Zoon", "Zoon\nPrevious segment transcript: First."]
        );
        assert_eq!(run.progress, [(1, 2), (2, 2)]);
        assert_eq!(
            saved,
            [
                (0, 2 * part_bytes(), "First.".to_string()),
                (2 * part_bytes(), audio_bytes, "Second.".to_string()),
            ]
        );
        assert_eq!(retry.result.unwrap(), "First. Second.");
        assert!(retry.sent.is_empty());
    }

    #[tokio::test]
    async fn a_refused_flac_part_sends_the_rest_of_the_recording_as_wav() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);
        let audio_bytes = fs::metadata(&path).unwrap().len() - 44;
        let parts = flac_parts(&path, &[part_bytes()]);

        let run = transcribe_long(
            &path,
            vec![
                Ok("One.".into()),
                Err(bad_request(None)),
                Ok("Two.".into()),
                Ok("Three.".into()),
            ],
            "Zoon",
            &parts,
        )
        .await;
        // A retry tries the FLAC part once more, then reuses the WAV parts
        // that finished.
        let retry = transcribe_long(&path, vec![Err(bad_request(None))], "Zoon", &parts).await;

        assert_eq!(run.result.unwrap(), "One. Two. Three.");
        assert_eq!(
            run.sent
                .iter()
                .map(|(mime, _)| mime.as_str())
                .collect::<Vec<_>>(),
            ["audio/flac", "audio/flac", "audio/wav", "audio/wav"]
        );
        assert_eq!(
            run.prompts,
            [
                "Zoon",
                "Zoon\nPrevious segment transcript: One.",
                "Zoon\nPrevious segment transcript: One.",
                "Zoon\nPrevious segment transcript: Two.",
            ]
        );
        assert_eq!(run.progress, [(1, 2), (2, 2), (2, 3), (3, 3)]);
        // The WAV parts are the recording's own audio behind a header.
        assert!(
            run.wavs
                == [
                    wav_part(&path, part_bytes(), 2 * part_bytes()),
                    wav_part(&path, 2 * part_bytes(), audio_bytes),
                ]
        );
        assert_eq!(retry.result.unwrap(), "One. Two. Three.");
        assert_eq!(retry.prompts, ["Zoon\nPrevious segment transcript: One."]);
    }

    #[tokio::test]
    async fn flac_parts_that_do_not_cover_the_recording_are_not_sent() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);
        let short = &flac_parts(&path, &[part_bytes()])[..1];

        let run = transcribe_long(&path, ok(&["A.", "B.", "C."]), "Zoon", short).await;

        assert_eq!(run.result.unwrap(), "A. B. C.");
        assert!(run.sent.iter().all(|(mime, _)| mime == "audio/wav"));
    }

    #[tokio::test]
    async fn a_silent_flac_part_is_never_uploaded() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH, 0, 0]);
        let audio_bytes = fs::metadata(&path).unwrap().len() - 44;
        let parts = flac_parts(&path, &[part_bytes()]);

        let run = transcribe_long(&path, ok(&["Only part."]), "Zoon", &parts).await;

        assert_eq!(run.result.unwrap(), "Only part.");
        assert_eq!(
            run.sent,
            [("audio/flac".to_string(), Some(b"fLaC part 1".to_vec()))]
        );
        assert_eq!(
            saved_texts(&path),
            [
                (0, part_bytes(), "Only part.".to_string()),
                (part_bytes(), audio_bytes, String::new()),
            ]
        );
    }

    #[tokio::test]
    async fn long_recordings_follow_the_part_rules_shared_with_android() {
        let cases: Value = serde_json::from_str(include_str!(
            "../../../../packages/protocol/audio-examples/cases.json"
        ))
        .unwrap();
        let cases = cases["long_recording_parts"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            for as_flac in [false, true] {
                let about = format!("{} (as_flac={as_flac})", case["about"]);
                let replies: Vec<Option<&str>> = case["parts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(Value::as_str)
                    .collect();
                let dir = TempDir::new();
                // A silent part (null) never reaches the silence level.
                let peaks: Vec<i16> = replies
                    .iter()
                    .map(|reply| if reply.is_some() { SPEECH } else { 0 })
                    .collect();
                let path = long_wav(&dir.0, &peaks);
                let bounds: Vec<u64> = (1..replies.len() as u64)
                    .map(|n| n * part_bytes())
                    .collect();
                let parts = if as_flac {
                    flac_parts(&path, &bounds)
                } else {
                    Vec::new()
                };

                let run = transcribe_long(
                    &path,
                    replies
                        .iter()
                        .flatten()
                        .map(|text| Ok(text.to_string()))
                        .collect(),
                    "Zoon",
                    &parts,
                )
                .await;

                assert_eq!(run.result.unwrap(), case["transcript"], "{about}");
                let expected_prompts: Vec<String> = case["quoted"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|quoted| {
                        if quoted.is_empty() {
                            "Zoon".to_string()
                        } else {
                            format!("Zoon\nPrevious segment transcript: {quoted}")
                        }
                    })
                    .collect();
                assert_eq!(run.prompts, expected_prompts, "{about}");
                let dropped: Vec<u64> = case["dropped"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|index| index.as_u64().unwrap())
                    .collect();
                let expected_saved: Vec<String> = replies
                    .iter()
                    .enumerate()
                    .map(|(index, reply)| match reply {
                        Some(text) if !dropped.contains(&(index as u64)) => text.trim().to_string(),
                        _ => String::new(),
                    })
                    .collect();
                let saved: Vec<String> = saved_texts(&path)
                    .into_iter()
                    .map(|(_, _, text)| text)
                    .collect();
                assert_eq!(saved, expected_saved, "{about}");
            }
        }
    }

    #[tokio::test]
    async fn a_retry_after_a_failed_last_part_sends_only_that_part() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);
        let network = ApiError::Connection("reset".into());

        let failed = transcribe_long(
            &path,
            vec![
                Ok("First part.".into()),
                Ok("Second part.".into()),
                Err(network),
            ],
            "Zoon",
            &[],
        )
        .await;
        let retry = transcribe_long(&path, ok(&["Third part."]), "Zoon", &[]).await;

        assert!(failed.result.is_err());
        assert_eq!(
            retry.result.unwrap(),
            "First part. Second part. Third part."
        );
        // The last part's prompt is rebuilt from the saved text before it.
        assert_eq!(
            retry.prompts,
            ["Zoon\nPrevious segment transcript: Second part."]
        );
        // Only the last part's audio is read and uploaded again.
        let audio_bytes = fs::metadata(&path).unwrap().len() - 44;
        assert_eq!(failed.reads.len(), 3);
        assert_eq!(retry.reads, [(2 * part_bytes(), audio_bytes)]);
        assert_eq!(retry.sent.len(), 1);
    }

    #[tokio::test]
    async fn a_refused_wav_part_is_reported_without_sending_it_again() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);

        let run = transcribe_long(
            &path,
            vec![Ok("First part.".into()), Err(bad_request(None))],
            "Zoon",
            &[],
        )
        .await;

        assert_eq!(
            run.result.unwrap_err().to_string(),
            "Error code: 400 - Could not process input"
        );
        assert_eq!(run.sent.len(), 2);
        // The part that finished stays saved for a retry.
        assert_eq!(
            saved_texts(&path)
                .into_iter()
                .map(|(_, _, text)| text)
                .collect::<Vec<_>>(),
            ["First part."]
        );
    }

    #[tokio::test]
    async fn a_retry_resumes_parts_saved_with_their_prompts_by_an_older_version() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);
        let failed = transcribe_long(
            &path,
            vec![
                Ok("First part.".into()),
                Ok("Second part.".into()),
                Err(ApiError::Connection("reset".into())),
            ],
            "Zoon",
            &[],
        )
        .await;
        // Older versions also saved the prompt each part was sent with.
        let parts_path = path.with_file_name("parts.json");
        let mut saved: Value =
            serde_json::from_str(&fs::read_to_string(&parts_path).unwrap()).unwrap();
        let older_prompts = ["Zoon", "Zoon\nPrevious segment transcript: First part."];
        let parts = saved["parts"].as_array_mut().unwrap();
        assert!(parts.iter().all(|part| part.get("prompt").is_none()));
        for (part, prompt) in parts.iter_mut().zip(older_prompts) {
            part["prompt"] = prompt.into();
        }
        fs::write(&parts_path, saved.to_string()).unwrap();

        let retry = transcribe_long(&path, ok(&["Third part."]), "Zoon", &[]).await;

        assert!(failed.result.is_err());
        assert_eq!(
            retry.result.unwrap(),
            "First part. Second part. Third part."
        );
        assert_eq!(
            retry.prompts,
            ["Zoon\nPrevious segment transcript: Second part."]
        );
        assert_eq!(retry.sent.len(), 1);
    }

    #[tokio::test]
    async fn a_recording_cut_short_is_reused_on_a_retry() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);
        // The header claims more audio than the file holds, as after a crash
        // mid-recording: the last part ends where the file does.
        let audio_bytes = fs::metadata(&path).unwrap().len() - 44;
        let mut file = File::options().write(true).open(&path).unwrap();
        file.write_all(&crate::recording_store::wav_header(
            (audio_bytes + 24_000) as u32,
            1,
            2,
            24_000,
        ))
        .unwrap();
        drop(file);

        let run = transcribe_long(&path, ok(&["One.", "Two.", "Three."]), "Zoon", &[]).await;
        let retry = transcribe_long(&path, ok(&[]), "Zoon", &[]).await;

        assert_eq!(run.result.unwrap(), "One. Two. Three.");
        // The last part's header states the audio the file holds, not the
        // length its own header claims.
        assert!(
            run.wavs
                == [
                    wav_part(&path, 0, part_bytes()),
                    wav_part(&path, part_bytes(), 2 * part_bytes()),
                    wav_part(&path, 2 * part_bytes(), audio_bytes),
                ]
        );
        assert_eq!(
            saved_texts(&path)[2],
            (2 * part_bytes(), audio_bytes, "Three.".into())
        );
        assert_eq!(retry.result.unwrap(), "One. Two. Three.");
        assert!(retry.prompts.is_empty());
        // Every part is reused without reading its audio.
        assert!(retry.reads.is_empty());
    }

    #[tokio::test]
    async fn an_empty_part_that_finished_is_not_sent_again() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);

        let failed = transcribe_long(
            &path,
            vec![
                Ok("First part.".into()),
                Ok(String::new()),
                Err(ApiError::Connection("reset".into())),
            ],
            "Zoon",
            &[],
        )
        .await;
        let retry = transcribe_long(&path, ok(&["Third part."]), "Zoon", &[]).await;

        assert!(failed.result.is_err());
        assert_eq!(retry.result.unwrap(), "First part. Third part.");
        // The prompt quotes the last part that had text.
        assert_eq!(
            retry.prompts,
            ["Zoon\nPrevious segment transcript: First part."]
        );
    }

    #[tokio::test]
    async fn quiet_speech_and_a_short_last_part_are_still_uploaded() {
        let dir = TempDir::new();
        let quiet = rules().silent_pcm_peak as i16;
        let path = long_wav(&dir.0, &[SPEECH, quiet]);

        let run = transcribe_long(&path, ok(&["Loud.", "Quiet."]), "", &[]).await;

        assert_eq!(run.result.unwrap(), "Loud. Quiet.");
        assert_eq!(run.prompts, ["", "Previous segment transcript: Loud."]);
    }

    #[tokio::test]
    async fn changed_settings_or_audio_start_the_parts_afresh() {
        for change in ["prompt", "audio"] {
            let dir = TempDir::new();
            let path = long_wav(&dir.0, &[SPEECH; 3]);
            let failed = transcribe_long(
                &path,
                vec![Ok("One.".into()), Ok("Two.".into()), Err(ApiError::Timeout)],
                "Zoon",
                &[],
            )
            .await;
            let prompt = if change == "prompt" {
                "Zoon Robotics"
            } else {
                let file = File::options().write(true).open(&path).unwrap();
                let later = fs::metadata(&path).unwrap().modified().unwrap()
                    + std::time::Duration::from_secs(1);
                file.set_modified(later).unwrap();
                "Zoon"
            };

            let retry = transcribe_long(&path, ok(&["Uno.", "Dos.", "Tres."]), prompt, &[]).await;

            assert!(failed.result.is_err());
            assert_eq!(retry.result.unwrap(), "Uno. Dos. Tres.", "{change}");
            assert_eq!(retry.prompts.len(), 3, "{change}");
        }
    }

    #[tokio::test]
    async fn a_damaged_parts_file_starts_afresh() {
        let dir = TempDir::new();
        let path = long_wav(&dir.0, &[SPEECH; 3]);
        fs::write(path.with_file_name("parts.json"), "{not json").unwrap();

        let run = transcribe_long(&path, ok(&["One.", "Two.", "Three."]), "Zoon", &[]).await;

        assert_eq!(run.result.unwrap(), "One. Two. Three.");
        assert_eq!(run.prompts.len(), 3);
    }

    /// A saved one-second microphone recording.
    fn recording(dir: &Path) -> SavedAudio {
        let audio_path = dir.join("audio.wav");
        let pcm: Vec<u8> = (0..24_000_i32)
            .flat_map(|n| (((n % 50) * 200) as i16).to_le_bytes())
            .collect();
        fs::write(&audio_path, wav_bytes(&pcm, 1, 2, 24_000)).unwrap();
        SavedAudio {
            audio_path,
            mime_type: "audio/wav".into(),
            microphone: true,
            parts_path: Some(dir.join("parts.json")),
        }
    }

    /// The files in `dir`: only the recording once its upload parts are
    /// deleted.
    fn files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn single_flac(dir: &Path, bytes: &[u8], end: u64) -> Vec<UploadPart> {
        let path = dir.join("audio.part-1.flac");
        fs::write(&path, bytes).unwrap();
        vec![UploadPart {
            path,
            pcm_start: 0,
            pcm_end: end,
        }]
    }

    #[test]
    fn upload_parts_still_encoding_when_cancelled_are_deleted_once_written() {
        let dir = TempDir::new();
        let recording = recording(&dir.0);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (written, wait_for_written) = mpsc::channel();
        let (release, wait_for_release) = mpsc::channel();
        lock(&ENCODER_PAUSES).push((recording.audio_path.clone(), written, wait_for_release));
        runtime.block_on(async {
            let mut encoding = std::pin::pin!(flac_upload_parts(&recording.audio_path));
            // One poll starts the encoder, which then runs on by itself.
            assert!(futures_util::FutureExt::now_or_never(&mut encoding).is_none());
            // Cancelled while the encoder still holds the parts it wrote.
            wait_for_written
                .recv_timeout(Duration::from_secs(10))
                .expect("the encoder wrote its parts");
            assert_eq!(files(&dir.0).len(), 2);
        });
        release.send(()).unwrap();
        // Dropping the runtime waits for the encoder to finish.
        drop(runtime);

        assert_eq!(files(&dir.0), ["audio.wav"]);
    }

    #[tokio::test]
    async fn a_dictations_single_upload_part_is_sent_in_place_of_the_saved_wav() {
        let dir = TempDir::new();
        let recording = recording(&dir.0);
        let transcriber = transcriber(FakeApi::transcribing(ok(&["Hello."])));

        let text = transcriber
            .transcribe_recording(&recording, "Zoon", &|_, _| {})
            .await;

        assert_eq!(text.unwrap(), "Hello.");
        let sent = lock(&transcriber.api.sent);
        assert_eq!(sent.len(), 1);
        let (mime, prompt, upload) = &sent[0];
        assert_eq!((mime.as_str(), prompt.as_str()), ("audio/flac", "Zoon"));
        assert!(upload.as_ref().unwrap().starts_with(b"fLaC"));
        assert_eq!(files(&dir.0), ["audio.wav"]);
    }

    #[tokio::test]
    async fn a_refused_single_upload_part_falls_back_to_the_saved_wav_once() {
        let dir = TempDir::new();
        let recording = recording(&dir.0);
        let transcriber = transcriber(FakeApi::transcribing(vec![
            Err(bad_request(None)),
            Ok("From WAV.".into()),
        ]));

        let text = transcriber
            .transcribe_recording(&recording, "Zoon", &|_, _| {})
            .await;

        assert_eq!(text.unwrap(), "From WAV.");
        let sent = lock(&transcriber.api.sent);
        assert_eq!(
            sent.iter()
                .map(|(mime, _, _)| mime.as_str())
                .collect::<Vec<_>>(),
            ["audio/flac", "audio/wav"]
        );
        assert_eq!(files(&dir.0), ["audio.wav"]);
    }

    #[tokio::test]
    async fn other_upload_failures_are_reported_without_a_second_upload() {
        let dir = TempDir::new();
        let recording = recording(&dir.0);
        let transcriber = transcriber(FakeApi::transcribing(vec![Err(ApiError::Status {
            status: 401,
            code: Some("invalid_api_key".into()),
            param: None,
            message: "Incorrect API key provided".into(),
        })]));

        let error = transcriber
            .transcribe_recording(&recording, "Zoon", &|_, _| {})
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Error code: 401 - Incorrect API key provided"
        );
        assert_eq!(lock(&transcriber.api.sent).len(), 1);
        // The upload part is deleted on an error too.
        assert_eq!(files(&dir.0), ["audio.wav"]);
    }

    #[tokio::test]
    async fn missing_empty_oversized_or_misplaced_upload_parts_send_the_saved_wav() {
        let dir = TempDir::new();
        let missing = vec![UploadPart {
            path: dir.0.join("gone.flac"),
            pcm_start: 0,
            pcm_end: 48_000,
        }];
        let empty_path = dir.0.join("empty.flac");
        fs::write(&empty_path, b"").unwrap();
        let empty = vec![UploadPart {
            path: empty_path,
            pcm_start: 0,
            pcm_end: 48_000,
        }];
        let misplaced = vec![UploadPart {
            pcm_start: 2,
            ..single_flac(&dir.0, b"fLaC", 48_000)[0].clone()
        }];
        let oversized_path = dir.0.join("oversized.flac");
        File::create(&oversized_path)
            .unwrap()
            .set_len(max_upload_bytes() + 1)
            .unwrap();
        let oversized = vec![UploadPart {
            path: oversized_path,
            pcm_start: 0,
            pcm_end: 48_000,
        }];
        for parts in [missing, empty, oversized, misplaced] {
            let recording = recording(&dir.0);
            let transcriber = transcriber(FakeApi::transcribing(ok(&["From WAV."])));

            let text = transcriber
                .transcribe_with_uploads(&recording, "Zoon", &|_, _| {}, &parts)
                .await;

            assert_eq!(text.unwrap(), "From WAV.");
            assert_eq!(lock(&transcriber.api.sent)[0].0, "audio/wav");
        }
    }

    #[tokio::test]
    async fn only_microphone_recordings_are_uploaded_as_flac() {
        let dir = TempDir::new();
        let imported = SavedAudio {
            microphone: false,
            ..recording(&dir.0)
        };
        let transcriber = transcriber(FakeApi::transcribing(ok(&["Imported WAV."])));

        let text = transcriber
            .transcribe_recording(&imported, "Zoon", &|_, _| {})
            .await;

        assert_eq!(text.unwrap(), "Imported WAV.");
        assert_eq!(lock(&transcriber.api.sent)[0].0, "audio/wav");
    }

    #[tokio::test]
    async fn compressed_imports_go_straight_to_the_model() {
        let dir = TempDir::new();
        let path = dir.0.join("import.mp3");
        fs::write(&path, b"ID3 mp3 audio").unwrap();
        let transcriber = transcriber(FakeApi::transcribing(ok(&["Imported."])));
        let recording = SavedAudio {
            audio_path: path,
            mime_type: "audio/mpeg".into(),
            microphone: false,
            parts_path: None,
        };

        let text = transcriber
            .transcribe_recording(&recording, "", &|_, _| {})
            .await;

        assert_eq!(text.unwrap(), "Imported.");
        assert_eq!(lock(&transcriber.api.sent)[0].0, "audio/mpeg");
    }

    #[tokio::test]
    async fn polish_asks_for_minimal_reasoning() {
        let api = FakeApi::default();
        lock(&api.responses).push_back(Ok("  Tidied.  ".into()));
        let transcriber = transcriber(api);

        let text = transcriber.polish("um tidied", "").await.unwrap();

        assert_eq!(text, "Tidied.");
        assert_eq!(
            *lock(&transcriber.api.tidied),
            [("um tidied".to_string(), "minimal".to_string())]
        );
    }

    #[tokio::test]
    async fn rejected_minimal_reasoning_retries_once_with_low_and_remembers_it() {
        let api = FakeApi::default();
        lock(&api.responses).extend([
            Err(bad_request(Some("reasoning.effort"))),
            Ok("First.".into()),
            Ok("Second.".into()),
        ]);
        let transcriber = transcriber(api);

        assert_eq!(transcriber.polish("first", "").await.unwrap(), "First.");
        assert_eq!(transcriber.polish("second", "").await.unwrap(), "Second.");

        let efforts: Vec<String> = lock(&transcriber.api.tidied)
            .iter()
            .map(|(_, e)| e.clone())
            .collect();
        assert_eq!(efforts, ["minimal", "low", "low"]);
        assert_eq!(*lock(&transcriber.polish.effort), "low");
    }

    #[tokio::test]
    async fn rejected_low_reasoning_and_other_errors_are_not_retried() {
        let api = FakeApi::default();
        lock(&api.responses).extend([Err(bad_request(Some("reasoning.effort")))]);
        let transcriber = transcriber(api);
        *lock(&transcriber.polish.effort) = "low".into();
        assert!(transcriber.polish("text", "").await.is_err());

        let api = FakeApi::default();
        lock(&api.responses).extend([Err(bad_request(Some("input")))]);
        let transcriber2 = self::transcriber(api);
        assert!(transcriber2.polish("text", "").await.is_err());

        assert_eq!(lock(&transcriber.api.tidied).len(), 1);
        assert_eq!(lock(&transcriber2.api.tidied).len(), 1);
    }

    #[tokio::test]
    async fn a_long_transcript_is_tidied_in_sentence_chunks_kept_in_order() {
        let sentence = format!("{}.", "word ".repeat(600).trim());
        let text = vec![sentence.as_str(); 8].join(" ");
        let chunks = chunk_for_polish(&text);
        let api = FakeApi::default();
        lock(&api.responses).extend((1..=chunks.len()).map(|n| Ok(format!("Chunk {n}."))));
        let transcriber = transcriber(api);

        let tidied = transcriber.polish(&text, "").await.unwrap();

        assert!(chunks.len() > 1);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.chars().count() <= MAX_POLISH_CHUNK_CHARS));
        assert_eq!(chunks.join(" "), text);
        assert_eq!(
            tidied,
            (1..=chunks.len())
                .map(|n| format!("Chunk {n}."))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn a_custom_prompt_is_added_to_the_tidy_rules() {
        assert_eq!(build_polish_system("  "), POLISH_SYSTEM);
        let system = build_polish_system("Use British spelling.");
        assert!(system.starts_with(POLISH_SYSTEM));
        assert!(system.ends_with("Use British spelling."));
    }

    #[test]
    fn sentences_split_after_their_closing_quotes() {
        assert_eq!(
            split_sentences("He said \"Hi.\" Then left!  Why? ok"),
            ["He said \"Hi.\"", "Then left!", "Why?", "ok"]
        );
        assert_eq!(split_sentences("No end"), ["No end"]);
    }

    #[test]
    fn short_clean_transcripts_are_recognised() {
        for text in [
            "Send me the report by Friday.",
            "  Can you call me back?  ",
            "I like it, thanks!",
            "3 people are coming.",
            "Thanks, see you tomorrow.",
            "Meet me at 3.30 today.",
        ] {
            assert!(is_short_clean_transcript(text), "{text:?} needs no tidying");
        }
        for text in [
            "",
            "send me the report by Friday.",
            "Send me the report by Friday",
            "Um, send me the report.",
            "So we should go.",
            "It works, right?",
            "I want to -- I think we should go.",
            "Send the the report.",
            "Send The the report.",
            "Buy eggs, milk, and bread.",
            "Note: buy eggs.",
            "Put this in capitals please.",
            "New paragraph please.",
            "Like, it works.",
            "First sentence. Second sentence.",
            "One two three four five six seven eight nine ten eleven twelve.",
            "Line one.\nLine two.",
            "Wait... what?",
            "send me the report by friday.",
            "Send me the report. Thanks.",
            "Send it now\nThanks.",
            "Please send me the full quarterly sales report and the budget by Friday.",
            "Uh send me the report.",
            "Send me, you know, the report.",
            "Send me the, like, report.",
            "It is basically done.",
            "Send me the report, right?",
            "I want\u{2014} I think we should go.",
            "Make the next word bold please.",
            "New line thanks for coming.",
            "Buy apples, pears, plums and figs.",
        ] {
            assert!(!is_short_clean_transcript(text), "{text:?} needs tidying");
        }
    }
}
