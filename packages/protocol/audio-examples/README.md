# Example audio

Each WAV file here is a short 16 kHz mono recording with a known answer to the
question "does this hold speech worth sending to OpenAI?". `cases.json` lists
the files and the expected result: `too_short`, `silent` or `speech`. It also
lists transcripts with the prompt they were made from, and whether each one
only repeats that prompt, and (`long_recording_parts`) how a long recording's
parts join: what the service answers for each part (null for a silent part,
which is never sent), the earlier text each part's prompt quotes, the parts
dropped because they only repeat it, and the transcript pasted.

The desktop's Rust tests (`apps/desktop/src-tauri/src/audio_rules.rs` and
`transcriber.rs`, run by `cargo test -p openwhisper_core` in `apps/desktop/src-tauri`)
and the Android unit tests (`AudioRulesTest`, `LongAudioChunkingTest`) run
every case, so the desktop and Android decide the same way. `make_examples.py` writes the WAV files; rerun it from the
repository root only to change them, and keep `cases.json` in step.
