// Post-handshake SDK commands, each lazily importing the SDK module so it
// stays in the deferred chunk (see discord.ts and the bundle budget). The
// dynamic import resolves instantly after boot — the chunk is already loaded.
//
// Nothing here throws or rejects: every caller gets an answer it can show.

/**
 * What became of a link handed to Discord:
 *
 * - `opened`: Discord opened it (or runs a client too old to say).
 * - `cancelled`: the person chose to stay on Discord's "leaving" prompt.
 *   Not a failure; show nothing.
 * - `failed`: the command was refused or never ran. Offer the link itself.
 */
export type LinkOutcome = 'opened' | 'cancelled' | 'failed';

/** Opens a link (e.g. a jump-to-message URL) through the Discord client. */
export async function openExternalLink(url: string): Promise<LinkOutcome> {
  try {
    const { getSdk } = await import('./discord');
    const { opened } = await getSdk().commands.openExternalLink({ url });
    return opened === false ? 'cancelled' : 'opened';
  } catch (e) {
    console.error('leaf: opening a link through Discord failed', e);
    return 'failed';
  }
}

/**
 * Asks Discord to close the Activity. Resolves to whether the request was
 * sent; Discord does not confirm it, so a caller that is still on screen a
 * moment later should point at Discord's own close control.
 */
export async function closeActivity(): Promise<boolean> {
  try {
    const { closeSdk } = await import('./discord');
    return closeSdk();
  } catch (e) {
    console.error('leaf: closing the Activity failed', e);
    return false;
  }
}

const LAYOUT_EVENT = 'ACTIVITY_LAYOUT_MODE_UPDATE';
/** `layout_mode` of an Activity shown in full (not picture-in-picture, not a grid tile). */
const LAYOUT_FOCUSED = 0;
/** Signals that arrive together (visibility, layout mode, focus) count once. */
const FOREGROUND_GAP_MS = 1_000;

/**
 * Calls `callback` when the Activity comes back in front of the person:
 * the page becomes visible again, Discord's layout mode returns to focused
 * (out of picture-in-picture or the grid), or the window regains focus. No
 * single signal is dependable on every client, so all three are watched and
 * a burst of them counts once. It is not called for the state at subscribe.
 *
 * `onLayoutMode` hears every layout mode Discord reports, the one at
 * subscribe included, from the same subscription (Discord tells a second
 * subscriber nothing until the mode changes).
 *
 * Returns a function that stops listening. Discord has no "left the
 * Activity" event, and a phone may fire none of these, so pair this with a
 * refresh control.
 */
export function onForeground(
  callback: () => void,
  onLayoutMode?: (mode: number) => void,
): () => void {
  let stopped = false;
  let lastFired = Number.NEGATIVE_INFINITY;
  const fire = (): void => {
    const at = Date.now();
    if (stopped || at - lastFired < FOREGROUND_GAP_MS) return;
    lastFired = at;
    callback();
  };

  const onVisibility = (): void => {
    if (document.visibilityState === 'visible') fire();
  };
  document.addEventListener('visibilitychange', onVisibility);
  window.addEventListener('focus', fire);

  // Discord publishes the current mode on subscribe, so only a change back
  // to focused counts.
  let mode: number | null = null;
  const onLayout = (update: { layout_mode: number }): void => {
    if (stopped) return;
    const before = mode;
    mode = update.layout_mode;
    onLayoutMode?.(mode);
    if (mode === LAYOUT_FOCUSED && before !== null && before !== LAYOUT_FOCUSED) fire();
  };
  let unsubscribe: (() => void) | null = null;
  void (async () => {
    try {
      const { getSdk } = await import('./discord');
      const sdk = getSdk();
      await sdk.subscribe(LAYOUT_EVENT, onLayout);
      unsubscribe = () => {
        sdk.unsubscribe(LAYOUT_EVENT, onLayout).catch(() => undefined);
      };
      if (stopped) unsubscribe();
    } catch {
      // No SDK (outside Discord) or a client without the event: the page
      // signals above still work.
    }
  })();

  return () => {
    stopped = true;
    document.removeEventListener('visibilitychange', onVisibility);
    window.removeEventListener('focus', fire);
    unsubscribe?.();
    unsubscribe = null;
  };
}
