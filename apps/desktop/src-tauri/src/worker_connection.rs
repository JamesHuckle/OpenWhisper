//! Line-based JSON link to the worker process.
//!
//! Requests can overlap: each one waits only for the reply that carries its
//! id, so a cancel or a ping is answered while a long transcription runs.
//! Session events arrive on the same stdout without an id and are passed on as
//! they come. This file uses no Tauri types so its tests run on their own.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

/// How long the desktop waits for each kind of worker request.
pub fn request_time_limit(method: &str) -> Duration {
    match method {
        // Covers every audio part, each with its own network time limit,
        // and the tidy pass.
        "finalize_session_audio" => Duration::from_secs(2 * 60 * 60),
        // A live session waits up to two minutes for a segment when it
        // rotates to a new connection.
        "append_audio_chunk" => Duration::from_secs(3 * 60),
        "start_session" | "list_models" => Duration::from_secs(60),
        _ => Duration::from_secs(30),
    }
}

#[derive(Debug, Serialize)]
struct WorkerRequest<'a> {
    id: &'a str,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct WorkerResponse {
    id: String,
    ok: bool,
    result: Option<Value>,
    error: Option<WorkerError>,
}

#[derive(Debug, Deserialize)]
struct WorkerError {
    code: String,
    message: String,
}

type Reply = std::result::Result<Value, String>;

#[derive(Default)]
struct Shared {
    pending: HashMap<String, oneshot::Sender<Reply>>,
    /// Set once the worker's stdout closes; later requests fail at once.
    exited: Option<String>,
    /// Set when the desktop stops the worker itself, so no exit is reported.
    closing: bool,
}

pub type EventHandler = Box<dyn Fn(Value) + Send + Sync>;
pub type ExitHandler = Box<dyn FnOnce(String) + Send>;

pub struct WorkerConnection<W> {
    stdin: Mutex<W>,
    shared: Arc<StdMutex<Shared>>,
}

impl<W: AsyncWrite + Unpin + Send> WorkerConnection<W> {
    /// Starts reading `stdout` in the background. `on_event` gets each
    /// session event; `on_exit` runs once if the worker stops on its own,
    /// with the text from `exit_message`.
    pub fn start<R, F, Fut>(
        stdin: W,
        stdout: R,
        on_event: EventHandler,
        on_exit: ExitHandler,
        exit_message: F,
    ) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = String> + Send,
    {
        let shared = Arc::new(StdMutex::new(Shared::default()));
        let reader_shared = shared.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                route_line(&line, &reader_shared, &on_event);
            }
            let message = exit_message().await;
            let (waiters, closing) = {
                let mut shared = lock(&reader_shared);
                shared.exited = Some(message.clone());
                (
                    shared.pending.drain().map(|(_, tx)| tx).collect::<Vec<_>>(),
                    shared.closing,
                )
            };
            for waiter in waiters {
                let _ = waiter.send(Err(message.clone()));
            }
            if !closing {
                on_exit(message);
            }
        });
        Self {
            stdin: Mutex::new(stdin),
            shared,
        }
    }

    pub fn is_alive(&self) -> bool {
        lock(&self.shared).exited.is_none()
    }

    /// Marks a stop the desktop asked for, so it is not reported as a crash.
    pub fn mark_closing(&self) {
        lock(&self.shared).closing = true;
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_within(method, params, request_time_limit(method))
            .await
    }

    pub async fn request_within(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut shared = lock(&self.shared);
            if let Some(message) = &shared.exited {
                return Err(anyhow!("{}", message));
            }
            shared.pending.insert(id.clone(), tx);
        }
        // Forgets the request if it times out, fails to send, or the caller
        // gives up, so a late reply is dropped instead of misrouted.
        let _pending = PendingGuard {
            shared: &self.shared,
            id: &id,
        };

        let mut line = serde_json::to_string(&WorkerRequest {
            id: &id,
            method,
            params,
        })?;
        line.push('\n');
        // The time limit covers waiting to send as well as the reply, so a
        // worker that stops reading its input cannot hold a request forever.
        let deadline = tokio::time::Instant::now() + limit;
        let not_taken = || {
            // A line may have been cut off part way, so this link cannot
            // carry another request; the next one starts a new worker.
            fail_link(&self.shared, "Worker stopped reading requests");
            anyhow!(
                "Worker did not take {} within {} seconds",
                method,
                limit.as_secs()
            )
        };
        let Ok(mut stdin) = tokio::time::timeout_at(deadline, self.stdin.lock()).await else {
            return Err(not_taken());
        };
        // A request that waited here behind one that timed out must not
        // finish that request's cut-off line.
        if let Some(message) = &lock(&self.shared).exited {
            return Err(anyhow!("{}", message));
        }
        let sent = tokio::time::timeout_at(deadline, async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        })
        .await;
        match sent {
            Ok(result) => result?,
            // Marked while this request still holds the input, so no request
            // waiting for it can write first.
            Err(_) => return Err(not_taken()),
        }
        drop(stdin);

        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(reply)) => reply.map_err(|message| anyhow!("{}", message)),
            Ok(Err(_)) => Err(anyhow!("Worker process exited unexpectedly")),
            Err(_) => Err(anyhow!(
                "Worker did not answer {} within {} seconds",
                method,
                limit.as_secs()
            )),
        }
    }
}

