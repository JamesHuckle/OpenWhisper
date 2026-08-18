import { describe, expect, it } from "vitest";
import { transcriptPreview } from "./recording-transcript";

describe("transcriptPreview", () => {
  it("shows the first sentence and indicates that more text is available", () => {
    expect(transcriptPreview("First sentence. Second sentence."))
      .toBe("First sentence.…");
  });

  it("normalizes whitespace and leaves short transcripts intact", () => {
    expect(transcriptPreview("  A short\n transcript  ")).toBe("A short transcript");
  });

  it("truncates a very long first sentence on a word boundary", () => {
    const transcript = "This is a deliberately long opening sentence with several useful words before it eventually reaches its conclusion.";

    expect(transcriptPreview(transcript, 55)).toBe("This is a deliberately long opening sentence with…");
  });
});
