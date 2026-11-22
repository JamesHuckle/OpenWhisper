//! The two OpenAI HTTP calls the desktop makes: transcribe one audio file
//! (`/v1/audio/transcriptions`) and tidy a transcript
//! (`/v1/responses`). The Live websocket is in src/realtime.rs.
//!
//! A request that fails for a passing reason (the connection dropped, a rate
//! limit, a 500/502/503/504) is tried twice more, after 1 s and then 3 s; any
//! other failure is reported at once. A cancelled dictation drops the call,
//! which ends any wait.
//!
//! Errors keep the API's message for the user, but `describe` (used in logs
//! and warnings) gives only the error type, HTTP status and error code: an
//! API message can quote the request, which holds the user's dictated words
//! and prompts.
//!
//! This file needs no Tauri, so its tests run in the `core` crate.

use crate::debug_log::debug_log_line;
use crate::recording_store::extension_for_mime_type;
use bytes::Bytes;
use reqwest::{multipart, Client, Response};
use serde_json::{json, Value};
use std::fmt;
use std::future::Future;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole of one transcription request is a minute plus a second for every
/// 50 kB uploaded (about 400 kbit/s), so a stalled short dictation fails
/// within about a minute while a full-size part has nine; never more than this.
/// The answer is one JSON body sent when the transcript is finished, so this
/// is also the only limit on a stall after the upload: for a full-size part
/// a stalled answer is noticed only when its nine minutes run out. There is
/// no shorter read timeout, because reqwest counts that from the start of the
/// upload, and a long part on a slow connection can take minutes to send.
const MAX_TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(40 * 60);
const TRANSCRIBE_BASE_TIMEOUT: Duration = Duration::from_secs(60);
const UPLOAD_BYTES_PER_SECOND: u64 = 50_000;
/// The whole of one tidy request.
const POLISH_TIMEOUT: Duration = Duration::from_secs(90);
/// The waits before the second and third tries. Tests use short ones.
const RETRY_DELAYS: [Duration; 2] = if cfg!(test) {
    [Duration::from_millis(200), Duration::from_millis(400)]
} else {
    [Duration::from_secs(1), Duration::from_secs(3)]
};
/// The longest a rate limit's `Retry-After` may make a dictation wait.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

/// The time limit of one transcription request uploading `upload_bytes`.
fn transcribe_timeout(upload_bytes: usize) -> Duration {
    let upload = Duration::from_secs(upload_bytes as u64 / UPLOAD_BYTES_PER_SECOND);
    (TRANSCRIBE_BASE_TIMEOUT + upload).min(MAX_TRANSCRIBE_TIMEOUT)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The service answered with an error status.
    Status {
        status: u16,
        code: Option<String>,
        param: Option<String>,
        message: String,
    },
    /// The request could not be sent or its answer could not be read.
    Connection(String),
    Timeout,
    MissingKey,
    /// The answer was not what the API documents.
    Unexpected(String),
}

impl ApiError {
    /// HTTP 400: the service could not use what was sent.
    pub fn is_bad_request(&self) -> bool {
        matches!(self, ApiError::Status { status: 400, .. })
    }

    /// A failure that may pass within seconds: the connection dropped, a
    /// rate limit or a server error. A spent quota does not pass.
    pub fn is_temporary(&self) -> bool {
        match self {
            ApiError::Connection(_) => true,
            ApiError::Status {
                status: 429, code, ..
            } => code.as_deref() != Some("insufficient_quota"),
            ApiError::Status { status, .. } => matches!(status, 500 | 502 | 503 | 504),
            ApiError::Timeout | ApiError::MissingKey | ApiError::Unexpected(_) => false,
        }
    }

    pub fn param(&self) -> Option<&str> {
        match self {
            ApiError::Status { param, .. } => param.as_deref(),
            _ => None,
        }
    }

