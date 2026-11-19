//! Dictation sessions: each dictation starts one with the settings saved at
//! that moment, and it ends with one `final` or `error` event.
//!
//! A Standard session is transcribed from the saved recording on disk when
//! the user stops (src/transcriber.rs). A Live session opens a Realtime
//! websocket at once and streams the audio while the user speaks
//! (src/realtime.rs). A Live session finalized with its saved recording
//! instead (its connection failed) is transcribed from that file with the
//! Standard model. Either is tidied before its final event. Progress,
//! live text, warnings and the result reach the window as
//! `worker-session-event` payloads: `{ session_id, event, done }`.
//!
//! Stopping a session cancels its token: whatever request it is waiting for
//! is dropped at once and it sends no more events.
//!
//! This file needs no Tauri, so its tests run in `core-tests`.

use crate::audio_rules::{
    echoes_prompt, no_speech_reason, no_speech_reason_from_counters, pcm_peak, pcm_rate, rules,
};
use crate::debug_log::debug_log_line;
use crate::models::{
    default_transcription_model, live_transcription_model, refine_model,
    session_transcription_model,
};
use crate::openai_api::{HttpApi, DEFAULT_BASE_URL};
use crate::realtime::{DeltaHandler, RealtimeStream, FINAL_TIMEOUT};
use crate::transcriber::{
    is_short_clean_transcript, PolishSettings, SavedAudio, Transcriber,
    DEFAULT_POLISH_REASONING_EFFORT,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The Tauri event carrying one session update to the window.
pub const SESSION_EVENT: &str = "worker-session-event";

/// A Live session moves to a new Realtime connection after this much audio
/// (25 minutes), before the service's session limit.
pub const LIVE_ROTATE_BYTES: u64 = 24_000 * 2 * 25 * 60;
/// Shorter prompt fragments are too common to remove from an error message
/// without mangling it.
const MIN_REDACT_CHARS: usize = 4;
/// How long one Live audio chunk may take to send. A connection whose peer
/// stops reading is closed after this, so later chunks and the stop fail
/// instead of waiting for ever.
const APPEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The settings a dictation uses, read when it starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionSettings {
    pub profile_id: String,
    /// Empty falls back to the `OPENAI_API_KEY` environment variable.
    pub openai_api_key: String,
    /// Empty falls back to `OPENWHISPER_MODEL`, then the default model.
    pub transcription_model: String,
    pub prompt: String,
    pub refine_enabled: bool,
    pub refine_prompt: String,
    /// Opens no Live connection: a Live dictation whose connection failed
    /// keeps recording, and is transcribed from its saved recording with
    /// Standard transcription when it stops.
    pub standard_only: bool,
}

pub enum FinalizeAudio {
    /// The chunks a Live session was sent, in this raw PCM type.
    Live {
        mime_type: String,
    },
    /// The saved recording, always transcribed with Standard transcription.
    Saved(SavedAudio),
}

pub type EventSink = Arc<dyn Fn(Value) + Send + Sync>;

struct Session {
    id: String,
    prompt: String,
    refine_enabled: bool,
    refine_prompt: String,
    api_key: String,
    model: String,
    live: bool,
    cancel: CancellationToken,
    state: Mutex<SessionState>,
    /// Held while a Live chunk is sent (and the connection rotated), so
    /// chunks go out in order and finalizing waits for the last one.
    sending: tokio::sync::Mutex<LiveProgress>,
}

#[derive(Default)]
struct SessionState {
    finalizing: bool,
    /// The Live connection; stop closes it at once.
    stream: Option<Arc<RealtimeStream>>,
    /// Counters over the chunks a Live session appends, for the no-speech
    /// check at the end. The desktop already saves the audio, so no copy is
    /// kept here.
    audio_bytes: u64,
    peak: i32,
}

#[derive(Default)]
struct LiveProgress {
    /// The text of earlier Live connections, in order.
    segments: Vec<String>,
    /// Audio sent on the current connection.
    stream_bytes: u64,
}

pub struct Sessions {
    client: reqwest::Client,
    base_url: String,
    polish: PolishSettings,
    live_rotate_bytes: u64,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    emit: EventSink,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn bad_request(message: &str) -> String {
    format!("bad_request: {message}")
}

fn unknown_session() -> String {
    bad_request("Unknown session_id")
}

/// Removes the user's prompts from an error message before it is shown or
/// logged. Errors can quote the request they came from; the prompts are the
/// user's own words and must not reach notices, logs or bug reports.
fn without_private_text(message: &str, private: &[&str]) -> String {
    let values: BTreeSet<&str> = private.iter().map(|value| value.trim()).collect();
    let mut values: Vec<&str> = values.into_iter().collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.chars().count()));
    let mut message = message.to_string();
    for value in values {
        if value.chars().count() >= MIN_REDACT_CHARS {
            message = message.replace(value, "[redacted]");
        }
    }
    message
}

impl Sessions {
    pub fn new(emit: EventSink) -> Result<Self, String> {
        Ok(Self::with_base_url(
            crate::openai_api::new_http_client()?,
            DEFAULT_BASE_URL,
            emit,
        ))
    }

