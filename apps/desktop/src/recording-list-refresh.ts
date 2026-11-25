// Refreshes the menu's recent recordings. The app is asked for the list only
// while the menu is open (opening the menu always asks), and a reply is drawn
// only if the menu is still open and no newer request has started since, so a
// slow older reply never replaces newer cards.

/** What refreshing needs from the app and the page. */
export type RecordingListRefreshDeps<R> = {
  isMenuOpen: () => boolean;
  list: () => Promise<R>;
  draw: (recordings: R) => void | Promise<void>;
};

export function createRecordingListRefresh<R>(deps: RecordingListRefreshDeps<R>) {
  let latestRequest = 0;

  return async function refreshRecordingList(): Promise<void> {
    if (!deps.isMenuOpen()) return;
    const request = ++latestRequest;
    const recordings = await deps.list();
    if (request !== latestRequest || !deps.isMenuOpen()) return;
    await deps.draw(recordings);
  };
}