    /// The error's type, HTTP status and API error code, without its message.
    pub fn describe(&self) -> String {
        match self {
            ApiError::Status { status, code, .. } => {
                let mut parts = vec![
                    status_error_name(*status).to_string(),
                    format!("status={status}"),
                ];
                if let Some(code) = code.as_deref().filter(|code| is_plain_code(code)) {
                    parts.push(format!("code={code}"));
                }
                parts.join(" ")
            }
            ApiError::Connection(_) => "APIConnectionError".into(),
            ApiError::Timeout => "APITimeoutError".into(),
            ApiError::MissingKey => "MissingKeyError".into(),
            ApiError::Unexpected(_) => "APIResponseError".into(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Status {
                status, message, ..
            } => write!(f, "Error code: {status} - {message}"),
            ApiError::Connection(detail) => write!(f, "Connection error: {detail}"),
            ApiError::Timeout => write!(f, "Request timed out."),
            ApiError::MissingKey => write!(f, "Add your OpenAI API key in settings"),
            ApiError::Unexpected(detail) => write!(f, "Unexpected answer from OpenAI: {detail}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// The names the OpenAI SDKs give each status, so logs read as before.
fn status_error_name(status: u16) -> &'static str {
    match status {
        400 => "BadRequestError",
        401 => "AuthenticationError",
        403 => "PermissionDeniedError",
        404 => "NotFoundError",
        409 => "ConflictError",
        422 => "UnprocessableEntityError",
        429 => "RateLimitError",
        500.. => "InternalServerError",
        _ => "APIStatusError",
    }
}

fn is_plain_code(code: &str) -> bool {
    (1..=64).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// What the transcriber needs from OpenAI. The app uses [`HttpApi`]; tests
/// use stand-ins.
pub trait OpenAiApi: Send + Sync {
    /// The text of one audio file.
    fn transcribe(
        &self,
        audio: Vec<u8>,
        mime_type: &str,
        model: &str,
        prompt: &str,
    ) -> impl Future<Output = Result<String, ApiError>> + Send;

    /// The model's answer to `text` under the developer message `system`.
    fn respond(
        &self,
        model: &str,
        system: &str,
        text: &str,
        reasoning_effort: &str,
    ) -> impl Future<Output = Result<String, ApiError>> + Send;
}

/// rustls is built with ring only (as the updater uses it), so it is chosen
/// once for the whole app before the first connection.
pub fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// One connection pool for every dictation, so connections stay warm. Each
/// request sets its own time limit.
pub fn new_http_client() -> Result<Client, String> {
    ensure_crypto_provider();
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| format!("Could not start the HTTP client: {error}"))
}

/// OpenAI over HTTP with one key.
#[derive(Clone)]
pub struct HttpApi {
    client: Client,
    base_url: String,
    api_key: String,
}

impl HttpApi {
    pub fn new(client: Client, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
        }
    }

    fn key(&self) -> Result<&str, ApiError> {
        if self.api_key.is_empty() {
            Err(ApiError::MissingKey)
        } else {
            Ok(&self.api_key)
        }
    }
}

impl HttpApi {
    /// One try at transcribing `audio`, which every try shares uncopied.
    async fn transcribe_once(
        &self,
        key: &str,
        audio: Bytes,
        mime_type: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String, Failure> {
        let timeout = transcribe_timeout(audio.len());
        let length = audio.len() as u64;
        let file = multipart::Part::stream_with_length(audio, length)
            .file_name(format!(
                "session_audio.{}",
                extension_for_mime_type(mime_type)
            ))
            .mime_str(mime_type)
            .map_err(|_| ApiError::Unexpected(format!("audio type {mime_type}")))?;
        let mut form = multipart::Form::new()
            .text("model", model.to_string())
            .text("response_format", "json")
            .part("file", file);
        if !prompt.is_empty() {
            form = form.text("prompt", prompt.to_string());
        }
        let response = self
            .client
            .post(format!("{}/audio/transcriptions", self.base_url))
            .bearer_auth(key)
            .timeout(timeout)
            .multipart(form)
            .send()
            .await
            .map_err(request_error)?;
        Ok(read_transcript(checked(response).await?).await?)
    }

    /// One try at a Responses API answer to `body`.
    async fn respond_once(&self, key: &str, body: &Value) -> Result<String, Failure> {
        let response = self
            .client
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(key)
            .timeout(POLISH_TIMEOUT)
            .json(body)
            .send()
            .await
            .map_err(request_error)?;
        let answer: Value = checked(response)
            .await?
            .json()
            .await
            .map_err(request_error)?;
        // An answer cut short (its output limit, a content filter) or one
        // that failed holds only part of the text, if any.
        // A status that is not text is not "completed" either.
        let status = match &answer["status"] {
            Value::Null => None,
            Value::String(status) => Some(status.clone()),
            other => Some(other.to_string()),
        };
        match status.as_deref() {
            None | Some("completed") => Ok(responses_text(&answer)),
            Some(status) => Err(ApiError::Unexpected(format!(
                "the response was {status}{}",
                answer["incomplete_details"]["reason"]
                    .as_str()
                    .map(|reason| format!(" ({reason})"))
                    .unwrap_or_default()
            ))
            .into()),
        }
    }
}

impl OpenAiApi for HttpApi {
    async fn transcribe(
        &self,
        audio: Vec<u8>,
        mime_type: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String, ApiError> {
        let key = self.key()?;
        // A part can be 24 MiB; cloning `Bytes` shares it instead of copying.
        let audio = Bytes::from(audio);
        with_retries("transcribe", || {
            self.transcribe_once(key, audio.clone(), mime_type, model, prompt)
        })
        .await
    }

    async fn respond(
        &self,
        model: &str,
        system: &str,
        text: &str,
        reasoning_effort: &str,
    ) -> Result<String, ApiError> {
        let key = self.key()?;
        let body = json!({
            "model": model,
            "input": [
                { "role": "developer", "content": system },
                { "role": "user", "content": text },
            ],
            "reasoning": { "effort": reasoning_effort },
            "text": { "verbosity": "low" },
        });
        with_retries("respond", || self.respond_once(key, &body)).await
    }
}

/// A failed try, and how long a rate limit asked us to wait before the next.
struct Failure {
    error: ApiError,
    retry_after: Option<Duration>,
}

impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}

/// Runs `attempt` until it succeeds, fails for a reason that will not pass,
/// or has been tried three times. Dropping the future ends any wait.
async fn with_retries<T, F, Fut>(name: &str, mut attempt: F) -> Result<T, ApiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    let mut delays = RETRY_DELAYS.into_iter();
    loop {
        let failure = match attempt().await {
            Ok(answer) => return Ok(answer),
            Err(failure) => failure,
        };
        let Some(delay) = delays.next().filter(|_| failure.error.is_temporary()) else {
            return Err(failure.error);
        };
        let wait = failure
            .retry_after
            .map_or(delay, |asked| asked.min(MAX_RETRY_AFTER));
        debug_log_line(
            "openai",
            &format!(
                "retry request={name} wait_ms={} error={}",
                wait.as_millis(),
                failure.error.describe()
            ),
        );
        tokio::time::sleep(wait).await;
    }
}

fn request_error(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        ApiError::Timeout
    } else if error.is_decode() && is_json_error(&error) {
        // A whole answer that is not JSON: sending again would not change it.
        ApiError::Unexpected(error.without_url().to_string())
    } else {
        // Including an answer whose download broke off, which reqwest also
        // calls a decode error: the next try may get all of it.
        ApiError::Connection(error.without_url().to_string())
    }
}

/// Whether the error comes from reading JSON, not from reading the body.
fn is_json_error(error: &reqwest::Error) -> bool {
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        if inner.is::<serde_json::Error>() {
            return true;
        }
        source = inner.source();
    }
    false
}

/// The response when its status is a success, or the API's error.
async fn checked(response: Response) -> Result<Response, Failure> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    // Only whole seconds: an HTTP date is left to the usual wait.
    let retry_after = (status.as_u16() == 429)
        .then(|| response.headers().get(reqwest::header::RETRY_AFTER))
        .flatten()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response.text().await.unwrap_or_default();
    let error = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .unwrap_or(Value::Null);
    let text = |name: &str| error.get(name).and_then(Value::as_str).map(str::to_string);
    let message = text("message").unwrap_or_else(|| {
        let body = body.trim();
        if body.is_empty() {
            status
                .canonical_reason()
                .unwrap_or("Request failed")
                .to_string()
        } else {
            body.chars().take(300).collect()
        }
    });
    Err(Failure {
        error: ApiError::Status {
            status: status.as_u16(),
            code: text("code"),
            param: text("param"),
            message,
        },
        retry_after,
    })
}

