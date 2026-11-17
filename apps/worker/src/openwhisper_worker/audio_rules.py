"""The dictation rules the Android app also follows.

They live in packages/protocol/audio-rules.json: the shortest recording worth
sending, the silence level, the prompt-echo check and the largest upload.
"""

from __future__ import annotations

import re
import sys
from array import array
from collections.abc import Callable
from pathlib import Path

from openwhisper_worker.shared_files import load_shared_json, shared_file_path

_FILE_NAME = "audio-rules.json"


def shared_audio_rules_path() -> Path:
    return shared_file_path(_FILE_NAME)


_SHARED = load_shared_json(_FILE_NAME)

# A quick tap of the shortcut captures a fraction of a second of PCM. Uploading
# it either fails (the API rejects clips under 0.1 s) or makes the model echo
# the dictionary prompt back, so anything this short is no audio.
MIN_SPEECH_SECONDS: float = float(_SHARED["min_speech_seconds"])
# A muted or disconnected microphone produces near-digital silence. Real speech
# peaks far above this, even on quiet microphones.
SILENT_PCM_PEAK: int = int(_SHARED["silent_pcm_peak"])
# A transcript that repeats at least this share of the prompt, and nothing
# else, is the model answering silence.
PROMPT_ECHO_MIN_SHARE: float = float(_SHARED["prompt_echo_min_share"])
# The largest file sent in one transcription request.
MAX_UPLOAD_BYTES: int = int(_SHARED["max_upload_bytes"])
NO_AUDIO_MESSAGE: str = _SHARED["no_audio_message"]

# How much PCM the silence check reads at a time.
_PCM_SCAN_BYTES = 1024 * 1024
# How much of the text before it a long recording's next part is prompted with.
PREVIOUS_PART_CHARS = 1200


def pcm_peak(block: bytes) -> int:
    """The loudest 16-bit little-endian sample in `block` (0 when it has none)."""
    usable = len(block) - (len(block) % 2)
    if not usable:
        return 0
    samples = array("h")
    samples.frombytes(block[:usable])
    if sys.byteorder == "big":
        samples.byteswap()
    return max(max(samples), -min(samples))


def pcm_is_silent(read: Callable[[int], bytes]) -> bool:
    """True when 16-bit samples from `read` never reach SILENT_PCM_PEAK.

    The same check decides whether a whole recording is a muted microphone
    and whether one part of a long recording is worth uploading.
    """
    while True:
        block = read(_PCM_SCAN_BYTES)
        if not block:
            return True
        if pcm_peak(block) >= SILENT_PCM_PEAK:
            return False


def _normalize_words(text: str) -> str:
    return " ".join(re.findall(r"[a-z0-9]+", text.lower()))


def echoes_prompt(text: str, prompt: str) -> bool:
    """Transcription models answer silent or near-silent audio by repeating
    the prompt they were sent. Such a transcript is no speech, not dictation."""
    spoken = _normalize_words(text)
    hints = _normalize_words(prompt)
    if not spoken or not hints:
        return False
    return spoken in hints and len(spoken) >= len(hints) * PROMPT_ECHO_MIN_SHARE


def previous_part_text(transcripts: list[str]) -> str:
    """The text a long recording's next part is prompted with: the end of the
    last part that had text."""
    previous = next((text for text in reversed(transcripts) if text), "")
    return previous[-PREVIOUS_PART_CHARS:]


def kept_part_text(text: str, prompt: str, previous: str) -> str:
    """A part's text, or nothing when it only repeats the prompt it was sent
    with or the earlier text that prompt quotes. A part that is mostly
    silence makes the model say its prompt back, which would paste a
    sentence twice. The rules and their examples are shared with the Android
    app (packages/protocol/audio-examples/cases.json, long_recording_parts)."""
    if echoes_prompt(text, prompt) or echoes_prompt(text, previous):
        return ""
    return text
