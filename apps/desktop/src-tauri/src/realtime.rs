//! Live transcription: one OpenAI Realtime websocket per stretch of a Live
//! recording. Audio is appended while the user speaks, the model's words
//! come back as deltas, and a commit at the end asks for the finished text.
//!
//! This file needs no Tauri, so its tests run in `core-tests`.

use crate::openai_api::ensure_crypto_provider;
use crate::recording_store::MICROPHONE_SAMPLE_RATE;
use crate::sync::lock;
use base64::Engine;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// How long a finished stretch may take to arrive after its commit.
pub const FINAL_TIMEOUT: Duration = Duration::from_secs(120);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub type DeltaHandler = Arc<dyn Fn(String) + Send + Sync>;

/// The finished text or why there is none; empty until the service answers.
type Outcome = Option<Result<String, String>>;

/// The websocket address for an API base address
/// (`https://api.openai.com/v1` becomes `wss://api.openai.com/v1/realtime`).
pub fn realtime_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    // The dedicated transcription transport is chosen by intent. It refuses
    // a conversational model in the address; the transcription model is
    // named in session.update.
    format!("{base}/realtime?intent=transcription")
}

/// A persistent Realtime connection that takes PCM while recording.
pub struct RealtimeStream {
    sink: Arc<AsyncMutex<SplitSink<Socket, Message>>>,
    outcome: watch::Receiver<Outcome>,
    closed: AtomicBool,
    finish: watch::Sender<Outcome>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl RealtimeStream {
    /// Opens the connection and starts a transcription session with
    /// `model`. `on_delta` hears each piece of text as it arrives.
    pub async fn connect(
        base_url: &str,
        api_key: &str,
        model: &str,
        on_delta: DeltaHandler,
    ) -> Result<Self, String> {
        ensure_crypto_provider();
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err("Add your OpenAI API key in settings".into());
        }
        let mut request = realtime_url(base_url)
            .into_client_request()
            .map_err(|error| format!("Realtime address is not valid: {error}"))?;
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|_| "The API key holds characters a header cannot carry".to_string())?,
        );
        let (socket, _) =
            tokio::time::timeout(OPEN_TIMEOUT, tokio_tungstenite::connect_async(request))
                .await
                .map_err(|_| "Realtime connection timed out".to_string())?
                .map_err(|error| format!("Realtime connection failed: {error}"))?;
        let (mut sink, events) = socket.split();
        let update = json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {
                    "input": {
                        "format": { "type": "audio/pcm", "rate": MICROPHONE_SAMPLE_RATE },
                        "transcription": { "model": model, "delay": "minimal" },
                        "turn_detection": null,
                    }
                },
            },
        });
        // Bounded, as a peer that stops reading would otherwise hold it for ever.
        tokio::time::timeout(OPEN_TIMEOUT, sink.send(Message::text(update.to_string())))
            .await
            .map_err(|_| "Realtime connection timed out".to_string())?
            .map_err(|error| format!("Realtime connection failed: {error}"))?;

        let (finish, outcome) = watch::channel(None);
        let reader_finish = finish.clone();
        let reader = tokio::spawn(async move {
            let result = read_events(events, on_delta).await;
            reader_finish.send_if_modified(|outcome| {
                if outcome.is_some() {
                    return false;
                }
                *outcome = Some(result);
                true
            });
        });
        Ok(Self {
            sink: Arc::new(AsyncMutex::new(sink)),
            outcome,
            closed: AtomicBool::new(false),
            finish,
            reader: Mutex::new(Some(reader)),
        })
    }

    pub async fn append(&self, audio: &[u8]) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("Realtime transcription session is closed".into());
        }
        if audio.is_empty() {
            return Ok(());
        }
        let event = json!({
            "type": "input_audio_buffer.append",
            "audio": base64::engine::general_purpose::STANDARD.encode(audio),
        });
        self.send(event).await
    }

    /// Asks for the finished text of everything appended, waits for it and
    /// closes the connection.
    pub async fn finalize(&self, timeout: Duration) -> Result<String, String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("Realtime transcription session is closed".into());
        }
        // The commit's send is bounded too: a peer that stops reading leaves it
        // waiting on a full socket, and the saved-audio fallback never starts.
        let commit = tokio::time::timeout(
            timeout,
            self.send(json!({ "type": "input_audio_buffer.commit" })),
        )
        .await
        .unwrap_or_else(|_| Err("Realtime transcription timed out".into()));
        let result = match commit {
            Ok(()) => {
                let mut outcome = self.outcome.clone();
                let waited =
                    match tokio::time::timeout(timeout, outcome.wait_for(Option::is_some)).await {
                        Ok(Ok(done)) => done
                            .clone()
                            .unwrap_or_else(|| Err("Realtime transcription failed".into())),
                        Ok(Err(_)) => Err("Realtime transcription failed".into()),
                        Err(_) => Err("Realtime transcription timed out".into()),
                    };
                waited
            }
            Err(error) => Err(error),
        };
        self.close();
        result
    }

    /// Ends the connection. A `finalize` still waiting stops at once.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.finish.send_if_modified(|outcome| {
            if outcome.is_some() {
                return false;
            }
            *outcome = Some(Err("Realtime transcription session is closed".into()));
            true
        });
        if let Some(reader) = lock(&self.reader).take() {
            reader.abort();
        }
        // Closing politely needs the sink, which a send may still hold; it
        // is done in the background so a cancel never waits for it. Without
        // a runtime the connection simply drops with this stream.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let sink = self.sink.clone();
            runtime.spawn(async move {
                let _ = sink.lock().await.close().await;
            });
        }
    }

    async fn send(&self, event: Value) -> Result<(), String> {
        self.sink
            .lock()
            .await
            .send(Message::text(event.to_string()))
            .await
            .map_err(|error| format!("Realtime connection failed: {error}"))
    }
}

