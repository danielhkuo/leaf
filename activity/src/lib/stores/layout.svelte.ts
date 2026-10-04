// Whether Discord has shrunk leaf to a tile: the small window a minimised
// Activity becomes, beside the chat (about 120 x 120 CSS px on Android; other
// clients differ). No screen of the gallery can be read at that size, let
// alone used, so the shell shows one small card instead (see
// components/shared/Minimisable.svelte).
//
// The viewport decides, because it is what the page really has to draw in.
// Discord's layout mode is a second signal, for a window somewhat bigger
// than that: not every client reports it, and one that reported
// picture-in-picture and never the return must not leave a full-size gallery
// stuck behind the card, so it too is held to a size.

/** `layout_mode` of an Activity in picture-in-picture (ACTIVITY_LAYOUT_MODE_UPDATE). */
const LAYOUT_PIP = 1;
/** Both sides under this many CSS px: a tile, whatever Discord says. */
export const TILE_MAX_PX = 260;
/** Both sides under this while Discord says picture-in-picture: a tile too. */
export const PIP_MAX_PX = 480;

/** Whether a viewport of this size, in this layout mode (`null`: not reported), is a tile. */
export function isTile(width: number, height: number, mode: number | null): boolean {
  // A webview can report 0 before its first layout. That is no size at all.
  if (width <= 0 || height <= 0) return false;
  const max = mode === LAYOUT_PIP ? PIP_MAX_PX : TILE_MAX_PX;
  return width < max && height < max;
}

const browser = typeof window !== 'undefined';
const seen = $state({
  width: browser ? window.innerWidth : 0,
  height: browser ? window.innerHeight : 0,
  mode: null as number | null,
});
const tile = $derived(isTile(seen.width, seen.height, seen.mode));

export const layout = {
  /** Whether leaf is shown as a tile right now. */
  get tile(): boolean {
    return tile;
  },
};

/** Records the layout mode Discord reports (see `onForeground` in sdk/actions.ts). */
export function setLayoutMode(mode: number): void {
  seen.mode = mode;
}

/**
 * Takes the viewport's size as it is this instant. For what cannot wait to
 * be told: WebKit lays a page out for its new size, and scrolls it, before
 * it reports the resize.
 */
export function measureViewport(): void {
  const was = tile;
  seen.width = window.innerWidth;
  seen.height = window.innerHeight;
  // Grown out of a tile, leaf is open again, and a report of
  // picture-in-picture is over whether or not the client says so. Left
  // standing, it would turn a phone into a tile when the keyboard comes up
  // and the webview shrinks (360 x 300, say).
  if (was && !tile) seen.mode = null;
}

/**
 * Follows the viewport's size until the returned function is called. The
 * handler only stores two numbers; what reads `layout.tile` is told when the
 * answer changes, not on every resize.
 */
export function watchViewport(): () => void {
  measureViewport();
  window.addEventListener('resize', measureViewport);
  return () => window.removeEventListener('resize', measureViewport);
}
