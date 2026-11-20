// Saves settings changes from the menu: one way for every field. Each field
// has its own busy key. A change made while that field is still saving waits
// and is saved next (only the newest one), and other fields still save.

import { errorMessage } from "./error-message";

/** How one save reports back. */
export type SaveOptions<S> = {
  /** The field's busy key. */
  busy: string;
  /** The toast shown after the save, if any. */
  done?: (updated: S) => string;
  /** The key status text shown when a key is stored (the page's default when absent). */
  label?: (updated: S) => string;
  /** Runs after the save, before the page shows the saved settings. */
  saved?: () => void;
  /** Runs after a failed save has shown its error. */
  onError?: () => unknown;
};

/** What saving needs from the app and the page. */
export type SettingsSaveDeps<S> = {
  save: (patch: Record<string, unknown>) => Promise<S>;
  apply: (updated: S, configuredLabel?: string) => void;
  showToast: (message: string) => void;
};

export function createSettingsSaver<S>(deps: SettingsSaveDeps<S>) {
  const busy = new Set<string>();
  // The newest change made to a field while it was saving, saved next.
  const pending = new Map<string, { patch: Record<string, unknown>; options: SaveOptions<S> }>();

  async function saveSettings(patch: Record<string, unknown>, options: SaveOptions<S>): Promise<void> {
    if (busy.has(options.busy)) {
      pending.set(options.busy, { patch, options });
      return;
    }

    busy.add(options.busy);
    try {
      const updated = await deps.save(patch);
      // A newer change is saved next; this older answer must not undo it on the page.
      if (!pending.has(options.busy)) {
        options.saved?.();
        deps.apply(updated, options.label?.(updated));
        const message = options.done?.(updated);
        if (message) deps.showToast(message);
      }
    } catch (err) {
      deps.showToast(errorMessage(err, "Could not save settings"));
      if (!pending.has(options.busy)) await options.onError?.();
    } finally {
      busy.delete(options.busy);
    }
    const next = pending.get(options.busy);
    if (next) {
      pending.delete(options.busy);
      await saveSettings(next.patch, next.options);
    }
  }

  return { saveSettings, isSaving: (key: string) => busy.has(key) };
}
