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

  it("ignores a change to a field that is still saving, but saves other fields", async () => {
    let finish!: (settings: Settings) => void;
    const save = vi.fn(
      (patch: Record<string, unknown>) =>
        "transcriptionPrompt" in patch
          ? new Promise<Settings>((resolve) => (finish = resolve))
          : Promise.resolve({ hasOpenaiApiKey: true, holdToTalk: true }),
    );
    const { saveSettings, isSaving } = setup(save);

    const first = saveSettings({ transcriptionPrompt: "a" }, { busy: "prompt" });
    expect(isSaving("prompt")).toBe(true);
    await saveSettings({ transcriptionPrompt: "b" }, { busy: "prompt" });
    await saveSettings({ holdToTalk: true }, { busy: "holdToTalk" });
    finish({ hasOpenaiApiKey: true, holdToTalk: true });
    await first;

    expect(save.mock.calls.map(([patch]) => patch)).toEqual([{ transcriptionPrompt: "a" }, { holdToTalk: true }]);
    expect(isSaving("prompt")).toBe(false);
    const next = saveSettings({ transcriptionPrompt: "c" }, { busy: "prompt" });
    expect(save).toHaveBeenCalledTimes(3);
    finish({ hasOpenaiApiKey: true, holdToTalk: true });
    await next;
  });
});