struct PendingGuard<'a> {
    shared: &'a StdMutex<Shared>,
    id: &'a str,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        lock(self.shared).pending.remove(self.id);
    }
}

/// Marks the link unusable and fails every request still waiting for a reply,
/// so none waits out its own time limit.
fn fail_link(shared: &StdMutex<Shared>, message: &str) {
    let (message, waiters) = {
        let mut shared = lock(shared);
        let message = shared
            .exited
            .get_or_insert_with(|| message.to_string())
            .clone();
        let waiters = shared.pending.drain().map(|(_, tx)| tx).collect::<Vec<_>>();
        (message, waiters)
    };
    for waiter in waiters {
        let _ = waiter.send(Err(message.clone()));
    }
}

fn lock(shared: &StdMutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn route_line(line: &str, shared: &StdMutex<Shared>, on_event: &EventHandler) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("[openwhisper][worker] unreadable worker output: {err}");
            return;
        }
    };
    if value.get("id").is_none() {
        if value.get("session_id").is_some() {
            on_event(value);
        }
        return;
    }
    let response: WorkerResponse = match serde_json::from_value(value) {
        Ok(response) => response,
        Err(err) => {
            eprintln!("[openwhisper][worker] unreadable worker response: {err}");
            return;
        }
    };
    let waiter = lock(shared).pending.remove(&response.id);
    let Some(waiter) = waiter else {
        // The request already timed out, or the worker could not read it.
        if let Some(error) = response.error {
            eprintln!(
                "[openwhisper][worker] unmatched worker reply: {}",
                error.code
            );
        }
        return;
    };
    let reply = if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        Err(match response.error {
            Some(error) => format!("{}: {}", error.code, error.message),
            None => "Unknown worker error".to_string(),
        })
    };
    let _ = waiter.send(reply);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;
    use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    struct FakeWorker {
        requests: tokio::io::Lines<BufReader<DuplexStream>>,
        replies: DuplexStream,
    }

    impl FakeWorker {
        async fn next_request(&mut self) -> Value {
            let line = self.requests.next_line().await.unwrap().unwrap();
            serde_json::from_str(&line).unwrap()
        }

        async fn send(&mut self, value: Value) {
            let mut line = value.to_string();
            line.push('\n');
            self.replies.write_all(line.as_bytes()).await.unwrap();
        }
    }

    fn connect() -> (
        Arc<WorkerConnection<DuplexStream>>,
        FakeWorker,
        Arc<StdMutex<Vec<Value>>>,
        oneshot::Receiver<String>,
    ) {
        let (desktop_in, worker_out) = duplex(64 * 1024);
        let (worker_in, desktop_out) = duplex(64 * 1024);
        let events = Arc::new(StdMutex::new(Vec::new()));
        let seen = events.clone();
        let (exit_tx, exit_rx) = oneshot::channel();
        let connection = WorkerConnection::start(
            desktop_in,
            desktop_out,
            Box::new(move |event| seen.lock().unwrap().push(event)),
            Box::new(move |message| {
                let _ = exit_tx.send(message);
            }),
            || async { "Worker process exited".to_string() },
        );
        let worker = FakeWorker {
            requests: BufReader::new(worker_out).lines(),
            replies: worker_in,
        };
        (Arc::new(connection), worker, events, exit_rx)
    }

    #[tokio::test]
    async fn a_cancel_is_answered_while_a_transcription_is_still_running() {
        let (connection, mut worker, events, _exit) = connect();
        let finalizing = connection.clone();
        let finalize = tokio::spawn(async move {
            finalizing
                .request("finalize_session_audio", json!({ "session_id": "s1" }))
                .await
        });
        let finalize_request = worker.next_request().await;
        assert_eq!(finalize_request["method"], "finalize_session_audio");
        worker
            .send(json!({
                "session_id": "s1",
                "event": { "type": "partial", "text": "Transcribing part 1 of 5..." },
                "done": false,
            }))
            .await;

        let stopping = connection.clone();
        let stop = tokio::spawn(async move {
            stopping
                .request("stop_session", json!({ "session_id": "s1" }))
                .await
        });
        let stop_request = worker.next_request().await;
        assert_eq!(stop_request["method"], "stop_session");
        worker
            .send(json!({ "id": stop_request["id"], "ok": true, "result": { "note": "Session stopped." } }))
            .await;
        assert_eq!(stop.await.unwrap().unwrap()["note"], "Session stopped.");
        assert!(!finalize.is_finished());
        assert_eq!(
            events.lock().unwrap()[0]["event"]["text"],
            "Transcribing part 1 of 5..."
        );

        worker
            .send(json!({ "id": finalize_request["id"], "ok": true, "result": { "cancelled": true } }))
            .await;
        assert_eq!(finalize.await.unwrap().unwrap()["cancelled"], true);
    }

    #[tokio::test]
    async fn a_late_reply_after_a_time_limit_does_not_break_the_next_request() {
        let (connection, mut worker, _events, _exit) = connect();
        let slow = connection
            .request_within("ping", json!({}), Duration::from_millis(20))
            .await;
        assert!(slow.unwrap_err().to_string().contains("did not answer ping"));
        let slow_request = worker.next_request().await;

        let next = connection.clone();
        let list = tokio::spawn(async move { next.request("list_models", json!({})).await });
        let list_request = worker.next_request().await;
        worker
            .send(json!({ "id": slow_request["id"], "ok": true, "result": { "late": true } }))
            .await;
        worker
            .send(json!({ "id": list_request["id"], "ok": true, "result": { "models": [] } }))
            .await;
        assert_eq!(list.await.unwrap().unwrap(), json!({ "models": [] }));
    }

    #[tokio::test]
    async fn a_worker_that_stops_reading_fails_the_request_and_is_replaced() {
        let (connection, _worker, _events, _exit) = connect();
        // Larger than the pipe, and the worker never reads it.
        let chunk = "x".repeat(256 * 1024);
        let sent = tokio::time::timeout(
            Duration::from_secs(5),
            connection.request_within(
                "append_audio_chunk",
                json!({ "chunk_base64": chunk }),
                Duration::from_millis(50),
            ),
        )
        .await
        .expect("the time limit must cover sending");
        assert!(sent.unwrap_err().to_string().contains("did not take append_audio_chunk"));
        assert!(!connection.is_alive());
        let after = connection.request("ping", json!({})).await.unwrap_err();
        assert_eq!(after.to_string(), "Worker stopped reading requests");
    }

    #[tokio::test]
    async fn a_send_that_times_out_fails_every_waiting_request_and_queued_send() {
        let (connection, mut worker, _events, _exit) = connect();
        let waiting = connection.clone();
        let finalize = tokio::spawn(async move {
            waiting
                .request("finalize_session_audio", json!({ "session_id": "s1" }))
                .await
        });
        worker.next_request().await;

        // Larger than the pipe, and the worker stops reading part way.
        let stuck = connection.clone();
        let chunk = "x".repeat(256 * 1024);
        let append = tokio::spawn(async move {
            stuck
                .request_within(
                    "append_audio_chunk",
                    json!({ "chunk_base64": chunk }),
                    Duration::from_millis(50),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let queued = connection.clone();
        let ping = tokio::spawn(async move {
            queued
                .request_within("ping", json!({}), Duration::from_secs(5))
                .await
        });

        let append = append.await.unwrap().unwrap_err().to_string();
        assert!(append.contains("did not take append_audio_chunk"), "{append}");
        let finalize = tokio::time::timeout(Duration::from_secs(5), finalize)
            .await
            .expect("the waiting request fails at once")
            .unwrap()
            .unwrap_err();
        assert_eq!(finalize.to_string(), "Worker stopped reading requests");
        let ping = ping.await.unwrap().unwrap_err();
        assert_eq!(ping.to_string(), "Worker stopped reading requests");
        // The worker reads on: the cut-off line is never finished by the ping.
        let rest = tokio::time::timeout(Duration::from_millis(200), worker.requests.next_line()).await;
        assert!(rest.is_err(), "another request was written after the cut-off line");
    }

    #[tokio::test]
    async fn a_worker_that_stops_fails_waiting_requests_and_is_reported() {
        let (connection, mut worker, _events, exit) = connect();
        let waiting = connection.clone();
        let finalize = tokio::spawn(async move {
            waiting
                .request("finalize_session_audio", json!({ "session_id": "s1" }))
                .await
        });
        worker.next_request().await;
        drop(worker);

        let error = finalize.await.unwrap().unwrap_err().to_string();
        assert_eq!(error, "Worker process exited");
        assert_eq!(exit.await.unwrap(), "Worker process exited");
        assert!(!connection.is_alive());
        let after = connection.request("ping", json!({})).await.unwrap_err();
        assert_eq!(after.to_string(), "Worker process exited");
    }

    #[tokio::test]
    async fn a_stop_the_desktop_asked_for_is_not_reported() {
        let (connection, worker, _events, exit) = connect();
        connection.mark_closing();
        drop(worker);
        assert!(exit.await.is_err());
    }

    /// One exchange from `packages/protocol/examples`, which the worker's
    /// tests answer with the same replies.
    fn example(json: &str) -> Value {
        serde_json::from_str(json).unwrap()
    }

    const EXCHANGES: [&str; 4] = [
        include_str!("../../../../packages/protocol/examples/start_session.json"),
        include_str!("../../../../packages/protocol/examples/stop_session.json"),
        include_str!("../../../../packages/protocol/examples/unknown_session.json"),
        include_str!("../../../../packages/protocol/examples/list_models.json"),
    ];

    #[tokio::test]
    async fn requests_and_replies_match_the_shared_examples() {
        for exchange in EXCHANGES.map(example) {
            let (connection, mut worker, _events, _exit) = connect();
            let expected_request = &exchange["request"];
            let method = expected_request["method"].as_str().unwrap().to_string();
            let params = expected_request["params"].clone();
            let asking = connection.clone();
            let answer = tokio::spawn(async move { asking.request(&method, params).await });

            let request = worker.next_request().await;
            assert_eq!(request["method"], expected_request["method"]);
            assert_eq!(request["params"], expected_request["params"]);
            assert!(request["id"].as_str().is_some_and(|id| !id.is_empty()));
            let mut reply = exchange["reply"].clone();
            reply["id"] = request["id"].clone();
            worker.send(reply).await;

            let expected_reply = &exchange["reply"];
            match answer.await.unwrap() {
                Ok(result) => {
                    assert_eq!(expected_reply["ok"], true);
                    assert_eq!(result, expected_reply["result"]);
                }
                Err(error) => {
                    assert_eq!(expected_reply["ok"], false);
                    let expected = format!(
                        "{}: {}",
                        expected_reply["error"]["code"].as_str().unwrap(),
                        expected_reply["error"]["message"].as_str().unwrap()
                    );
                    assert_eq!(error.to_string(), expected);
                }
            }
        }
    }

    #[tokio::test]
    async fn the_shared_example_session_event_is_passed_on_unchanged() {
        let line = example(include_str!(
            "../../../../packages/protocol/examples/session_event.json"
        ))["event"]
            .clone();
        let (connection, mut worker, events, _exit) = connect();
        worker.send(line.clone()).await;
        // A ping answered after the event proves the event line was read first.
        let asking = connection.clone();
        let ping = tokio::spawn(async move { asking.request("ping", json!({})).await });
        let request = worker.next_request().await;
        worker
            .send(json!({ "id": request["id"], "ok": true, "result": {}, "error": null }))
            .await;
        ping.await.unwrap().unwrap();

        assert_eq!(*events.lock().unwrap(), vec![line]);
    }

    #[tokio::test]
    async fn worker_errors_are_returned_to_the_caller() {
        let (connection, mut worker, _events, _exit) = connect();
        let asking = connection.clone();
        let stop = tokio::spawn(async move {
            asking
                .request("stop_session", json!({ "session_id": "gone" }))
                .await
        });
        let request = worker.next_request().await;
        worker
            .send(json!({
                "id": request["id"],
                "ok": false,
                "error": { "code": "bad_request", "message": "Unknown session_id" },
            }))
            .await;
        assert_eq!(
            stop.await.unwrap().unwrap_err().to_string(),
            "bad_request: Unknown session_id"
        );
    }
}
