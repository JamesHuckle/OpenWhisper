import { describe, expect, it, vi } from "vitest";
import { createSettingsSaver } from "./settings-save";

type Settings = { hasOpenaiApiKey: boolean; holdToTalk: boolean };

function setup(save: (patch: Record<string, unknown>) => Promise<Settings>) {
  const calls: string[] = [];
  const saver = createSettingsSaver<Settings>({
    save: (patch) => {
      calls.push(`save ${JSON.stringify(patch)}`);
      return save(patch);
    },
    apply: (updated, label) => calls.push(`apply ${updated.holdToTalk} ${label}`),
    showToast: (message) => calls.push(`toast ${message}`),
  });
  return { calls, ...saver };
}

describe("saveSettings", () => {
  it("saves, runs saved before showing the result, then shows the done toast", async () => {
    const { calls, saveSettings } = setup(async () => ({ hasOpenaiApiKey: true, holdToTalk: false }));

    await saveSettings({ holdToTalk: false }, {
      busy: "holdToTalk",
      saved: () => calls.push("saved"),
      label: (updated) => (updated.hasOpenaiApiKey ? "Key saved" : "Key cleared"),
      done: (updated) => (updated.holdToTalk ? "Hold to talk enabled" : "Press to start and stop enabled"),
    });

    expect(calls).toEqual([
      'save {"holdToTalk":false}',
      "saved",
      "apply false Key saved",
      "toast Press to start and stop enabled",
    ]);
  });

  it("leaves the page's default label and shows no toast when none is given", async () => {
    const { calls, saveSettings } = setup(async () => ({ hasOpenaiApiKey: false, holdToTalk: true }));

    await saveSettings({ refinePrompt: "x" }, { busy: "refine" });

    expect(calls).toEqual(['save {"refinePrompt":"x"}', "apply true undefined"]);
  });

  it("shows the error, then runs onError, and frees the field after onError finishes", async () => {
    const { calls, saveSettings, isSaving } = setup(async () => {
      throw new Error("disk full");
    });
    let busyDuringOnError: boolean | undefined;

    await saveSettings({ showIdlePill: false }, {
      busy: "showIdlePill",
      saved: () => calls.push("saved"),
      done: () => "Idle pill hidden",
      onError: async () => {
        busyDuringOnError = isSaving("showIdlePill");
        calls.push("onError");
      },
    });

    expect(calls).toEqual(['save {"showIdlePill":false}', "toast disk full", "onError"]);
    expect(busyDuringOnError).toBe(true);
    expect(isSaving("showIdlePill")).toBe(false);
  });

  it("shows a thrown string as is", async () => {
    const { calls, saveSettings } = setup(() => Promise.reject("no settings file"));

    await saveSettings({ transcriptionModel: "m" }, { busy: "model" });

    expect(calls).toContain("toast no settings file");
  });

  it("saves the newest change made while a field is saving once that save ends, and shows only its answer", async () => {
    const finishes: ((settings: Settings) => void)[] = [];
    const save = vi.fn(
      (patch: Record<string, unknown>) =>
        "holdToTalk" in patch
          ? new Promise<Settings>((resolve) => finishes.push(resolve))
          : Promise.resolve({ hasOpenaiApiKey: true, holdToTalk: true }),
    );
    const { calls, saveSettings, isSaving } = setup(save);
    const options = { busy: "holdToTalk", saved: () => calls.push("saved") };

    const first = saveSettings({ holdToTalk: true }, options);
    expect(isSaving("holdToTalk")).toBe(true);
    await saveSettings({ holdToTalk: false }, options);
    await saveSettings({ holdToTalk: true }, options);
    await saveSettings({ refinePrompt: "x" }, { busy: "refine" });
    expect(save.mock.calls.map(([patch]) => patch)).toEqual([{ holdToTalk: true }, { refinePrompt: "x" }]);

    finishes[0]({ hasOpenaiApiKey: true, holdToTalk: true });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(save.mock.calls.map(([patch]) => patch)).toEqual([
      { holdToTalk: true },
      { refinePrompt: "x" },
      { holdToTalk: true },
    ]);
    // Only the refine save was applied so far; the older holdToTalk answer was not.
    expect(calls.filter((call) => call.startsWith("apply") || call === "saved")).toEqual(["apply true undefined"]);

    finishes[1]({ hasOpenaiApiKey: true, holdToTalk: true });
    await first;
    expect(calls.filter((call) => call.startsWith("apply") || call === "saved")).toEqual([
      "apply true undefined",
      "saved",
      "apply true undefined",
    ]);
    expect(isSaving("holdToTalk")).toBe(false);
  });

  it("skips onError for a failed save when a newer change is waiting, and saves that change", async () => {
    let fail!: (error: unknown) => void;
    const save = vi
      .fn<(patch: Record<string, unknown>) => Promise<Settings>>()
      .mockImplementationOnce(() => new Promise<Settings>((_, reject) => (fail = reject)))
      .mockResolvedValue({ hasOpenaiApiKey: true, holdToTalk: false });
    const { calls, saveSettings } = setup(save);
    const options = { busy: "holdToTalk", onError: () => calls.push("onError") };

    const first = saveSettings({ holdToTalk: true }, options);
    await saveSettings({ holdToTalk: false }, options);
    fail(new Error("disk full"));
    await first;

    expect(calls).toEqual([
      'save {"holdToTalk":true}',
      "toast disk full",
      'save {"holdToTalk":false}',
      "apply false undefined",
    ]);
  });
});
