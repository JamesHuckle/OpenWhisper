"""Live smoke test: dictate a known WAV into a real Windows text field.

Open a Notepad window (any window whose focused control is a Win32 edit
control works), start the Tauri dev app with remote debugging, then run:

    python realtime-target-smoke.py speech.wav --target-window Notepad --expect "hello world"

The script brings the target window to the front so the app locks it as the
dictation target, feeds the WAV to the app's development smoke hook, and
passes only when the expected words appear in the field's text. Returning to
idle alone is not enough: the app also goes idle after a failure.
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import sys
import time
import urllib.request
import wave

_WM_GETTEXT = 0x000D
_WM_GETTEXTLENGTH = 0x000E
_SMTO_ABORTIFHUNG = 0x0002


def normalize(text: str) -> str:
    """Lower-case words only, so punctuation or a tidy pass does not matter."""
    return " ".join(re.findall(r"[a-z0-9']+", text.lower()))


def dictated_into(before: str, after: str, expected: str) -> bool:
    """True when the field gained another copy of the expected words."""
    phrase = normalize(expected)
    if not phrase:
        raise ValueError("--expect must contain at least one word")
    return normalize(after).count(phrase) > normalize(before).count(phrase)


class Win32TextField:
    """The focused control of a top-level window, read with WM_GETTEXT."""

    def __init__(self, title_part: str) -> None:
        import ctypes
        from ctypes import wintypes

        class GuiThreadInfo(ctypes.Structure):
            _fields_ = [
                ("cbSize", wintypes.DWORD),
                ("flags", wintypes.DWORD),
                ("hwndActive", wintypes.HWND),
                ("hwndFocus", wintypes.HWND),
                ("hwndCapture", wintypes.HWND),
                ("hwndMenuOwner", wintypes.HWND),
                ("hwndMoveSize", wintypes.HWND),
                ("hwndCaret", wintypes.HWND),
                ("rcCaret", wintypes.RECT),
            ]

        self._ctypes = ctypes
        self._GuiThreadInfo = GuiThreadInfo
        self._enum_proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        # Window handles are pointer-sized; name every type so none is cut to 32 bits.
        user32.EnumWindows.argtypes = [self._enum_proc, wintypes.LPARAM]
        user32.IsWindowVisible.argtypes = [wintypes.HWND]
        user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
        user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
        user32.SetForegroundWindow.argtypes = [wintypes.HWND]
        user32.GetForegroundWindow.restype = wintypes.HWND
        user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, wintypes.LPDWORD]
        user32.GetWindowThreadProcessId.restype = wintypes.DWORD
        user32.GetGUIThreadInfo.argtypes = [wintypes.DWORD, ctypes.POINTER(GuiThreadInfo)]
        user32.SendMessageTimeoutW.argtypes = [
            wintypes.HWND,
            wintypes.UINT,
            wintypes.WPARAM,
            wintypes.LPARAM,
            wintypes.UINT,
            wintypes.UINT,
            ctypes.POINTER(ctypes.c_size_t),
        ]
        user32.SendMessageTimeoutW.restype = wintypes.LPARAM
        self._user32 = user32
        self.window = self._find_window(title_part)
        if not self.window:
            raise SystemExit(f"No visible window title contains {title_part!r}")
        self.control = 0

    def _find_window(self, title_part: str) -> int:
        ctypes = self._ctypes
        found: list[int] = []

        def visit(hwnd, _lparam) -> bool:
            if not self._user32.IsWindowVisible(hwnd):
                return True
            length = self._user32.GetWindowTextLengthW(hwnd)
            buffer = ctypes.create_unicode_buffer(length + 1)
            self._user32.GetWindowTextW(hwnd, buffer, length + 1)
            if title_part.lower() in buffer.value.lower():
                found.append(hwnd)
                return False
            return True

        self._user32.EnumWindows(self._enum_proc(visit), 0)
        return found[0] if found else 0

    def focus(self) -> None:
        """Bring the window to the front and remember its focused control."""
        ctypes = self._ctypes
        self._user32.ShowWindow(self.window, 9)  # SW_RESTORE
        self._user32.SetForegroundWindow(self.window)
        time.sleep(0.5)
        if self._user32.GetForegroundWindow() != self.window:
            raise SystemExit("Could not bring the target window to the front")

        thread = self._user32.GetWindowThreadProcessId(self.window, None)
        info = self._GuiThreadInfo(cbSize=ctypes.sizeof(self._GuiThreadInfo))
        if not self._user32.GetGUIThreadInfo(thread, ctypes.byref(info)) or not info.hwndFocus:
            raise SystemExit("The target window has no focused text field")
        self.control = info.hwndFocus

    def text(self) -> str:
        ctypes = self._ctypes
        result = ctypes.c_size_t()
        self._user32.SendMessageTimeoutW(
            self.control, _WM_GETTEXTLENGTH, 0, 0, _SMTO_ABORTIFHUNG, 2000, ctypes.byref(result)
        )
        buffer = ctypes.create_unicode_buffer(result.value + 1)
        self._user32.SendMessageTimeoutW(
            self.control,
            _WM_GETTEXT,
            result.value + 1,
            ctypes.addressof(buffer),
            _SMTO_ABORTIFHUNG,
            2000,
            ctypes.byref(result),
        )
        return buffer.value


def main() -> None:
    parser = argparse.ArgumentParser(description="Feed deterministic PCM to the live Tauri dev app.")
    parser.add_argument("wav")
    parser.add_argument("--cdp-port", type=int, default=9222)
    parser.add_argument(
        "--target-window",
        required=True,
        help="Part of the title of the window to dictate into, such as Notepad",
    )
    parser.add_argument(
        "--expect",
        required=True,
        help="Words the WAV says; they must appear in the target field",
    )
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()
    if not normalize(args.expect):
        parser.error("--expect must contain at least one word")
    if sys.platform != "win32":
        raise SystemExit("This smoke test drives a real Windows text field; run it on Windows")

    from websockets.sync.client import connect

    with wave.open(args.wav, "rb") as audio:
        if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth()) != (24000, 1, 2):
            raise SystemExit("WAV must be 24 kHz mono PCM16")
        pcm = audio.readframes(audio.getnframes())
        rate = audio.getframerate()

    targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{args.cdp_port}/json/list"))
    target = next(item for item in targets if item.get("type") == "page")
    websocket = connect(target["webSocketDebuggerUrl"], open_timeout=10, close_timeout=5)
    request_id = 0

    def evaluate(expression: str) -> object:
        nonlocal request_id
        request_id += 1
        websocket.send(
            json.dumps(
                {
                    "id": request_id,
                    "method": "Runtime.evaluate",
                    "params": {"expression": expression, "returnByValue": True},
                }
            )
        )
        while True:
            message = json.loads(websocket.recv())
            if message.get("id") == request_id:
                result = message.get("result", {})
                if "exceptionDetails" in result:
                    raise RuntimeError(result["exceptionDetails"])
                return result.get("result", {}).get("value")

    if evaluate("typeof window.__openwhisperRealtimeSmoke") != "function":
        raise SystemExit("The Tauri realtime development smoke hook is unavailable")

    # The app locks whichever field has focus when the dictation starts.
    field = Win32TextField(args.target_window)
    field.focus()
    before = field.text()

    chunk_bytes = rate * 2 // 10
    chunks = [
        base64.b64encode(pcm[offset : offset + chunk_bytes]).decode()
        for offset in range(0, len(pcm), chunk_bytes)
    ]
    payload = base64.b64encode(json.dumps(chunks).encode()).decode()
    started = time.monotonic()
    evaluate(f"window.__openwhisperRealtimeSmoke(JSON.parse(atob('{payload}')), 100); 'started'")

    seen_active = False
    finished = False
    try:
        while time.monotonic() - started < args.timeout:
            time.sleep(0.2)
            state = evaluate("document.getElementById('widget')?.dataset.state")
            print(f"APP_STATE t={time.monotonic() - started:.3f}s state={state}", flush=True)
            seen_active = seen_active or state in {"recording", "transcribing"}
            if evaluate("document.getElementById('widget')?.dataset.notice") == "true":
                notice = evaluate("document.getElementById('widget-notice')?.textContent")
                raise SystemExit(f"FAIL the app reported a problem: {notice}")
            if state == "idle" and seen_active:
                finished = True
            if finished and dictated_into(before, field.text(), args.expect):
                print(f"PASS total={time.monotonic() - started:.3f}s")
                return
    finally:
        websocket.close()

    if not finished:
        raise SystemExit("FAIL the live Tauri smoke test did not complete")
    raise SystemExit(
        f"FAIL the app finished but {args.expect!r} did not appear in the target field"
    )


if __name__ == "__main__":
    main()
