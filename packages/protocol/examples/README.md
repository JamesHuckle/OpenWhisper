# Example messages

Each file is one real exchange between the desktop and the worker, in the
order a short dictation uses them: `start_session`, `stop_session`, then a
`finalize_session_audio` for the stopped session (`unknown_session`), plus
`list_models` and one pushed `session_event`. Every message is one JSON line
on the wire; replies always carry `result` and `error`, one of them `null`.

Both sides test against these files:

- the worker answers each request with exactly the reply here
  (`apps/worker/tests/test_protocol_examples.py`);
- the desktop sends each request in this shape and reads each reply and event
  (`apps/desktop/src-tauri/src/worker_connection.rs` tests, run by
  `cargo test` in `apps/desktop/src-tauri/core-tests`).

Change a message here and in both programs together.
