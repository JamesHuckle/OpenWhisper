use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{path::BaseDirectory, AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct WorkerRequest {
    id: String,
    method: String,
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

pub struct WorkerClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Stderr output captured from the worker process, for error reporting.
    stderr_buf: Arc<Mutex<String>>,
}

impl WorkerClient {
    pub async fn spawn(app_handle: &AppHandle, openai_api_key: Option<String>) -> Result<Self> {
        let worker_src = worker_src_path();
        let mut last_err: Option<anyhow::Error> = None;
        let mut child_opt: Option<Child> = None;
        let debug_prefers_python =
            cfg!(debug_assertions) && env::var("OPENWHISPER_WORKER_BIN").is_err();

        if debug_prefers_python {
            for python_bin in python_candidates(&worker_src) {
                let mut cmd = Command::new(&python_bin);
                cmd.arg("-m")
                    .arg("openwhisper_worker")
                    .env("PYTHONPATH", &worker_src)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                #[cfg(target_os = "windows")]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
                if let Some(key) = openai_api_key
                    .as_ref()
                    .map(|k| k.trim())
                    .filter(|k| !k.is_empty())
                {
                    cmd.env("OPENAI_API_KEY", key);
                }

                match cmd.spawn() {
                    Ok(child) => {
                        eprintln!(
                            "[openwhisper][worker] spawned python worker '{}' with PYTHONPATH='{}'",
                            python_bin, worker_src
                        );
                        child_opt = Some(child);
                        break;
                    }
                    Err(err) => {
                        last_err = Some(anyhow!(
                            "Failed to spawn worker with '{}': {}",
                            python_bin,
                            err
                        ));
                    }
                }
            }
        }

        if child_opt.is_none() {
            if let Some(worker_bin) = worker_bin_candidates(app_handle)
                .into_iter()
                .find(|p| p.exists())
            {
                let mut cmd = Command::new(&worker_bin);
                cmd.stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                #[cfg(target_os = "windows")]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
                if let Some(key) = openai_api_key
                    .as_ref()
                    .map(|k| k.trim())
                    .filter(|k| !k.is_empty())
                {
                    cmd.env("OPENAI_API_KEY", key);
                }

                match cmd.spawn() {
                    Ok(child) => {
                        eprintln!(
                            "[openwhisper][worker] spawned bundled worker '{}'",
                            worker_bin.display()
                        );
                        child_opt = Some(child);
                    }
                    Err(err) => {
                        last_err = Some(anyhow!(
                            "Failed to spawn bundled worker '{}': {}",
                            worker_bin.display(),
                            err
                        ));
                    }
                }
            }
        }

        if child_opt.is_none() && !debug_prefers_python {
            for python_bin in python_candidates(&worker_src) {
                let mut cmd = Command::new(&python_bin);
                cmd.arg("-m")
                    .arg("openwhisper_worker")
                    .env("PYTHONPATH", &worker_src)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                #[cfg(target_os = "windows")]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
                if let Some(key) = openai_api_key
                    .as_ref()
                    .map(|k| k.trim())
                    .filter(|k| !k.is_empty())
                {
                    cmd.env("OPENAI_API_KEY", key);
                }

                match cmd.spawn() {
                    Ok(child) => {
                        eprintln!(
                            "[openwhisper][worker] spawned python worker '{}' with PYTHONPATH='{}'",
                            python_bin, worker_src
                        );
                        child_opt = Some(child);
                        break;
                    }
                    Err(err) => {
                        last_err = Some(anyhow!(
                            "Failed to spawn worker with '{}': {}",
                            python_bin,
                            err
                        ));
                    }
                }
            }
        }

        let mut child = child_opt
            .ok_or_else(|| last_err.unwrap_or_else(|| anyhow!("Failed to spawn worker process")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open worker stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open worker stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to open worker stderr"))?;

        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let buf_clone = stderr_buf.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                eprint!("{}", line);
                let _ = io::stderr().flush();
                let mut buf = buf_clone.lock().await;
                buf.push_str(&line);
                // Cap at 8 KB to avoid unbounded growth.
                if buf.len() > 8192 {
                    let keep_from = buf.len() - 4096;
                    let truncated = buf.split_off(keep_from);
                    *buf = truncated;
                }
                line.clear();
            }
        });

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr_buf,
        })
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let req = WorkerRequest {
            id: Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
        };

        let req_json = serde_json::to_string(&req)?;
        self.stdin.write_all(req_json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let mut line = String::new();
        self.stdout.read_line(&mut line).await?;
        if line.trim().is_empty() {
            let stderr = self.stderr_buf.lock().await;
            let msg = if stderr.trim().is_empty() {
                "Worker process exited unexpectedly".to_string()
            } else {
                format!("Worker process exited:\n{}", stderr.trim())
            };
            return Err(anyhow!("{}", msg));
        }

        let response: WorkerResponse = serde_json::from_str(&line)?;
        if response.id != req.id {
            return Err(anyhow!("Worker response id mismatch"));
        }

        if response.ok {
            Ok(response.result.unwrap_or(Value::Null))
        } else {
            let err = response
                .error
                .ok_or_else(|| anyhow!("Unknown worker error"))?;
            Err(anyhow!("{}: {}", err.code, err.message))
        }
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn worker_src_path() -> String {
    let default = worker_root_path().join("src").to_string_lossy().to_string();

    let raw = if cfg!(debug_assertions) {
        env::var("OPENWHISPER_WORKER_SRC").unwrap_or(default)
    } else {
        default
    };
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path.to_string_lossy().to_string()
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(path)
            .to_string_lossy()
            .to_string()
    }
}

fn worker_root_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../worker")
}

fn worker_bin_candidates(app_handle: &AppHandle) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();

    if cfg!(debug_assertions) {
        if let Ok(raw) = env::var("OPENWHISPER_WORKER_BIN") {
            let path = PathBuf::from(raw);
            if path.is_absolute() {
                paths.push(path);
            } else {
                paths.push(Path::new(env!("CARGO_MANIFEST_DIR")).join(path));
            }
        }
    }

    let resource_candidates = if cfg!(target_os = "windows") {
        vec!["binaries/openwhisper-worker.exe", "openwhisper-worker.exe"]
    } else {
        vec!["binaries/openwhisper-worker", "openwhisper-worker"]
    };
    for candidate in resource_candidates {
        if let Ok(path) = app_handle
            .path()
            .resolve(candidate, BaseDirectory::Resource)
        {
            paths.push(path);
        }
    }

    let local_dev_bin = if cfg!(target_os = "windows") {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("openwhisper-worker.exe")
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("openwhisper-worker")
    };
    paths.push(local_dev_bin);

    paths
}

fn python_candidates(worker_src: &str) -> Vec<String> {
    if cfg!(debug_assertions) {
        if let Ok(custom_bin) = env::var("OPENWHISPER_PYTHON_BIN") {
            return vec![custom_bin];
        }
    }

    let mut candidates: Vec<String> = Vec::new();
    let worker_root = Path::new(worker_src)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(worker_root_path);

    // Prefer project-local virtualenv so worker deps are reproducible anywhere.
    let venv_python = if cfg!(target_os = "windows") {
        worker_root.join(".venv").join("Scripts").join("python.exe")
    } else {
        worker_root.join(".venv").join("bin").join("python")
    };
    if venv_python.exists() {
        candidates.push(venv_python.to_string_lossy().to_string());
    }

    if cfg!(target_os = "windows") {
        candidates.push("python".to_string());
    } else {
        candidates.push("python3".to_string());
        candidates.push("python".to_string());
    }

    candidates
}
