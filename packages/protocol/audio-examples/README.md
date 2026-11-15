# Example audio

Each WAV file here is a short 16 kHz mono recording with a known answer to the
question "does this hold speech worth sending to OpenAI?". `cases.json` lists
the files and the expected result: `too_short`, `silent` or `speech`. It also
lists transcripts with the prompt they were made from, and whether each one
only repeats that prompt.

The worker's tests (`apps/worker/tests/test_audio_rules.py`) and the Android
unit tests (`AudioRulesTest`) both run every case, so the desktop and Android
decide the same way. `make_examples.py` writes the WAV files; rerun it from the
repository root only to change them, and keep `cases.json` in step.
