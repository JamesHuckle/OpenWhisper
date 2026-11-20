import { describe, expect, it } from "vitest";
import { errorMessage } from "./error-message";

describe("errorMessage", () => {
  it("uses an Error's message and a thrown string as they are", () => {
    expect(errorMessage(new Error("disk full"))).toBe("disk full");
    expect(errorMessage("no settings file")).toBe("no settings file");
  });

  it("falls back when the thrown value says nothing", () => {
    expect(errorMessage(undefined)).toBe("Transcription failed");
    expect(errorMessage(null, "Could not save settings")).toBe("Could not save settings");
    expect(errorMessage("  ", "Could not save settings")).toBe("Could not save settings");
  });
});
