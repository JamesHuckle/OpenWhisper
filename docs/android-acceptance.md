# Android companion acceptance criteria

The first Android deliverable retains Samsung Keyboard and adds OpenWhisper as an
accessibility overlay. It is considered demonstrable when all of the following are
repeatable in the automated demo environment and on a Samsung device:

1. The mic control appears only while an editable field and an input-method window
   are both present.
2. The control never appears for password or platform-sensitive fields.
3. A tap starts recording, another tap finalizes it, and cancellation is always safe.
4. Final text replaces only the current selection or inserts at the current cursor.
5. Existing text is never silently discarded.
6. A deterministic demo transcriber exercises partial and final transcript handling
   without a network connection or API key.
7. Accessibility being disabled, microphone denial, network failure, focus loss,
   rotation, and service restart all result in a recoverable state.
8. A debug APK, unit-test report, and instrumented end-to-end test report can be
   produced from a clean checkout using repository scripts.

The overlay is not an extension loaded by Samsung Keyboard. It is a separate,
user-authorized accessibility surface positioned relative to the active IME window.