    /// Sessions that talk to `base_url` instead of OpenAI (tests use a local
    /// stand-in).
    pub fn with_base_url(client: reqwest::Client, base_url: &str, emit: EventSink) -> Self {
        Self {
            client,
            base_url: base_url.to_string(),
            polish: PolishSettings {
                model: env_or("OPENWHISPER_POLISH_MODEL", refine_model()),
                effort: Arc::new(Mutex::new(env_or(
                    "OPENWHISPER_POLISH_REASONING",
                    DEFAULT_POLISH_REASONING_EFFORT,
                ))),
            },
            live_rotate_bytes: LIVE_ROTATE_BYTES,
            sessions: Mutex::new(HashMap::new()),
            emit,
        }
    }

    fn session(&self, session_id: &str) -> Result<Arc<Session>, String> {
        lock(&self.sessions)
            .get(session_id)
            .cloned()
            .ok_or_else(unknown_session)
    }

    fn transcriber_for(&self, session: &Session, model: &str) -> Transcriber<HttpApi> {
        Transcriber::new(
            HttpApi::new(self.client.clone(), &self.base_url, &session.api_key),
            model,
            self.polish.clone(),
        )
    }

    /// Sends one session update to the window at once. A final or error
    /// event ends the session, so it is forgotten here and its transcript
    /// does not stay in memory. A stopped session sends nothing.
    fn emit(&self, session: &Session, event: Value) {
        if session.cancel.is_cancelled() {
            return;
        }
        let done = matches!(event["type"].as_str(), Some("final" | "error"));
        if done {
            lock(&self.sessions).remove(&session.id);
        }
        (self.emit)(json!({ "session_id": session.id, "event": event, "done": done }));
    }

    /// Live text goes to the window as it arrives, until the session stops.
    fn delta_handler(&self, session_id: &str, cancel: &CancellationToken) -> DeltaHandler {
        let emit = self.emit.clone();
        let session_id = session_id.to_string();
        let cancel = cancel.clone();
        Arc::new(move |text: String| {
            if !cancel.is_cancelled() {
                emit(json!({
                    "session_id": session_id,
                    "event": { "type": "partial", "text": text },
                    "done": false,
                }));
            }
        })
    }

    async fn connect_live(&self, session: &Session) -> Result<RealtimeStream, String> {
        RealtimeStream::connect(
            &self.base_url,
            &session.api_key,
            &session.model,
            self.delta_handler(&session.id, &session.cancel),
        )
        .await
    }

    /// Starts a dictation and returns its session id. A Live dictation
    /// connects at once, so the first words are heard.
    pub async fn start(&self, settings: SessionSettings) -> Result<String, String> {
        let requested = if settings.transcription_model.trim().is_empty() {
            env_or("OPENWHISPER_MODEL", &default_transcription_model())
        } else {
            settings.transcription_model.clone()
        };
        let mut model =
            session_transcription_model(&requested).map_err(|error| bad_request(&error))?;
        if settings.standard_only && model == live_transcription_model() {
            model = default_transcription_model();
        }
        let api_key = env_or("OPENAI_API_KEY", "");
        let api_key = match settings.openai_api_key.trim() {
            "" => api_key,
            saved => saved.to_string(),
        };
        // Never log the key, the prompts or the dictated text.
        debug_log_line(
            "transcriber",
            &format!(
                "start_session profile_id={:?} model={model} standard_only={} \
                 transcription_prompt_chars={} refine_enabled={} refine_prompt_chars={}",
                settings.profile_id,
                settings.standard_only,
                settings.prompt.chars().count(),
                settings.refine_enabled,
                settings.refine_prompt.chars().count()
            ),
        );
        let session = Arc::new(Session {
            id: Uuid::new_v4().to_string(),
            prompt: settings.prompt,
            refine_enabled: settings.refine_enabled,
            refine_prompt: settings.refine_prompt,
            api_key,
            live: model == live_transcription_model(),
            model,
            cancel: CancellationToken::new(),
            state: Mutex::new(SessionState::default()),
            sending: tokio::sync::Mutex::new(LiveProgress::default()),
        });
        if session.live {
            let stream = self
                .connect_live(&session)
                .await
                .map_err(|error| format!("internal_error: {error}"))?;
            lock(&session.state).stream = Some(Arc::new(stream));
        }
        lock(&self.sessions).insert(session.id.clone(), session.clone());
        Ok(session.id.clone())
    }

    /// Stops a session at once: a running transcription is dropped and
    /// sends nothing more.
    pub fn stop(&self, session_id: &str) -> Result<(), String> {
        let session = lock(&self.sessions)
            .remove(session_id)
            .ok_or_else(unknown_session)?;
        Self::cancel(&session);
        Ok(())
    }

    fn cancel(session: &Session) {
        // Under the state lock, so a Live rotation either sees the cancel or
        // has its new connection closed here.
        let stream = {
            let mut state = lock(&session.state);
            session.cancel.cancel();
            state.stream.take()
        };
        if let Some(stream) = stream {
            stream.close();
        }
    }

