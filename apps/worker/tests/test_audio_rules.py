from __future__ import annotations

import json
import unittest
import wave
from pathlib import Path

from openwhisper_worker import audio_rules
from openwhisper_worker.server import (
    _echoes_prompt,
    _no_speech_reason,
    _no_speech_reason_from_counters,
)

PROTOCOL_DIR = Path(__file__).resolve().parents[3] / "packages" / "protocol"
SHARED_FILE = PROTOCOL_DIR / "audio-rules.json"
EXAMPLES_DIR = PROTOCOL_DIR / "audio-examples"


def shared_cases() -> dict:
    return json.loads((EXAMPLES_DIR / "cases.json").read_text(encoding="utf-8"))


class SharedAudioRulesTests(unittest.TestCase):
    """The worker follows the dictation rules the Android app also follows."""

    def test_a_worker_run_from_the_repository_reads_the_shared_file(self) -> None:
        self.assertEqual(audio_rules.shared_audio_rules_path(), SHARED_FILE)

    def test_the_worker_uses_exactly_the_shared_rules(self) -> None:
        shared = json.loads(SHARED_FILE.read_text(encoding="utf-8"))
        self.assertEqual(audio_rules.MIN_SPEECH_SECONDS, shared["min_speech_seconds"])
        self.assertEqual(audio_rules.SILENT_PCM_PEAK, shared["silent_pcm_peak"])
        self.assertEqual(audio_rules.PROMPT_ECHO_MIN_SHARE, shared["prompt_echo_min_share"])
        self.assertEqual(audio_rules.MAX_UPLOAD_BYTES, shared["max_upload_bytes"])
        self.assertEqual(audio_rules.NO_AUDIO_MESSAGE, shared["no_audio_message"])

    def test_every_example_recording_gets_its_expected_answer(self) -> None:
        cases = shared_cases()["audio"]
        self.assertTrue(cases)
        for case in cases:
            path = EXAMPLES_DIR / case["file"]
            with self.subTest(file=case["file"]):
                reason = _no_speech_reason(str(path), "audio/wav", path.stat().st_size)
                answer = reason.split("=")[0] if reason else "speech"
                self.assertEqual(answer, case["expected"])

    def test_example_recordings_sent_as_raw_pcm_get_the_same_answer(self) -> None:
        # The desktop's live dictation sends raw PCM chunks, not a WAV file,
        # and the worker only counts them.
        for case in shared_cases()["audio"]:
            with self.subTest(file=case["file"]):
                with wave.open(str(EXAMPLES_DIR / case["file"]), "rb") as source:
                    rate = source.getframerate()
                    pcm = source.readframes(source.getnframes())
                chunk_bytes = rate // 10 * 2
                peak = max(
                    (audio_rules.pcm_peak(pcm[i : i + chunk_bytes]) for i in range(0, len(pcm), chunk_bytes)),
                    default=0,
                )
                reason = _no_speech_reason_from_counters(len(pcm), peak, rate)
                answer = reason.split("=")[0] if reason else "speech"
                self.assertEqual(answer, case["expected"])

    def test_every_example_transcript_gets_its_expected_answer(self) -> None:
        cases = shared_cases()["prompt_echo"]
        self.assertTrue(cases)
        for case in cases:
            with self.subTest(text=case["text"], prompt=case["prompt"]):
                self.assertEqual(_echoes_prompt(case["text"], case["prompt"]), case["expected"])


if __name__ == "__main__":
    unittest.main()
