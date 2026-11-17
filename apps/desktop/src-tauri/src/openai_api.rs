//! The two OpenAI HTTP calls the desktop makes: transcribe one audio file
//! (`/v1/audio/transcriptions`, streamed) and tidy a transcript
//! (`/v1/responses`). The Live websocket is in src/realtime.rs.
//!
//! Errors keep the API's message for the user, but `describe` (used in logs
//! and warnings) gives only the error type, HTTP status and error code: an
//! API message can quote the request, which holds the user's dictated words
//! and prompts.
//!
//! This file needs no Tauri, so its tests run in `core-tests`.

use reqwest::{multipart, Client, Response};
use serde_json::{json, Value};
use std::fmt;
use std::future::Future;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Long recordings can legitimately take many minutes to upload and
/// transcribe, so a request may wait this long between pieces of its answer.
const READ_TIMEOUT: Duration = Duration::from_secs(1800);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole of one transcription request: ten minutes to upload a part and
/// thirty to hear back, as before.
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(40 * 60);
/// The whole of one tidy request.
const POLISH_TIMEOUT: Duration = Duration::from_secs(420);

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
            ApiError::MissingKey => write!(f, "OPENAI_API_KEY is not set"),
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

/// One connection pool for every dictation, so connections stay warm.
pub fn new_http_client() -> Result<Client, String> {
    ensure_crypto_provider();
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
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

impl OpenAiApi for HttpApi {
    async fn transcribe(
        &self,
        audio: Vec<u8>,
        mime_type: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String, ApiError> {
        let key = self.key()?;
        let file = multipart::Part::bytes(audio)
            .file_name(format!("session_audio{}", extension_for_mime(mime_type)))
            .mime_str(mime_type)
            .map_err(|_| ApiError::Unexpected(format!("audio type {mime_type}")))?;
        let mut form = multipart::Form::new()
            .text("model", model.to_string())
            .text("stream", "true")
            .text("response_format", "json")
            .part("file", file);
        if !prompt.is_empty() {
            form = form.text("prompt", prompt.to_string());
        }
        let response = self
            .client
            .post(format!("{}/audio/transcriptions", self.base_url))
            .bearer_auth(key)
            .timeout(TRANSCRIBE_TIMEOUT)
            .multipart(form)
            .send()
            .await
            .map_err(request_error)?;
        read_transcript(checked(response).await?).await
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
        let response = self
            .client
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(key)
            .timeout(POLISH_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(request_error)?;
        let answer: Value = checked(response)
            .await?
            .json()
            .await
            .map_err(request_error)?;
        Ok(responses_text(&answer))
    }
}

fn request_error(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        ApiError::Timeout
    } else {
        ApiError::Connection(error.without_url().to_string())
    }
}

/// The response when its status is a success, or the API's error.
async fn checked(response: Response) -> Result<Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let error = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .unwrap_or(Value::Null);
    let text = |name: &str| error.get(name).and_then(Value::as_str).map(str::to_string);
    let message = text("message").unwrap_or_else(|| {
        let body = body.trim();
        if body.is_empty() {
            status.canonical_reason().unwrap_or("Request failed").to_string()
        } else {
            body.chars().take(300).collect()
        }
    });
    Err(ApiError::Status {
        status: status.as_u16(),
        code: text("code"),
        param: text("param"),
        message,
    })
}

/// The transcript from a streamed answer (server-sent events, the text in
/// `transcript.text.done`) or a plain JSON one.
async fn read_transcript(mut response: Response) -> Result<String, ApiError> {
    let streamed = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if !streamed {
        let answer: Value = response.json().await.map_err(request_error)?;
        return Ok(answer["text"].as_str().unwrap_or("").to_string());
    }
    let mut pending: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request_error)? {
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            if let Some(text) = transcript_event(&String::from_utf8_lossy(&line))? {
                return Ok(text);
            }
        }
    }
    Ok(transcript_event(&String::from_utf8_lossy(&pending))?.unwrap_or_default())
}

/// The finished transcript, if this event line carries it.
fn transcript_event(line: &str) -> Result<Option<String>, ApiError> {
    let Some(data) = line.trim().strip_prefix("data:") else {
        return Ok(None);
    };
    let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
        return Ok(None);
    };
    match event["type"].as_str() {
        Some("transcript.text.done") => Ok(Some(event["text"].as_str().unwrap_or("").to_string())),
        Some("error") => Err(ApiError::Unexpected(
            event["error"]["message"]
                .as_str()
                .unwrap_or("transcription stream failed")
                .to_string(),
        )),
        _ => Ok(None),
    }
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