    /// Sends a Live session's next chunk of 16-bit PCM.
    pub async fn append(&self, session_id: &str, chunk: &[u8]) -> Result<(), String> {
        let session = self.session(session_id)?;
        if !session.live {
            // Standard sessions are transcribed from the saved file.
            return Err(bad_request("Only Live sessions take audio chunks"));
        }
        if chunk.is_empty() {
            return Err(bad_request("The audio chunk is empty"));
        }
        {
            let mut state = lock(&session.state);
            if session.cancel.is_cancelled() {
                return Err(unknown_session());
            }
            if state.finalizing {
                return Err(bad_request("Session is already finalizing"));
            }
            state.audio_bytes += chunk.len() as u64;
            state.peak = state.peak.max(pcm_peak(chunk));
        }
        let mut progress = session.sending.lock().await;
        if progress.stream_bytes > 0
            && progress.stream_bytes + chunk.len() as u64 > self.live_rotate_bytes
        {
            self.rotate(&session, &mut progress).await?;
        }
        let stream = lock(&session.state)
            .stream
            .clone()
            .ok_or_else(unknown_session)?;
        match until_cancelled(
            &session.cancel,
            tokio::time::timeout(APPEND_TIMEOUT, stream.append(chunk)),
        )
        .await
        {
            None => return Err(unknown_session()),
            Some(Err(_)) => {
                stream.close();
                return Err("internal_error: Realtime audio send timed out".into());
            }
            Some(Ok(sent)) => sent.map_err(|error| format!("internal_error: {error}"))?,
        }
        progress.stream_bytes += chunk.len() as u64;
        Ok(())
    }

    /// Finishes the current Live connection's text and opens the next one.
    async fn rotate(&self, session: &Session, progress: &mut LiveProgress) -> Result<(), String> {
        let current = lock(&session.state)
            .stream
            .clone()
            .ok_or_else(unknown_session)?;
        let segment = until_cancelled(&session.cancel, current.finalize(FINAL_TIMEOUT))
            .await
            .ok_or_else(unknown_session)?
            .map_err(|error| format!("internal_error: {error}"))?;
        let segment = segment.trim();
        if !segment.is_empty() {
            progress.segments.push(segment.to_string());
        }
        let replacement = until_cancelled(&session.cancel, self.connect_live(session))
            .await
            .ok_or_else(unknown_session)?
            .map_err(|error| format!("internal_error: {error}"))?;
        {
            let mut state = lock(&session.state);
            if !session.cancel.is_cancelled() {
                state.stream = Some(Arc::new(replacement));
                progress.stream_bytes = 0;
                return Ok(());
            }
        }
        replacement.close();
        Err(unknown_session())
    }

    /// Transcribes and tidies a session's audio. Its result arrives as a
    /// `final` or `error` event before this returns; a stopped session
    /// sends nothing.
    pub async fn finalize(&self, session_id: &str, audio: FinalizeAudio) -> Result<(), String> {
        let session = self.session(session_id)?;
        {
            let mut state = lock(&session.state);
            if state.finalizing {
                return Err(bad_request("Session is already finalizing"));
            }
            if session.cancel.is_cancelled() {
                Self::log_cancelled(&session);
                return Ok(());
            }
            state.finalizing = true;
        }
        let finished = until_cancelled(&session.cancel, self.run_finalize(&session, audio)).await;
        // Finalizing always ends a session, even when it fails.
        lock(&self.sessions).remove(&session.id);
        if finished.is_none() {
            Self::log_cancelled(&session);
        }
        Ok(())
    }

    fn log_cancelled(session: &Session) {
        debug_log_line(
            "transcriber",
            &format!("finalize session_id={:?} cancelled", session.id),
        );
    }