impl Drop for RealtimeStream {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads the service's events until the finished text, a failure or the end
/// of the connection.
async fn read_events(
    mut events: futures_util::stream::SplitStream<Socket>,
    on_delta: DeltaHandler,
) -> Result<String, String> {
    let mut deltas = String::new();
    while let Some(message) = events.next().await {
        let text = match message {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(error) => return Err(format!("Realtime connection failed: {error}")),
        };
        let Ok(event) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let message = |fallback: &str| {
            event["error"]["message"]
                .as_str()
                .unwrap_or(fallback)
                .to_string()
        };
        match event["type"].as_str().unwrap_or("") {
            "conversation.item.input_audio_transcription.delta" => {
                if let Some(delta) = event["delta"].as_str().filter(|delta| !delta.is_empty()) {
                    deltas.push_str(delta);
                    on_delta(delta.to_string());
                }
            }
            "conversation.item.input_audio_transcription.completed" => {
                let transcript = event["transcript"].as_str().unwrap_or("");
                return Ok(if transcript.is_empty() {
                    deltas
                } else {
                    transcript.to_string()
                });
            }
            "conversation.item.input_audio_transcription.failed" => {
                return Err(message("Realtime transcription failed"));
            }
            "error" => return Err(message("Realtime API error")),
            _ => {}
        }
    }
    // A connection the service closed without the finished text has lost it.
    Err("The live transcription connection closed before the transcript arrived".into())
}

#[cfg(test)]
pub mod test_server {
    //! A local stand-in for the Realtime websocket. Each connection gets the
    //! next script: the events it sends once a commit arrives (or at once
    //! for `on_connect`), and what it was sent is kept.

    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    use tokio_tungstenite::tungstenite::Message;

    #[derive(Clone, Default)]
    pub struct Script {
        /// Sent as soon as the session is updated.
        pub on_connect: Vec<Value>,
        /// Sent when the audio is committed; then the server closes.
        pub on_commit: Vec<Value>,
        /// Never answers the commit and keeps the connection open.
        pub hang_on_commit: bool,
    }

    #[derive(Default)]
    pub struct Connection {
        pub uri: String,
        pub authorization: String,
        pub events: Vec<Value>,
    }

    #[derive(Clone)]
    pub struct RealtimeServer {
        pub base_url: String,
        pub connections: Arc<Mutex<Vec<Connection>>>,
    }

