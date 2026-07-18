from __future__ import annotations

import argparse
import base64
import json
import time
import urllib.request
import wave

from websockets.sync.client import connect


def main() -> None:
    parser = argparse.ArgumentParser(description="Feed deterministic PCM to the live Tauri dev app.")
    parser.add_argument("wav")
    parser.add_argument("--cdp-port", type=int, default=9222)
    args = parser.parse_args()

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

    chunk_bytes = rate * 2 // 10
    chunks = [
        base64.b64encode(pcm[offset : offset + chunk_bytes]).decode()
        for offset in range(0, len(pcm), chunk_bytes)
    ]
    payload = base64.b64encode(json.dumps(chunks).encode()).decode()
    started = time.monotonic()
    evaluate(f"window.__openwhisperRealtimeSmoke(JSON.parse(atob('{payload}')), 100); 'started'")

    seen_active = False
    while time.monotonic() - started < 20:
        time.sleep(0.2)
        state = evaluate("document.getElementById('widget')?.dataset.state")
        print(f"APP_STATE t={time.monotonic() - started:.3f}s state={state}", flush=True)
        seen_active = seen_active or state in {"recording", "transcribing"}
        if state == "idle" and seen_active:
            print(f"PASS total={time.monotonic() - started:.3f}s")
            websocket.close()
            return

    raise SystemExit("The live Tauri smoke test did not complete")


if __name__ == "__main__":
    main()