/// The transcript in a JSON answer `{"text": ...}`. Empty text is a silent
/// recording, not an error.
async fn read_transcript(response: Response) -> Result<String, ApiError> {
    let answer: Value = response.json().await.map_err(request_error)?;
    answer["text"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ApiError::Unexpected("the transcription answer had no text".into()))
}

/// Every output text of a Responses API answer, in order.
fn responses_text(answer: &Value) -> String {
    let mut text = String::new();
    for item in answer["output"].as_array().into_iter().flatten() {
        for content in item["content"].as_array().into_iter().flatten() {
            if let Some(piece) = content["text"].as_str() {
                text.push_str(piece);
            }
        }
    }
    text
}

#[cfg(test)]
pub mod test_server {
    //! A local stand-in for the OpenAI HTTP API: it answers each request
    //! with the next queued reply and keeps what it was sent.

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[derive(Debug, Clone)]
    pub struct Received {
        pub path: String,
        pub authorization: String,
        pub content_type: String,
        pub body: Vec<u8>,
    }

    impl Received {
        /// The multipart form fields: name, file name and value.
        pub fn fields(&self) -> Vec<(String, Option<String>, Vec<u8>)> {
            let boundary = self
                .content_type
                .split("boundary=")
                .nth(1)
                .expect("a multipart boundary")
                .trim_matches('"')
                .to_string();
            let delimiter = format!("--{boundary}").into_bytes();
            split(&self.body, &delimiter)
                .into_iter()
                .filter_map(|section| {
                    let section = section.strip_prefix(b"\r\n")?;
                    let head_end = find(section, b"\r\n\r\n")?;
                    let head = String::from_utf8_lossy(&section[..head_end]).to_string();
                    let mut value = section[head_end + 4..].to_vec();
                    if value.ends_with(b"\r\n") {
                        value.truncate(value.len() - 2);
                    }
                    let attribute = |name: &str| {
                        head.split(&format!("{name}=\""))
                            .nth(1)
                            .and_then(|rest| rest.split('"').next())
                            .map(str::to_string)
                    };
                    Some((attribute("name")?, attribute("filename"), value))
                })
                .collect()
        }

