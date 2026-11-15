"""Run with: python -m unittest discover -s qa/windows-overlay"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "realtime_target_smoke", Path(__file__).with_name("realtime-target-smoke.py")
)
smoke = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(smoke)


class DictatedIntoTests(unittest.TestCase):
    """The smoke test passes only when the words reach the target field."""

    def test_new_words_in_the_field_pass_whatever_the_punctuation(self) -> None:
        self.assertTrue(smoke.dictated_into("Notes: ", "Notes: Hello, world!", "hello world"))

    def test_an_unchanged_field_fails_even_when_it_already_held_the_words(self) -> None:
        self.assertFalse(smoke.dictated_into("hello world", "hello world", "hello world"))
        self.assertFalse(smoke.dictated_into("", "", "hello world"))

    def test_other_text_does_not_count(self) -> None:
        self.assertFalse(smoke.dictated_into("", "Goodbye world.", "hello world"))

    def test_a_second_copy_of_the_words_counts(self) -> None:
        self.assertTrue(smoke.dictated_into("Hello world.", "Hello world. Hello, world.", "hello world"))

    def test_expected_text_needs_a_word(self) -> None:
        with self.assertRaises(ValueError):
            smoke.dictated_into("", "text", " ... ")


if __name__ == "__main__":
    unittest.main()
