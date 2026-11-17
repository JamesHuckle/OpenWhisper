# OpenWhisper shared files

The files here are read by both apps, so the desktop and Android follow the
same models and dictation rules. Both apps call OpenAI directly.

## Shared model list

`models.json` is the one list of transcription models the app offers: the
default and live models, the menu label and hint for each, the models that
older saved settings move to, and the refine (tidy) model. The desktop settings
menu (`apps/desktop/src/models.ts`) and the desktop's settings and sessions
(`apps/desktop/src-tauri/src/models.rs`) read it; both are built into the app.
Change a model here only. The Android app still names `gpt-transcribe` in its
own code.

## Dictation rules

`audio-rules.json` holds the rules every app follows before and after it
sends a recording to OpenAI. The desktop builds it into the app
(`apps/desktop/src-tauri/src/audio_rules.rs`); the Android build
(`apps/android/app/build.gradle.kts`) turns it into `BuildConfig` values for
`AudioRules`.

- `min_speech_seconds`: a recording shorter than this is an accidental tap.
  It is not sent, and the user sees `no_audio_message`.
- `silent_pcm_peak`: 16-bit audio whose loudest sample stays below this is a
  muted microphone. It is not sent either. Each part of a long recording is
  checked the same way (but never for `min_speech_seconds`), and a silent
  part is not sent.
- `prompt_echo_min_share`: the models answer near-silent audio by repeating
  the prompt they were given. A transcript whose words (lower case, letters
  and digits only) appear in that order inside the prompt's words, and that
  is at least this share of the prompt's length, is treated as no speech.
  The desktop compares the whole transcript with the user's vocabulary
  prompt; Android, which sends only the previous part's text as a prompt,
  compares each part with that.
- `max_upload_bytes`: the largest file sent in one request. Longer PCM and
  WAV recordings are split into parts this size or smaller.
- `no_audio_message`: what the user sees for any of the cases above, or for
  an empty transcript.

`audio-examples/` holds WAV files and transcripts with the expected answer
for each rule; the desktop's Rust tests and Android's tests both run them. See
[`audio-examples/README.md`](audio-examples/README.md).

## Sending a recording

A Live (realtime) dictation streams 16-bit PCM to the OpenAI Realtime
websocket while the user speaks. No copy of the chunks is kept; the desktop
only counts their bytes and loudest sample to report silence or a quick tap
as no audio.

A Standard dictation is sent from the saved WAV once the user stops. For a
microphone recording the desktop also encodes the WAV once as smaller
lossless FLAC, split into parts that each fit one upload
(`audio.part-1.flac`, `audio.part-2.flac`, ... beside the WAV), each with the
byte range of the WAV's audio data it holds; together they cover the audio
in order. The WAV is still checked for silence or a quick tap, then the
parts are uploaded, one request each, in place of the WAV. The WAV is sent
instead when a part is missing, empty, larger than one upload or not where
the part before it ended, and, from the refused part on, when the service
refuses one (HTTP 400). The parts are removed when the transcription
finishes.

A recording too long for one request is sent in parts: its FLAC parts, or the
WAV cut at the upload size. Each part after the first is prompted with the
end of the text before it, and a part whose text only repeats that prompt
(the model answering a part that is mostly silence) is kept with no text, so
a sentence is never pasted twice. Android keeps the same rules; both apps run
the examples in `audio-examples/cases.json` (`long_recording_parts`).

The desktop saves each finished part's byte range, prompt and text in
`parts.json` in the recording's folder, written atomically:

```json
{ "key": "<sha256>", "parts": [{ "start": 0, "end": 50331648, "prompt": "Glossary: ...", "text": "..." }] }
```

A retry of the same recording skips the saved parts and sends only the rest,
building the next part's prompt from the saved text. A part that never
reaches `silent_pcm_peak` is not uploaded: it is saved as finished with no
text, so a retry skips it too, and the next part's prompt quotes the last
part that had text. The key is the SHA-256 of
`{"audio_bytes": ..., "audio_modified_ns": ..., "model": ..., "prompt": ..., "version": 3}`
(keys sorted, written as Python's `json.dumps` writes it), made from the
audio's size and modified time, the model, the vocabulary prompt and a
chunking version; when any of them changes, the transcription starts fresh.
The tidy pass is not part of the key, so changing its prompt never uploads
audio again. The desktop deletes the file when the transcription completes
(so transcribing a finished recording again starts fresh) and when the
recording is pruned.