        pub fn field(&self, name: &str) -> Option<String> {
            self.fields()
                .into_iter()
                .find(|(field, _, _)| field == name)
                .map(|(_, _, value)| String::from_utf8_lossy(&value).to_string())
        }

        pub fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).expect("a JSON body")
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn split<'a>(haystack: &'a [u8], delimiter: &[u8]) -> Vec<&'a [u8]> {
        let mut sections = Vec::new();
        let mut rest = haystack;
        while let Some(at) = find(rest, delimiter) {
            sections.push(&rest[..at]);
            rest = &rest[at + delimiter.len()..];
        }
        sections.push(rest);
        sections
    }

    /// One queued answer: a status, a content type and a body, sent after
    /// an optional delay. Status 0 closes the connection without answering.
    #[derive(Clone)]
    pub struct Reply {
        pub status: u16,
        pub content_type: &'static str,
        pub body: String,
        pub delay: Duration,
        /// The `Retry-After` header, in seconds.
        pub retry_after: Option<u64>,
        /// The connection closes before the whole announced body is sent.
        pub cut_short: bool,
    }

    impl Reply {
        /// A transcription answer holding `text`.
        pub fn transcript(text: &str) -> Self {
            Self::json(200, serde_json::json!({ "text": text }))
        }

        /// A Responses API answer holding `text`.
        pub fn response(text: &str) -> Self {
            Self::json(
                200,
                serde_json::json!({
                    "output": [
                        {"type": "reasoning", "content": []},
                        {"type": "message", "content": [{"type": "output_text", "text": text}]},
                    ]
                }),
            )
        }

        /// An API error.
        pub fn error(status: u16, message: &str, code: &str, param: Option<&str>) -> Self {
            Self::json(
                status,
                serde_json::json!({
                    "error": {"message": message, "type": "invalid_request_error", "code": code, "param": param}
                }),
            )
        }

        pub fn json(status: u16, body: serde_json::Value) -> Self {
            Self {
                status,
                content_type: "application/json",
                body: body.to_string(),
                delay: Duration::ZERO,
                retry_after: None,
                cut_short: false,
            }
        }

        /// The connection closes with no answer, as when a network drops.
        pub fn hang_up() -> Self {
            Self::json(0, serde_json::Value::Null)
        }

        pub fn after(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        pub fn retry_after(mut self, seconds: u64) -> Self {
            self.retry_after = Some(seconds);
            self
        }

        /// The answer's head and the start of its body, then the connection
        /// drops, as when a network fails mid-answer.
        pub fn cut_short(mut self) -> Self {
            self.cut_short = true;
            self
        }
    }

    #[derive(Default)]
    struct State {
        replies: VecDeque<Reply>,
        received: Vec<Received>,
    }

    #[derive(Clone)]
    pub struct TestServer {
        pub base_url: String,
        state: Arc<Mutex<State>>,
    }

    impl TestServer {
        pub async fn start(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
            let state = Arc::new(Mutex::new(State {
                replies: replies.into(),
                received: Vec::new(),
            }));
            let accept_state = state.clone();
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    tokio::spawn(serve(socket, accept_state.clone()));
                }
            });
            Self { base_url, state }
        }

        pub fn received(&self) -> Vec<Received> {
            self.state.lock().unwrap().received.clone()
        }

        pub fn unused_replies(&self) -> usize {
            self.state.lock().unwrap().replies.len()
        }
    }

    async fn serve(mut socket: TcpStream, state: Arc<Mutex<State>>) {
        let mut buffer = Vec::new();
        loop {
            // One request: its head, then a body of Content-Length bytes.
            let head_end = loop {
                if let Some(at) = find(&buffer, b"\r\n\r\n") {
                    break at;
                }
                let mut chunk = [0_u8; 65_536];
                match socket.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(count) => buffer.extend_from_slice(&chunk[..count]),
                }
            };
            let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
            let header = |name: &str| {
                head.lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case(name)
                            .then(|| value.trim().to_string())
                    })
                    .unwrap_or_default()
            };
            let length: usize = header("content-length").parse().unwrap_or(0);
            let body_start = head_end + 4;
            while buffer.len() < body_start + length {
                let mut chunk = vec![0_u8; 1 << 20];
                match socket.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(count) => buffer.extend_from_slice(&chunk[..count]),
                }
            }
            let received = Received {
                path: head.split_whitespace().nth(1).unwrap_or("").to_string(),
                authorization: header("authorization"),
                content_type: header("content-type"),
                body: buffer[body_start..body_start + length].to_vec(),
            };
            buffer.drain(..body_start + length);
            let reply = {
                let mut state = state.lock().unwrap();
                state.received.push(received);
                state.replies.pop_front()
            };
            let reply = reply.unwrap_or_else(|| Reply::error(500, "no reply queued", "test", None));
            tokio::time::sleep(reply.delay).await;
            if reply.status == 0 {
                return;
            }
            let retry_after = reply
                .retry_after
                .map(|seconds| format!("Retry-After: {seconds}\r\n"))
                .unwrap_or_default();
            let answer = format!(
                "HTTP/1.1 {} Test\r\nContent-Type: {}\r\nContent-Length: {}\r\n{retry_after}\r\n{}",
                reply.status,
                reply.content_type,
                reply.body.len() + if reply.cut_short { 100 } else { 0 },
                reply.body
            );
            if socket.write_all(answer.as_bytes()).await.is_err() || reply.cut_short {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{Reply, TestServer};
    use super::*;

    fn api(server: &TestServer, key: &str) -> HttpApi {
        HttpApi::new(new_http_client().unwrap(), &server.base_url, key)
    }

    #[tokio::test]
    async fn a_recording_is_sent_with_its_prompt() {
        let server = TestServer::start(vec![Reply::transcript("Hello there.")]).await;

        let text = api(&server, "sk-test")
            .transcribe(
                b"RIFFaudio".to_vec(),
                "audio/wav",
                "gpt-transcribe",
                "Glossary: Zoon",
            )
            .await
            .unwrap();

        let sent = &server.received()[0];
        assert_eq!(text, "Hello there.");
        assert_eq!(sent.path, "/v1/audio/transcriptions");
        assert_eq!(sent.authorization, "Bearer sk-test");
        assert_eq!(sent.field("model").as_deref(), Some("gpt-transcribe"));
        assert_eq!(sent.field("prompt").as_deref(), Some("Glossary: Zoon"));
        assert_eq!(sent.field("stream"), None);
        assert_eq!(sent.field("response_format").as_deref(), Some("json"));
        let file = sent
            .fields()
            .into_iter()
            .find(|(name, _, _)| name == "file")
            .unwrap();
        assert_eq!(file.1.as_deref(), Some("session_audio.wav"));
        assert_eq!(file.2, b"RIFFaudio");
    }

    #[test]
    fn uploads_are_named_by_their_audio_type() {
        for (mime_type, extension) in [
            ("audio/wav", "wav"),
            ("audio/x-wav", "wav"),
            ("audio/mpeg", "mp3"),
            ("audio/mp3", "mp3"),
            ("audio/x-m4a", "m4a"),
            ("audio/mp4", "mp4"),
            ("audio/ogg", "ogg"),
            ("audio/flac", "flac"),
            ("audio/x-flac", "flac"),
            ("audio/webm", "webm"),
            ("audio/webm;codecs=opus", "webm"),
            ("audio/aiff", "webm"),
        ] {
            assert_eq!(extension_for_mime_type(mime_type), extension, "{mime_type}");
        }
    }

    #[tokio::test]
    async fn flac_upload_is_named_as_flac_and_an_empty_prompt_is_left_out() {
        let server = TestServer::start(vec![Reply::json(200, json!({"text": "Plain."}))]).await;

        let text = api(&server, "sk-test")
            .transcribe(b"fLaC".to_vec(), "audio/flac", "gpt-transcribe", "")
            .await
            .unwrap();

        let sent = &server.received()[0];
        assert_eq!(text, "Plain.");
        assert_eq!(sent.field("prompt"), None);
        let file = sent
            .fields()
            .into_iter()
            .find(|(name, _, _)| name == "file")
            .unwrap();
        assert_eq!(file.1.as_deref(), Some("session_audio.flac"));
    }

    #[tokio::test]
    async fn a_tidy_request_asks_the_responses_api_for_low_verbosity() {
        let server = TestServer::start(vec![Reply::response("Tidied.")]).await;

        let text = api(&server, "sk-test")
            .respond("gpt-5-nano", "Rules", "um tidied", "minimal")
            .await
            .unwrap();

        let sent = &server.received()[0];
        assert_eq!(text, "Tidied.");
        assert_eq!(sent.path, "/v1/responses");
        assert_eq!(
            sent.json(),
            json!({
                "model": "gpt-5-nano",
                "input": [
                    {"role": "developer", "content": "Rules"},
                    {"role": "user", "content": "um tidied"},
                ],
                "reasoning": {"effort": "minimal"},
                "text": {"verbosity": "low"},
            })
        );
    }

    #[tokio::test]
    async fn api_errors_keep_their_status_code_and_param() {
        let server = TestServer::start(vec![Reply::error(
            400,
            "Unsupported value: 'minimal'",
            "unsupported_value",
            Some("reasoning.effort"),
        )])
        .await;

        let error = api(&server, "sk-test")
            .respond("gpt-5-nano", "Rules", "text", "minimal")
            .await
            .unwrap_err();

        assert!(error.is_bad_request());
        assert_eq!(error.param(), Some("reasoning.effort"));
        assert_eq!(
            error.to_string(),
            "Error code: 400 - Unsupported value: 'minimal'"
        );
    }

    #[tokio::test]
    async fn a_transcription_answer_without_text_is_an_error_sent_once() {
        let server = TestServer::start(vec![
            Reply::json(200, json!({"usage": {"seconds": 1}})),
            Reply::transcript(""),
        ])
        .await;
        let api = api(&server, "sk-test");

        let missing = api
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
            .await;
        assert_eq!(server.received().len(), 1);
        let empty = api
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
            .await;

        assert_eq!(
            missing,
            Err(ApiError::Unexpected(
                "the transcription answer had no text".into()
            ))
        );
        assert_eq!(empty, Ok(String::new()));
    }

    #[tokio::test]
    async fn an_incomplete_tidy_answer_is_an_error_not_part_of_the_text() {
        let server = TestServer::start(vec![Reply::json(
            200,
            json!({
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"},
                "output": [
                    {"type": "message", "content": [{"type": "output_text", "text": "Half of"}]},
                ]
            }),
        )])
        .await;

        let error = api(&server, "sk-test")
            .respond("gpt-5-nano", "Rules", "half of the text", "minimal")
            .await
            .unwrap_err();

        assert_eq!(
            error,
            ApiError::Unexpected("the response was incomplete (max_output_tokens)".into())
        );
    }

    #[tokio::test]
    async fn a_tidy_answer_whose_status_is_not_text_is_an_error() {
        let server = TestServer::start(vec![Reply::json(
            200,
            json!({
                "status": 123,
                "output": [
                    {"type": "message", "content": [{"type": "output_text", "text": "Odd"}]},
                ]
            }),
        )])
        .await;

        let error = api(&server, "sk-test")
            .respond("gpt-5-nano", "Rules", "text", "minimal")
            .await
            .unwrap_err();

        assert_eq!(error, ApiError::Unexpected("the response was 123".into()));
    }

    #[tokio::test]
    async fn an_answer_that_is_not_json_is_sent_once() {
        let not_json = Reply {
            status: 200,
            content_type: "application/json",
            body: "<html>Gateway</html>".into(),
            delay: Duration::ZERO,
            retry_after: None,
            cut_short: false,
        };
        let server =
            TestServer::start(vec![not_json.clone(), not_json, Reply::response("Unused.")]).await;
        let api = api(&server, "sk-test");

        let tidy = api.respond("gpt-5-nano", "Rules", "text", "minimal").await;
        let transcript = api
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
            .await;

        assert!(matches!(tidy, Err(ApiError::Unexpected(_))), "{tidy:?}");
        assert!(matches!(transcript, Err(ApiError::Unexpected(_))), "{transcript:?}");
        assert_eq!(server.received().len(), 2);
    }

    #[tokio::test]
    async fn no_key_sends_nothing() {
        let server = TestServer::start(vec![]).await;

        let error = api(&server, "  ")
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
            .await
            .unwrap_err();

        assert_eq!(error, ApiError::MissingKey);
        assert_eq!(error.to_string(), "Add your OpenAI API key in settings");
        assert!(server.received().is_empty());
    }

    fn server_error(status: u16) -> Reply {
        Reply::error(status, "Service unavailable", "server_error", None)
    }

    #[tokio::test]
    async fn a_server_error_is_tried_again_after_a_wait() {
        let server =
            TestServer::start(vec![server_error(503), Reply::transcript("Second try.")]).await;
        let started = std::time::Instant::now();

        let text = api(&server, "sk-test")
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "Zoon")
            .await
            .unwrap();

        assert_eq!(text, "Second try.");
        let sent = server.received();
        assert_eq!(sent.len(), 2);
        assert!(started.elapsed() >= RETRY_DELAYS[0]);
        // The second try sends the same audio, under the same name, and prompt.
        let file = |sent: &test_server::Received| {
            sent.fields()
                .into_iter()
                .find(|(name, _, _)| name == "file")
                .map(|(_, file_name, value)| (file_name, value))
        };
        let expected = (Some("session_audio.wav".to_string()), b"audio".to_vec());
        assert_eq!(file(&sent[0]), Some(expected.clone()));
        assert_eq!(file(&sent[1]), Some(expected));
        assert_eq!(sent[1].field("prompt").as_deref(), Some("Zoon"));
    }

    #[tokio::test]
    async fn a_dropped_connection_is_tried_again() {
        let server = TestServer::start(vec![Reply::hang_up(), Reply::response("Tidied.")]).await;

        let text = api(&server, "sk-test")
            .respond("gpt-5-nano", "Rules", "text", "minimal")
            .await
            .unwrap();

        assert_eq!(text, "Tidied.");
        assert_eq!(server.received().len(), 2);
    }

    #[tokio::test]
    async fn an_answer_cut_off_mid_download_is_tried_again() {
        let server = TestServer::start(vec![
            Reply::response("Tidied.").cut_short(),
            Reply::response("Tidied."),
            Reply::json(200, json!({"text": "Hello."})).cut_short(),
            Reply::json(200, json!({"text": "Hello."})),
        ])
        .await;
        let api = api(&server, "sk-test");

        let tidy = api.respond("gpt-5-nano", "Rules", "text", "minimal").await;
        let whole = api
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
            .await;

        assert_eq!(tidy, Ok("Tidied.".to_string()));
        assert_eq!(whole, Ok("Hello.".to_string()));
        assert_eq!(server.received().len(), 4);
    }

    #[tokio::test]
    async fn a_request_is_tried_three_times_at_most() {
        let server = TestServer::start(vec![
            server_error(502),
            server_error(500),
            server_error(504),
            Reply::transcript("Never asked for."),
        ])
        .await;

        let error = api(&server, "sk-test")
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
            .await
            .unwrap_err();

        assert!(matches!(error, ApiError::Status { status: 504, .. }));
        assert_eq!(server.received().len(), 3);
        assert_eq!(server.unused_replies(), 1);
    }

    #[tokio::test]
    async fn errors_that_will_not_pass_are_sent_once() {
        for reply in [
            Reply::error(400, "Bad audio", "invalid_value", None),
            Reply::error(401, "Incorrect API key", "invalid_api_key", None),
            Reply::error(403, "Forbidden", "forbidden", None),
            Reply::error(404, "No such model", "model_not_found", None),
            Reply::error(429, "Out of credit", "insufficient_quota", None),
        ] {
            let status = reply.status;
            let server = TestServer::start(vec![reply, Reply::transcript("Unused.")]).await;

            let error = api(&server, "sk-test")
                .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
                .await
                .unwrap_err();

            assert!(matches!(error, ApiError::Status { status: s, .. } if s == status));
            assert_eq!(server.received().len(), 1, "{status}");
        }
    }

    #[tokio::test]
    async fn a_rate_limit_waits_as_long_as_it_asks() {
        let server = TestServer::start(vec![
            Reply::error(429, "Slow down", "rate_limit_exceeded", None).retry_after(1),
            Reply::response("Tidied."),
        ])
        .await;
        let started = std::time::Instant::now();

        let text = api(&server, "sk-test")
            .respond("gpt-5-nano", "Rules", "text", "minimal")
            .await
            .unwrap();

        assert_eq!(text, "Tidied.");
        assert_eq!(server.received().len(), 2);
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[tokio::test]
    async fn a_cancel_during_the_wait_sends_nothing_more() {
        let server =
            TestServer::start(vec![server_error(503), Reply::transcript("Too late.")]).await;
        let api = api(&server, "sk-test");
        // Boxed, so drop(call) drops the call itself, not a pinned reference to it.
        let mut call =
            Box::pin(api.transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", ""));

        // Runs the call until its first try has failed and it is waiting.
        tokio::select! {
            _ = &mut call => panic!("the call ended during its wait"),
            _ = async {
                while server.received().is_empty() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                tokio::time::sleep(RETRY_DELAYS[0] / 4).await;
            } => {}
        }
        drop(call);
        tokio::time::sleep(RETRY_DELAYS[0] * 3).await;

        assert_eq!(server.received().len(), 1);
    }

    #[test]
    fn a_transcription_time_limit_grows_with_its_upload() {
        // A five-second dictation as FLAC: about a minute.
        assert_eq!(transcribe_timeout(160_000), Duration::from_secs(63));
        // A full-size part: about nine and a half minutes.
        assert_eq!(transcribe_timeout(25_165_824), Duration::from_secs(563));
        assert_eq!(transcribe_timeout(usize::MAX), MAX_TRANSCRIBE_TIMEOUT);
    }

    #[test]
    fn describe_error_leaves_out_the_message() {
        let error = ApiError::Status {
            status: 400,
            code: Some("invalid_input".into()),
            param: None,
            message: "Could not process input: My bank PIN is 4471".into(),
        };
        assert_eq!(
            error.describe(),
            "BadRequestError status=400 code=invalid_input"
        );
        let odd_code = ApiError::Status {
            status: 429,
            code: Some("quota exceeded for My bank PIN".into()),
            param: None,
            message: String::new(),
        };
        assert_eq!(odd_code.describe(), "RateLimitError status=429");
        assert_eq!(ApiError::Timeout.describe(), "APITimeoutError");
        assert!(!ApiError::Connection("reset".into())
            .describe()
            .contains("reset"));
    }
}