    async fn run_finalize(&self, session: &Session, audio: FinalizeAudio) {
        let progress = session.sending.lock().await;
        let saved = match audio {
            FinalizeAudio::Saved(saved) => {
                // A Live session given its saved recording (its connection
                // failed) is done with the connection; the file is read below
                // with Standard transcription.
                Self::close_stream(session);
                Some(saved)
            }
            FinalizeAudio::Live { mime_type } => {
                let (audio_bytes, peak) = {
                    let state = lock(&session.state);
                    (state.audio_bytes, state.peak)
                };
                // Only a Live session appends chunks; any other session
                // finalized without a saved file has no audio.
                if audio_bytes == 0 || !session.live {
                    return self.finish_without_audio(session, "no_bytes");
                }
                if progress.segments.is_empty() {
                    let reason = pcm_rate(&mime_type)
                        .and_then(|rate| no_speech_reason_from_counters(audio_bytes, peak, rate));
                    if let Some(reason) = reason {
                        return self.finish_without_audio(session, &reason);
                    }
                }
                None
            }
        };
        if let Some(saved) = &saved {
            let size = std::fs::metadata(&saved.audio_path)
                .map(|m| m.len())
                .unwrap_or(0);
            if size == 0 {
                return self.finish_without_audio(session, "no_bytes");
            }
            if let Some(reason) = no_speech_reason(&saved.audio_path, &saved.mime_type) {
                return self.finish_without_audio(session, &reason);
            }
        }

        // Live transcription hears audio only as it is appended, so a saved
        // file is always transcribed with the Standard model.
        let model = if saved.is_some() && session.live {
            default_transcription_model()
        } else {
            session.model.clone()
        };
        let transcriber = self.transcriber_for(session, &model);
        let transcribed = match saved {
            Some(saved) => {
                let on_progress = |current: usize, total: usize| {
                    self.emit(
                        session,
                        json!({ "type": "partial", "text": format!("Transcribing part {current} of {total}...") }),
                    );
                };
                transcriber
                    .transcribe_recording(&saved, &session.prompt, &on_progress)
                    .await
                    .map_err(|error| error.to_string())
            }
            None => {
                let stream = lock(&session.state).stream.take();
                match stream {
                    Some(stream) => stream.finalize(FINAL_TIMEOUT).await.map(|last| {
                        let mut pieces = progress.segments.clone();
                        pieces.push(last.trim().to_string());
                        pieces.join(" ").trim().to_string()
                    }),
                    None => Err("Realtime transcription session is closed".to_string()),
                }
            }
        };
        drop(progress);
        let mut text = match transcribed {
            Ok(text) => text,
            Err(error) => {
                let message = without_private_text(&error, &[&session.prompt]);
                let message = message.trim();
                self.emit(
                    session,
                    json!({
                        "type": "error",
                        "text": if message.is_empty() { "Transcription failed" } else { message },
                    }),
                );
                return;
            }
        };

        if text.trim().is_empty() {
            return self.finish_without_audio(session, "empty_transcript");
        }
        if echoes_prompt(&text, &session.prompt) {
            return self.finish_without_audio(session, "prompt_echo");
        }

        // A short clean transcript is pasted at once: the default tidy pass
        // would not change it. A custom prompt can ask for any change, so it
        // always runs.
        let skip_short_clean = session.refine_enabled
            && session.refine_prompt.trim().is_empty()
            && is_short_clean_transcript(&text);
        if session.refine_enabled && !skip_short_clean {
            debug_log_line(
                "transcriber",
                &format!(
                    "refine begin session_id={:?} text_chars={} custom_prompt_chars={}",
                    session.id,
                    text.chars().count(),
                    session.refine_prompt.chars().count()
                ),
            );
            self.emit(
                session,
                json!({ "type": "partial", "text": "Refining transcript..." }),
            );
            match transcriber.polish(&text, &session.refine_prompt).await {
                Ok(tidied) => {
                    text = tidied;
                    debug_log_line(
                        "transcriber",
                        &format!(
                            "refine done session_id={:?} text_chars={}",
                            session.id,
                            text.chars().count()
                        ),
                    );
                }
                Err(error) => {
                    // The error text can quote the transcript or custom
                    // prompt that was sent, so only its type and codes are
                    // reported.
                    let error = error.describe();
                    debug_log_line(
                        "transcriber",
                        &format!("refine failed session_id={:?} error={error}", session.id),
                    );
                    self.emit(
                        session,
                        json!({
                            "type": "warning",
                            "text": format!("Refinement failed; using the raw transcript ({error})"),
                        }),
                    );
                }
            }
        } else {
            debug_log_line(
                "transcriber",
                &format!(
                    "refine skipped session_id={:?} reason={}",
                    session.id,
                    if skip_short_clean {
                        "short_clean"
                    } else {
                        "disabled"
                    }
                ),
            );
        }

        self.emit(session, json!({ "type": "final", "text": text }));
    }

    fn close_stream(session: &Session) {
        let stream = lock(&session.state).stream.take();
        if let Some(stream) = stream {
            stream.close();
        }
    }

    fn finish_without_audio(&self, session: &Session, reason: &str) {
        debug_log_line(
            "transcriber",
            &format!("finalize session_id={:?} no_audio={reason}", session.id),
        );
        Self::close_stream(session);
        self.emit(
            session,
            json!({ "type": "error", "text": rules().no_audio_message }),
        );
    }
}

