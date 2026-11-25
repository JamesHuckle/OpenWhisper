import { describe, expect, it } from "vitest";
import { createRecordingListRefresh } from "./recording-list-refresh";

// A fake app whose recording_list replies are held until the test releases
// them, and a menu that records what it drew.
function setup() {
  const menu = { open: false, drawn: [] as string[][] };
  const pending: { reply: (recordings: string[]) => void; fail: (error: Error) => void }[] = [];
  const refresh = createRecordingListRefresh<string[]>({
    isMenuOpen: () => menu.open,
    list: () =>
      new Promise((reply, fail) => {
        pending.push({ reply, fail });
      }),
    draw: (recordings) => {
      menu.drawn.push(recordings);
    },
  });
  return { menu, pending, refresh };
}

describe("recording list refresh", () => {
  it("asks for nothing while the menu is closed, and shows the new entry when it opens", async () => {
    const { menu, pending, refresh } = setup();

    // A dictation finishing with the menu closed.
    await refresh();
    expect(pending).toHaveLength(0);
    expect(menu.drawn).toEqual([]);

    menu.open = true;
    const opening = refresh();
    expect(pending).toHaveLength(1);
    pending[0].reply(["rec-1"]);
    await opening;

    expect(menu.drawn).toEqual([["rec-1"]]);
  });

  it("keeps the newer cards when an older reply arrives last", async () => {
    const { menu, pending, refresh } = setup();
    menu.open = true;

    const older = refresh();
    const newer = refresh();
    pending[1].reply(["rec-2", "rec-1"]);
    await newer;
    pending[0].reply(["rec-1"]);
    await older;

    expect(menu.drawn).toEqual([["rec-2", "rec-1"]]);
  });

  it("draws nothing when the reply arrives after the menu closed", async () => {
    const { menu, pending, refresh } = setup();
    menu.open = true;

    const request = refresh();
    menu.open = false;
    pending[0].reply(["rec-1"]);
    await request;

    expect(menu.drawn).toEqual([]);
  });

  it("passes a failed list request to the caller", async () => {
    const { menu, pending, refresh } = setup();
    menu.open = true;

    const request = refresh();
    pending[0].fail(new Error("disk unavailable"));

    await expect(request).rejects.toThrow("disk unavailable");
    expect(menu.drawn).toEqual([]);
  });
});
