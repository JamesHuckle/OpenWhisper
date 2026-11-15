import { describe, expect, it } from "vitest";
import { finalWriteNotice } from "./target-write";

describe("final transcript notices", () => {
  it("confirms text written into the target field", () => {
    expect(finalWriteNotice({ outcome: "replaced", reason: null })).toBe("Transcribed live");
    expect(finalWriteNotice({ outcome: "kept", reason: null })).toBe("Transcribed live");
    expect(finalWriteNotice({ outcome: "pasted", reason: null })).toBe("Transcribed and pasted");
  });

  it("says why the transcript was copied instead of typed", () => {
    expect(finalWriteNotice({ outcome: "copied", reason: "password_field" })).toBe(
      "Not typed into a password field. Transcript copied",
    );
    expect(finalWriteNotice({ outcome: "copied", reason: "window_changed" })).toBe(
      "You switched windows, so the transcript was copied",
    );
    expect(finalWriteNotice({ outcome: "copied", reason: "no_target" })).toBe(
      "No text field to paste into. Transcript copied",
    );
    expect(finalWriteNotice({ outcome: "copied", reason: "unsupported" })).toBe(
      "Transcript copied to clipboard",
    );
  });
});
