// Saves settings changes from the menu: one way for every field. Each field
// has its own busy key, so a change made while that field is still saving is
// ignored, and other fields still save.

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

  async function saveSettings(patch: Record<string, unknown>, options: SaveOptions<S>) {
    if (busy.has(options.busy)) return;

    busy.add(options.busy);
    try {
      const updated = await deps.save(patch);
      options.saved?.();
      deps.apply(updated, options.label?.(updated));
      const message = options.done?.(updated);
      if (message) deps.showToast(message);
    } catch (err) {
      deps.showToast(err instanceof Error ? err.message : String(err));
      await options.onError?.();
    } finally {
      busy.delete(options.busy);
    }
  }

  return { saveSettings, isSaving: (key: string) => busy.has(key) };
}
