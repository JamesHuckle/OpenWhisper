import { describe, expect, it } from "vitest";
import shared from "../../../packages/protocol/models.json";
import {
  DEFAULT_TRANSCRIPTION_MODEL,
  isLiveTranscription,
  LIVE_TRANSCRIPTION_MODEL,
  REFINE_MODEL,
  TRANSCRIPTION_MODELS,
  transcriptionModelHint,
  transcriptionModelOptionsHtml,
} from "./models";

describe("shared model list", () => {
  it("offers the standard and live models from packages/protocol", () => {
    expect(TRANSCRIPTION_MODELS.map((option) => option.id)).toEqual([
      "gpt-transcribe",
      "gpt-live-transcribe",
    ]);
    expect(DEFAULT_TRANSCRIPTION_MODEL).toBe("gpt-transcribe");
    expect(LIVE_TRANSCRIPTION_MODEL).toBe("gpt-live-transcribe");
    expect(REFINE_MODEL).toBe(shared.refine_model);
  });

  it("only points at models the settings menu offers", () => {
    const offered = TRANSCRIPTION_MODELS.map((option) => option.id);

    expect(offered).toContain(shared.default_transcription_model);
    expect(offered).toContain(shared.live_transcription_model);
    for (const replacement of Object.values(shared.legacy_transcription_models)) {
      expect(offered).toContain(replacement);
    }
  });

  it("builds one settings menu entry per model, labelled from the list", () => {
    const entries = [
      ...transcriptionModelOptionsHtml().matchAll(/<option value="([^"]*)">([^<]*)<\/option>/g),
    ].map(([, value, label]) => [value, label]);

    expect(entries).toEqual([
      ["gpt-transcribe", "Standard — GPT-Transcribe ($0.0045/min)"],
      ["gpt-live-transcribe", "Live — GPT-Live-Transcribe ($0.017/min)"],
    ]);
  });

  it("shows each model's hint and nothing for an unknown model", () => {
    expect(transcriptionModelHint("gpt-live-transcribe")).toBe(
      "Types as you speak, so text is ready the moment you stop. $0.017 per audio minute.",
    );
    expect(transcriptionModelHint("gpt-5.4")).toBe("");
  });

  it("streams audio and text only for live transcription", () => {
    expect(isLiveTranscription("gpt-live-transcribe")).toBe(true);
    expect(isLiveTranscription("gpt-transcribe")).toBe(false);
    expect(isLiveTranscription(undefined)).toBe(false);
  });
});