/// The file name extension the service uses to recognise an upload.
fn extension_for_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/wav" | "audio/x-wav" => ".wav",
        "audio/mpeg" | "audio/mp3" => ".mp3",
        "audio/mp4" | "audio/x-m4a" => ".m4a",
        "audio/ogg" => ".ogg",
        "audio/flac" | "audio/x-flac" => ".flac",
        _ => ".webm",
    }
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
        haystack.windows(needle.len()).position(|window| window == needle)
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
    /// an optional delay.
    #[derive(Clone)]
    pub struct Reply {
        pub status: u16,
        pub content_type: &'static str,
        pub body: String,
        pub delay: Duration,
    }

    impl Reply {
        /// A streamed transcription that ends with `text`.
        pub fn transcript(text: &str) -> Self {
            let delta = serde_json::json!({"type": "transcript.text.delta", "delta": text});
            let done = serde_json::json!({"type": "transcript.text.done", "text": text});
            Self {
                status: 200,
                content_type: "text/event-stream",
                body: format!("data: {delta}\n\ndata: {done}\n\n"),
                delay: Duration::ZERO,
            }
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
            }
        }

        pub fn after(mut self, delay: Duration) -> Self {
            self.delay = delay;
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
                        key.eq_ignore_ascii_case(name).then(|| value.trim().to_string())
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
            let answer = format!(
                "HTTP/1.1 {} Test\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n{}",
                reply.status,
                reply.content_type,
                reply.body.len(),
                reply.body
            );
            if socket.write_all(answer.as_bytes()).await.is_err() {
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
    async fn a_recording_is_streamed_with_its_prompt() {
        let server = TestServer::start(vec![Reply::transcript("Hello there.")]).await;

        let text = api(&server, "sk-test")
            .transcribe(b"RIFFaudio".to_vec(), "audio/wav", "gpt-transcribe", "Glossary: Zoon")
            .await
            .unwrap();

        let sent = &server.received()[0];
        assert_eq!(text, "Hello there.");
        assert_eq!(sent.path, "/v1/audio/transcriptions");
        assert_eq!(sent.authorization, "Bearer sk-test");
        assert_eq!(sent.field("model").as_deref(), Some("gpt-transcribe"));
        assert_eq!(sent.field("prompt").as_deref(), Some("Glossary: Zoon"));
        assert_eq!(sent.field("stream").as_deref(), Some("true"));
        assert_eq!(sent.field("response_format").as_deref(), Some("json"));
        let file = sent.fields().into_iter().find(|(name, _, _)| name == "file").unwrap();
        assert_eq!(file.1.as_deref(), Some("session_audio.wav"));
        assert_eq!(file.2, b"RIFFaudio");
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
        let file = sent.fields().into_iter().find(|(name, _, _)| name == "file").unwrap();
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
        assert_eq!(error.to_string(), "Error code: 400 - Unsupported value: 'minimal'");
    }

    #[tokio::test]
    async fn no_key_sends_nothing() {
        let server = TestServer::start(vec![]).await;

        let error = api(&server, "  ")
            .transcribe(b"audio".to_vec(), "audio/wav", "gpt-transcribe", "")
            .await
            .unwrap_err();

        assert_eq!(error, ApiError::MissingKey);
        assert!(server.received().is_empty());
    }

    #[test]
    fn describe_error_leaves_out_the_message() {
        let error = ApiError::Status {
            status: 400,
            code: Some("invalid_input".into()),
            param: None,
            message: "Could not process input: My bank PIN is 4471".into(),
        };
        assert_eq!(error.describe(), "BadRequestError status=400 code=invalid_input");
        let odd_code = ApiError::Status {
            status: 429,
            code: Some("quota exceeded for My bank PIN".into()),
            param: None,
            message: String::new(),
        };
        assert_eq!(odd_code.describe(), "RateLimitError status=429");
        assert_eq!(ApiError::Timeout.describe(), "APITimeoutError");
        assert!(!ApiError::Connection("reset".into()).describe().contains("reset"));
    }
}