    impl RealtimeServer {
        pub async fn start(scripts: Vec<Script>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
            let scripts = Arc::new(Mutex::new(VecDeque::from(scripts)));
            let connections: Arc<Mutex<Vec<Connection>>> = Arc::default();
            let accepted = connections.clone();
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let script = scripts.lock().unwrap().pop_front().unwrap_or_default();
                    let connections = accepted.clone();
                    tokio::spawn(async move {
                        let mut seen = Connection::default();
                        let socket = tokio_tungstenite::accept_hdr_async(
                            socket,
                            |request: &Request, response: Response| {
                                seen.uri = request.uri().to_string();
                                seen.authorization = request
                                    .headers()
                                    .get("authorization")
                                    .and_then(|value| value.to_str().ok())
                                    .unwrap_or("")
                                    .to_string();
                                Ok(response)
                            },
                        )
                        .await
                        .unwrap();
                        let index = {
                            let mut all = connections.lock().unwrap();
                            all.push(seen);
                            all.len() - 1
                        };
                        let (mut sink, mut stream) = socket.split();
                        while let Some(Ok(message)) = stream.next().await {
                            let Message::Text(text) = message else {
                                continue;
                            };
                            let event: Value = serde_json::from_str(text.as_str()).unwrap();
                            let kind = event["type"].as_str().unwrap_or("").to_string();
                            connections.lock().unwrap()[index].events.push(event);
                            if kind == "input_audio_buffer.commit" && script.hang_on_commit {
                                continue;
                            }
                            let replies = match kind.as_str() {
                                "session.update" => &script.on_connect,
                                "input_audio_buffer.commit" => &script.on_commit,
                                _ => continue,
                            };
                            for reply in replies {
                                let _ = sink.send(Message::text(reply.to_string())).await;
                            }
                            if kind == "input_audio_buffer.commit" {
                                let _ = sink.close().await;
                                return;
                            }
                        }
                    });
                }
            });
            Self {
                base_url,
                connections,
            }
        }

        /// The kinds of event each connection was sent.
        pub fn event_types(&self) -> Vec<Vec<String>> {
            self.connections
                .lock()
                .unwrap()
                .iter()
                .map(|connection| {
                    connection
                        .events
                        .iter()
                        .map(|event| event["type"].as_str().unwrap_or("").to_string())
                        .collect()
                })
                .collect()
        }
    }

    pub fn delta(text: &str) -> Value {
        serde_json::json!({"type": "conversation.item.input_audio_transcription.delta", "delta": text})
    }

    pub fn completed(text: &str) -> Value {
        serde_json::json!({"type": "conversation.item.input_audio_transcription.completed", "transcript": text})
    }

    pub fn failed(message: &str) -> Value {
        serde_json::json!({"type": "conversation.item.input_audio_transcription.failed", "error": {"message": message}})
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{completed, delta, failed, RealtimeServer, Script};
    use super::*;

    fn collecting() -> (DeltaHandler, Arc<Mutex<Vec<String>>>) {
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = seen.clone();
        (Arc::new(move |text| sink.lock().unwrap().push(text)), seen)
    }

    #[test]
    fn the_address_selects_the_transcription_transport() {
        assert_eq!(
            realtime_url("https://api.openai.com/v1/"),
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
        assert_eq!(
            realtime_url("http://127.0.0.1:9/v1"),
            "ws://127.0.0.1:9/v1/realtime?intent=transcription"
        );
    }

    #[tokio::test]
    async fn the_stream_names_the_model_in_the_session_and_passes_deltas_on() {
        let server = RealtimeServer::start(vec![Script {
            on_connect: vec![delta("hello")],
            on_commit: vec![completed("hello world")],
            ..Script::default()
        }])
        .await;
        let (on_delta, deltas) = collecting();

        let stream =
            RealtimeStream::connect(&server.base_url, "sk-test", "gpt-live-transcribe", on_delta)
                .await
                .unwrap();
        stream.append(&[1, 2]).await.unwrap();
        let text = stream.finalize(FINAL_TIMEOUT).await.unwrap();

        assert_eq!(text, "hello world");
        assert_eq!(*deltas.lock().unwrap(), ["hello"]);
        let connections = server.connections.lock().unwrap();
        let connection = &connections[0];
        assert_eq!(connection.uri, "/v1/realtime?intent=transcription");
        assert_eq!(connection.authorization, "Bearer sk-test");
        let session = &connection.events[0]["session"];
        assert_eq!(connection.events[0]["type"], "session.update");
        assert_eq!(session["type"], "transcription");
        assert!(session.get("output_modalities").is_none());
        assert_eq!(
            session["audio"]["input"]["transcription"]["model"],
            "gpt-live-transcribe"
        );
        assert_eq!(session["audio"]["input"]["format"]["rate"], 24_000);
        assert!(session["audio"]["input"]["turn_detection"].is_null());
        assert_eq!(connection.events[1]["type"], "input_audio_buffer.append");
        assert_eq!(connection.events[1]["audio"], "AQI=");
        assert_eq!(connection.events[2]["type"], "input_audio_buffer.commit");
    }

    #[tokio::test]
    async fn without_a_finished_transcript_the_deltas_are_the_text() {
        let server = RealtimeServer::start(vec![Script {
            on_connect: vec![],
            on_commit: vec![delta("one "), delta("two"), completed("")],
            ..Script::default()
        }])
        .await;
        let (on_delta, _) = collecting();
        let stream = RealtimeStream::connect(&server.base_url, "sk-test", "m", on_delta)
            .await
            .unwrap();

        assert_eq!(stream.finalize(FINAL_TIMEOUT).await.unwrap(), "one two");
    }

    #[tokio::test]
    async fn a_failed_transcription_reports_the_service_message() {
        let server = RealtimeServer::start(vec![Script {
            on_connect: vec![],
            on_commit: vec![failed("Audio could not be decoded")],
            ..Script::default()
        }])
        .await;
        let (on_delta, _) = collecting();
        let stream = RealtimeStream::connect(&server.base_url, "sk-test", "m", on_delta)
            .await
            .unwrap();

        assert_eq!(
            stream.finalize(FINAL_TIMEOUT).await.unwrap_err(),
            "Audio could not be decoded"
        );
    }

    #[tokio::test]
    async fn a_clean_close_without_a_transcript_is_an_error_at_once() {
        let server = RealtimeServer::start(vec![Script::default()]).await;
        let (on_delta, _) = collecting();
        let stream = RealtimeStream::connect(&server.base_url, "sk-test", "m", on_delta)
            .await
            .unwrap();

        let started = std::time::Instant::now();
        let error = stream.finalize(FINAL_TIMEOUT).await.unwrap_err();

        assert!(
            error.contains("closed before the transcript arrived"),
            "{error}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn close_wakes_a_finalize_that_is_waiting() {
        // This server never answers the commit.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            while socket.next().await.is_some() {}
        });
        let (on_delta, _) = collecting();
        let stream = Arc::new(
            RealtimeStream::connect(&base_url, "sk-test", "m", on_delta)
                .await
                .unwrap(),
        );
        let closer = stream.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            closer.close();
        });

        let error = stream.finalize(FINAL_TIMEOUT).await.unwrap_err();

        assert!(error.contains("closed"), "{error}");
        assert!(stream.append(&[1, 2]).await.is_err());
    }

    #[tokio::test]
    async fn a_commit_that_cannot_be_sent_times_out() {
        let server = RealtimeServer::start(vec![Script::default()]).await;
        let (on_delta, _) = collecting();
        let stream = RealtimeStream::connect(&server.base_url, "sk-test", "m", on_delta)
            .await
            .unwrap();
        // Holds the socket, as a send stuck on a peer that stopped reading does.
        let _stuck = stream.sink.lock().await;

        let error = tokio::time::timeout(
            Duration::from_secs(10),
            stream.finalize(Duration::from_millis(100)),
        )
        .await
        .expect("finalize returned")
        .unwrap_err();

        assert_eq!(error, "Realtime transcription timed out");
    }

    #[tokio::test]
    async fn no_key_opens_no_connection() {
        let (on_delta, _) = collecting();
        let error = RealtimeStream::connect("http://127.0.0.1:9/v1", " ", "m", on_delta)
            .await
            .err()
            .unwrap();
        assert_eq!(error, "Add your OpenAI API key in settings");
    }
}