/// Runs `work` until it finishes (Some) or the session stops (None).
async fn until_cancelled<T>(
    cancel: &CancellationToken,
    work: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        result = work => Some(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai_api::new_http_client;
    use crate::openai_api::test_server::{Reply, TestServer};
    use crate::audio_rules::max_upload_bytes;
    use crate::realtime::test_server::{completed, delta, RealtimeServer, Script};
    use crate::recording_store::wav_header;
    use crate::wav_file::wav_bytes;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const PROMPT: &str = "Glossary: OpenWhisper, Tauri, Kotlin";
    const DICTATED: &str = "My bank PIN is four four seven one, please remind me at noon.";

    struct Harness {
        sessions: Arc<Sessions>,
        events: Arc<Mutex<Vec<Value>>>,
        dir: PathBuf,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn harness(base_url: &str) -> Harness {
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sink = events.clone();
        let sessions = Sessions::with_base_url(
            new_http_client().unwrap(),
            base_url,
            Arc::new(move |event| sink.lock().unwrap().push(event)),
        );
        let dir = std::env::temp_dir().join(format!("openwhisper-sessions-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Harness {
            sessions: Arc::new(sessions),
            events,
            dir,
        }
    }

    impl Harness {
        /// The (type, text, done) of every event so far.
        fn events(&self) -> Vec<(String, String, bool)> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|event| {
                    (
                        event["event"]["type"].as_str().unwrap().to_string(),
                        event["event"]["text"].as_str().unwrap_or("").to_string(),
                        event["done"].as_bool().unwrap(),
                    )
                })
                .collect()
        }

        fn last(&self) -> (String, String, bool) {
            self.events().pop().expect("an event")
        }

        async fn start(&self, model: &str, refine_enabled: bool, refine_prompt: &str) -> String {
            self.sessions
                .start(SessionSettings {
                    profile_id: "default".into(),
                    openai_api_key: "sk-test".into(),
                    transcription_model: model.into(),
                    prompt: PROMPT.into(),
                    refine_enabled,
                    refine_prompt: refine_prompt.into(),
                    standard_only: false,
                })
                .await
                .unwrap()
        }

        /// A saved 24 kHz 16-bit recording of `seconds` of tone at `amplitude`.
        fn wav(&self, seconds: f64, amplitude: i16) -> PathBuf {
            let path = self.dir.join(format!("{}.wav", Uuid::new_v4()));
            std::fs::write(&path, wav_bytes(&tone(seconds, amplitude), 1, 2, 24_000)).unwrap();
            path
        }

        async fn finalize_saved(&self, session_id: &str, path: PathBuf) {
            self.sessions
                .finalize(
                    session_id,
                    FinalizeAudio::Saved(SavedAudio {
                        audio_path: path,
                        mime_type: "audio/wav".into(),
                        microphone: false,
                        parts_path: None,
                    }),
                )
                .await
                .unwrap()
        }

        /// A whole Standard dictation of one second of speech.
        async fn dictate(&self, refine_enabled: bool, refine_prompt: &str) {
            let id = self
                .start("gpt-transcribe", refine_enabled, refine_prompt)
                .await;
            let path = self.wav(1.0, 6_000);
            self.finalize_saved(&id, path).await
        }
    }

    fn tone(seconds: f64, amplitude: i16) -> Vec<u8> {
        let samples = (seconds * 24_000.0) as usize;
        (0..samples)
            .flat_map(|n| {
                let value = if n % 48 < 24 { amplitude } else { -amplitude };
                value.to_le_bytes()
            })
            .collect()
    }

    #[tokio::test]
    async fn a_dictation_is_transcribed_tidied_and_ends_with_its_final_event() {
        let server = TestServer::start(vec![
            Reply::transcript(DICTATED),
            Reply::response("My bank PIN is 4471. Please remind me at noon."),
        ])
        .await;
        let h = harness(&server.base_url);

        h.dictate(true, "").await;

        assert_eq!(
            h.events(),
            [
                ("partial".into(), "Refining transcript...".into(), false),
                (
                    "final".into(),
                    "My bank PIN is 4471. Please remind me at noon.".into(),
                    true
                ),
            ]
        );
        let received = server.received();
        assert_eq!(received[0].field("prompt").as_deref(), Some(PROMPT));
        assert_eq!(received[0].authorization, "Bearer sk-test");
        let id = h.events.lock().unwrap()[0]["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            h.sessions.stop(&id).is_err(),
            "a finished session is forgotten"
        );
    }

    #[tokio::test]
    async fn short_clean_transcripts_are_pasted_without_a_tidy_call() {
        let server =
            TestServer::start(vec![Reply::transcript("Send me the report by Friday.")]).await;
        let h = harness(&server.base_url);

        h.dictate(true, "").await;

        assert_eq!(
            h.last(),
            ("final".into(), "Send me the report by Friday.".into(), true)
        );
        assert_eq!(server.received().len(), 1);
    }

    #[tokio::test]
    async fn a_custom_refine_prompt_always_gets_the_tidy_call() {
        let server = TestServer::start(vec![
            Reply::transcript("Send me the report by Friday."),
            Reply::response("SEND ME THE REPORT BY FRIDAY."),
        ])
        .await;
        let h = harness(&server.base_url);

        h.dictate(true, "Use capitals.").await;

        assert_eq!(h.last().1, "SEND ME THE REPORT BY FRIDAY.");
        let tidy = server.received()[1].json();
        assert!(tidy["input"][0]["content"]
            .as_str()
            .unwrap()
            .ends_with("Use capitals."));
    }

    #[tokio::test]
    async fn refine_disabled_pastes_the_raw_transcript() {
        let server = TestServer::start(vec![Reply::transcript("um so yeah")]).await;
        let h = harness(&server.base_url);

        h.dictate(false, "").await;

        assert_eq!(h.events(), [("final".into(), "um so yeah".into(), true)]);
    }

    #[tokio::test]
    async fn failed_refinement_keeps_the_transcript_out_of_the_notice() {
        let server = TestServer::start(vec![
            Reply::transcript(DICTATED),
            Reply::error(
                400,
                &format!("Could not process input: {DICTATED}"),
                "invalid_input",
                None,
            ),
        ])
        .await;
        let h = harness(&server.base_url);

        h.dictate(true, "").await;

        let events = h.events();
        let warning = &events[1];
        assert_eq!(warning.0, "warning");
        assert_eq!(
            warning.1,
            "Refinement failed; using the raw transcript (BadRequestError status=400 code=invalid_input)"
        );
        assert_eq!(events[2], ("final".into(), DICTATED.into(), true));
    }

    #[tokio::test]
    async fn a_failed_transcription_notice_leaves_out_the_vocabulary_prompt() {
        let server = TestServer::start(vec![Reply::error(
            400,
            &format!("Prompt rejected: {PROMPT}"),
            "invalid_prompt",
            None,
        )])
        .await;
        let h = harness(&server.base_url);

        h.dictate(true, "").await;

        let (kind, text, done) = h.last();
        assert_eq!(kind, "error");
        assert!(done);
        assert_eq!(text, "Error code: 400 - Prompt rejected: [redacted]");
    }

    #[tokio::test]
    async fn quick_taps_silence_and_empty_files_report_no_audio_without_sending() {
        let server = TestServer::start(vec![]).await;
        let h = harness(&server.base_url);
        let empty = h.dir.join("empty.wav");
        std::fs::write(&empty, b"").unwrap();
        let no_audio = rules().no_audio_message.clone();

        for path in [h.wav(0.1, 6_000), h.wav(1.0, 0), empty] {
            let id = h.start("gpt-transcribe", true, "").await;
            h.finalize_saved(&id, path).await;
            assert_eq!(h.last(), ("error".into(), no_audio.clone(), true));
        }
        // Each ends with its no-audio error alone: no text reaches the window.
        assert_eq!(h.events(), vec![("error".into(), no_audio, true); 3]);
        assert!(server.received().is_empty());
    }

    #[tokio::test]
    async fn a_dictionary_echo_is_not_pasted_but_real_speech_is() {
        let server = TestServer::start(vec![
            Reply::transcript("OpenWhisper, Tauri, Kotlin."),
            Reply::transcript("Kotlin."),
        ])
        .await;
        let h = harness(&server.base_url);

        h.dictate(true, "").await;
        assert_eq!(
            h.last(),
            ("error".into(), rules().no_audio_message.clone(), true)
        );
        h.dictate(true, "").await;
        assert_eq!(h.last(), ("final".into(), "Kotlin.".into(), true));
    }

    #[tokio::test]
    async fn long_recordings_report_their_progress() {
        // Every part of a long recording is checked by src/transcriber.rs;
        // here its progress reaches the window.
        let server =
            TestServer::start(vec![Reply::transcript("One."), Reply::transcript("Two.")]).await;
        let h = harness(&server.base_url);
        let id = h.start("gpt-transcribe", false, "").await;
        // A saved WAV too large for one request, with speech at the start
        // of each part. Sparse, so it costs no disk space.
        let path = h.dir.join("long.wav");
        let pcm_bytes = max_upload_bytes() + 48_000;
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&wav_header(pcm_bytes as u32, 1, 2, 24_000))
            .unwrap();
        file.set_len(44 + pcm_bytes).unwrap();
        for offset in [0, pcm_bytes - 48_000] {
            file.seek(SeekFrom::Start(44 + offset)).unwrap();
            file.write_all(&tone(1.0, 6_000)).unwrap();
        }
        drop(file);

        h.sessions
            .finalize(
                &id,
                FinalizeAudio::Saved(SavedAudio {
                    audio_path: path,
                    mime_type: "audio/wav".into(),
                    microphone: false,
                    parts_path: Some(h.dir.join("parts.json")),
                }),
            )
            .await
            .unwrap();

        assert_eq!(
            h.events(),
            [
                (
                    "partial".into(),
                    "Transcribing part 1 of 2...".into(),
                    false
                ),
                (
                    "partial".into(),
                    "Transcribing part 2 of 2...".into(),
                    false
                ),
                ("final".into(), "One. Two.".into(), true),
            ]
        );
        assert!(h.dir.join("parts.json").exists());
    }

    #[tokio::test]
    async fn cancel_answers_while_a_long_transcription_is_still_running() {
        let server = TestServer::start(vec![
            Reply::transcript("Too late.").after(Duration::from_secs(20))
        ])
        .await;
        let h = harness(&server.base_url);
        let id = h.start("gpt-transcribe", true, "").await;
        let path = h.wav(1.0, 6_000);
        let sessions = h.sessions.clone();
        let finalize_id = id.clone();
        let running = tokio::spawn(async move {
            sessions
                .finalize(
                    &finalize_id,
                    FinalizeAudio::Saved(SavedAudio {
                        audio_path: path,
                        mime_type: "audio/wav".into(),
                        microphone: false,
                        parts_path: None,
                    }),
                )
                .await
        });
        while server.received().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let started = Instant::now();
        h.sessions.stop(&id).unwrap();
        running.await.unwrap().unwrap();

        // The finalize returned without waiting for the 20-second reply, and
        // sent no event.
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(h.events().is_empty());
        assert_eq!(
            h.sessions.stop(&id).unwrap_err(),
            "bad_request: Unknown session_id"
        );
    }

    #[tokio::test]
    async fn cancel_after_transcription_skips_tidying() {
        let server = TestServer::start(vec![
            Reply::transcript(DICTATED),
            Reply::response("Too late.").after(Duration::from_secs(20)),
        ])
        .await;
        let h = harness(&server.base_url);
        let id = h.start("gpt-transcribe", true, "").await;
        let path = h.wav(1.0, 6_000);
        let sessions = h.sessions.clone();
        let finalize_id = id.clone();
        let running = tokio::spawn(async move {
            sessions
                .finalize(
                    &finalize_id,
                    FinalizeAudio::Saved(SavedAudio {
                        audio_path: path,
                        mime_type: "audio/wav".into(),
                        microphone: false,
                        parts_path: None,
                    }),
                )
                .await
        });
        while server.received().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        h.sessions.stop(&id).unwrap();

        running.await.unwrap().unwrap();
        assert_eq!(
            h.events(),
            [("partial".into(), "Refining transcript...".into(), false)]
        );
    }

    #[tokio::test]
    async fn a_failed_session_is_forgotten() {
        // A server error is tried three times before the dictation fails.
        let boom = Reply::error(500, "boom", "server_error", None);
        let server = TestServer::start(vec![boom.clone(), boom.clone(), boom]).await;
        let h = harness(&server.base_url);
        let id = h.start("gpt-transcribe", true, "").await;
        let path = h.wav(1.0, 6_000);

        h.finalize_saved(&id, path).await;

        assert_eq!(
            h.last(),
            ("error".into(), "Error code: 500 - boom".into(), true)
        );
        assert_eq!(
            h.sessions.stop(&id).unwrap_err(),
            "bad_request: Unknown session_id"
        );
        assert_eq!(
            h.sessions
                .finalize(
                    &id,
                    FinalizeAudio::Live {
                        mime_type: "audio/pcm;rate=24000".into()
                    }
                )
                .await
                .unwrap_err(),
            "bad_request: Unknown session_id"
        );
    }

    #[tokio::test]
    async fn new_settings_apply_to_the_next_dictation_only() {
        let server =
            TestServer::start(vec![Reply::transcript("One."), Reply::transcript("Two.")]).await;
        let h = harness(&server.base_url);
        let first = h.start("gpt-transcribe", false, "").await;
        let second = h
            .sessions
            .start(SessionSettings {
                openai_api_key: "sk-new".into(),
                transcription_model: "gpt-4o-transcribe".into(),
                ..SessionSettings::default()
            })
            .await
            .unwrap();

        h.finalize_saved(&first, h.wav(1.0, 6_000)).await;
        h.finalize_saved(&second, h.wav(1.0, 6_000)).await;

        let received = server.received();
        assert_eq!(received[0].authorization, "Bearer sk-test");
        assert_eq!(received[1].authorization, "Bearer sk-new");
        // A model saved before gpt-transcribe moves to its replacement.
        assert_eq!(
            received[1].field("model").as_deref(),
            Some("gpt-transcribe")
        );
    }

    #[tokio::test]
    async fn an_unknown_model_is_refused() {
        let h = harness("http://127.0.0.1:9/v1");
        let error = h
            .sessions
            .start(SessionSettings {
                openai_api_key: "sk-secret-key".into(),
                transcription_model: "whisper-1".into(),
                ..SessionSettings::default()
            })
            .await
            .unwrap_err();

        assert_eq!(
            error,
            "bad_request: Unsupported transcription model: whisper-1"
        );
    }

    #[tokio::test]
    async fn a_standard_session_refuses_audio_chunks() {
        let h = harness("http://127.0.0.1:9/v1");
        let id = h.start("gpt-transcribe", true, "").await;

        assert_eq!(
            h.sessions.append(&id, &[1, 2]).await.unwrap_err(),
            "bad_request: Only Live sessions take audio chunks"
        );
    }

    #[tokio::test]
    async fn live_audio_is_forwarded_and_deltas_arrive_before_finalize() {
        let server = RealtimeServer::start(vec![Script {
            on_connect: vec![],
            on_commit: vec![delta("hello"), completed("Hello world.")],
            ..Script::default()
        }])
        .await;
        let h = harness(&server.base_url);
        let id = h.start("gpt-live-transcribe", false, "").await;

        h.sessions.append(&id, &tone(0.5, 6_000)).await.unwrap();
        h.sessions
            .finalize(
                &id,
                FinalizeAudio::Live {
                    mime_type: "audio/pcm;rate=24000".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            h.events(),
            [
                ("partial".into(), "hello".into(), false),
                ("final".into(), "Hello world.".into(), true),
            ]
        );
        assert_eq!(
            server.event_types(),
            [[
                "session.update",
                "input_audio_buffer.append",
                "input_audio_buffer.commit"
            ]]
        );
    }

    #[tokio::test]
    async fn long_live_audio_moves_to_a_new_connection_and_joins_the_text() {
        let server = RealtimeServer::start(vec![
            Script {
                on_commit: vec![completed("First stretch.")],
                ..Script::default()
            },
            Script {
                on_commit: vec![completed("Second stretch.")],
                ..Script::default()
            },
        ])
        .await;
        let mut h = harness(&server.base_url);
        Arc::get_mut(&mut h.sessions).unwrap().live_rotate_bytes = 30_000;
        let id = h.start("gpt-live-transcribe", false, "").await;

        h.sessions.append(&id, &tone(0.5, 6_000)).await.unwrap();
        h.sessions.append(&id, &tone(0.5, 6_000)).await.unwrap();
        h.sessions
            .finalize(
                &id,
                FinalizeAudio::Live {
                    mime_type: "audio/pcm;rate=24000".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            h.events(),
            [(
                "final".into(),
                "First stretch. Second stretch.".into(),
                true
            )]
        );
        assert_eq!(server.event_types().len(), 2);
    }

    #[tokio::test]
    async fn stop_during_a_live_rotation_opens_no_new_connection() {
        // The first connection never answers its commit.
        let server = RealtimeServer::start(vec![Script {
            hang_on_commit: true,
            ..Script::default()
        }])
        .await;
        let mut h = harness(&server.base_url);
        Arc::get_mut(&mut h.sessions).unwrap().live_rotate_bytes = 30_000;
        let id = h.start("gpt-live-transcribe", false, "").await;
        h.sessions.append(&id, &tone(0.5, 6_000)).await.unwrap();
        let sessions = h.sessions.clone();
        let append_id = id.clone();
        let rotating =
            tokio::spawn(async move { sessions.append(&append_id, &tone(0.5, 6_000)).await });
        while !server.event_types().first().is_some_and(|events| {
            events
                .iter()
                .any(|kind| kind == "input_audio_buffer.commit")
        }) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let _ = h.sessions.stop(&id);
        let rotated = rotating.await.unwrap();

        assert!(rotated.is_err());
        assert_eq!(server.connections.lock().unwrap().len(), 1);
        assert!(h.events().iter().all(|(kind, _, _)| kind == "partial"));
    }

    #[tokio::test]
    async fn silent_or_too_short_live_audio_reports_no_audio() {
        let server = RealtimeServer::start(vec![Script::default(), Script::default()]).await;
        let h = harness(&server.base_url);

        for audio in [tone(1.0, 0), tone(0.1, 6_000)] {
            let id = h.start("gpt-live-transcribe", false, "").await;
            h.sessions.append(&id, &audio).await.unwrap();
            h.sessions
                .finalize(
                    &id,
                    FinalizeAudio::Live {
                        mime_type: "audio/pcm;rate=24000".into(),
                    },
                )
                .await
                .unwrap();
            assert_eq!(
                h.last(),
                ("error".into(), rules().no_audio_message.clone(), true)
            );
        }
    }

    #[tokio::test]
    async fn a_chunk_that_arrives_after_stop_is_refused() {
        let server = RealtimeServer::start(vec![Script::default()]).await;
        let h = harness(&server.base_url);
        let id = h.start("gpt-live-transcribe", false, "").await;

        h.sessions.stop(&id).unwrap();

        assert_eq!(
            h.sessions.append(&id, &[1, 2]).await.unwrap_err(),
            "bad_request: Unknown session_id"
        );
    }

    #[tokio::test]
    async fn a_live_session_given_its_saved_file_is_transcribed_with_standard() {
        // The Live connection opens on the Realtime stand-in; the saved file
        // then goes to the HTTP stand-in.
        let realtime = RealtimeServer::start(vec![Script::default()]).await;
        let server = TestServer::start(vec![Reply::transcript(DICTATED)]).await;
        let mut h = harness(&realtime.base_url);
        let id = h.start("gpt-live-transcribe", false, "").await;
        Arc::get_mut(&mut h.sessions).unwrap().base_url = server.base_url.clone();
        let path = h.wav(1.0, 6_000);

        h.finalize_saved(&id, path).await;

        assert_eq!(h.events(), [("final".into(), DICTATED.into(), true)]);
        let received = server.received();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0].field("model").as_deref(),
            Some("gpt-transcribe")
        );
        assert_eq!(realtime.connections.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_standard_only_live_session_opens_no_connection() {
        // A Live connection to the HTTP stand-in would fail the start.
        let server = TestServer::start(vec![Reply::transcript(DICTATED)]).await;
        let h = harness(&server.base_url);
        let id = h
            .sessions
            .start(SessionSettings {
                openai_api_key: "sk-test".into(),
                transcription_model: "gpt-live-transcribe".into(),
                standard_only: true,
                ..SessionSettings::default()
            })
            .await
            .unwrap();
        assert_eq!(
            h.sessions.append(&id, &[1, 2]).await.unwrap_err(),
            "bad_request: Only Live sessions take audio chunks"
        );
        let path = h.wav(1.0, 6_000);

        h.finalize_saved(&id, path).await;

        assert_eq!(h.last(), ("final".into(), DICTATED.into(), true));
        assert_eq!(
            server.received()[0].field("model").as_deref(),
            Some("gpt-transcribe")
        );
    }

    #[test]
    fn private_text_is_removed_from_errors() {
        assert_eq!(
            without_private_text(
                "bad prompt: Glossary: Zoon and Zoon",
                &["  Glossary: Zoon ", "Zoon"]
            ),
            "bad prompt: [redacted] and [redacted]"
        );
        assert_eq!(without_private_text("an error", &["an"]), "an error");
    }
}
