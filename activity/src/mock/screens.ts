// The mock screens, as the screen viewer's sidebar lists them. One list for
// the sidebar and for Screen.test.ts, which mounts every id in it: a screen
// cannot be listed without rendering, or render without being checked.

interface ScreenLink {
  /** What `?screen=` takes and Screen.svelte renders. */
  id: string;
  label: string;
}

interface ScreenGroup {
  name: string;
  /**
   * The group's screens are leaf as Discord shows it minimised: they are
   * opened in a viewport the size of that tile (see {@link TILE_SIZES}), not
   * of a phone, and it is the size that makes them what they are.
   */
  tile?: true;
  items: readonly ScreenLink[];
}

/**
 * The sizes the minimised screens are shown and checked at, in CSS px: the
 * tile Discord for Android makes, a larger square, a 16:9 window, and the
 * page Discord for Android really lays out behind its 120 x 120 tile (taller
 * than the tile, which shows only its middle; measured on a Pixel 8).
 */
export const TILE_SIZES = [
  { width: 120, height: 120 },
  { width: 160, height: 160 },
  { width: 240, height: 135 },
  { width: 120, height: 214 },
] as const;

export const SCREEN_GROUPS = [
  {
    name: 'Gallery',
    items: [
      { id: 'picker', label: 'Series picker' },
      { id: 'picker-empty', label: 'Picker — empty' },
      { id: 'picker-blocked', label: 'Picker — blocked' },
      { id: 'home', label: 'Series home' },
      { id: 'home-empty', label: 'Home — first post' },
      { id: 'home-sprout', label: 'Home — sprout' },
      { id: 'home-channel-gone', label: 'Home — channel gone' },
      { id: 'viewer', label: 'Day viewer' },
      { id: 'viewer-video', label: 'Viewer — a video' },
      { id: 'viewer-loading', label: 'Viewer — loading day' },
      { id: 'viewer-failed', label: 'Viewer — day didn’t load' },
    ],
  },
  {
    name: 'Creator',
    items: [
      { id: 'create', label: 'Create series' },
      { id: 'create-blocked', label: 'Create — blocked' },
      { id: 'myseries', label: 'My series' },
      { id: 'settings', label: 'Series settings' },
      { id: 'settings-channel-unseen', label: 'Settings — channel leaf can’t see' },
    ],
  },
  {
    name: 'Admin',
    items: [
      { id: 'admin-login', label: 'Admin login' },
      { id: 'admin-panel', label: 'Admin panel' },
    ],
  },
  {
    name: 'States',
    items: [
      { id: 'loading', label: 'Boot — loading' },
      { id: 'loading-consent', label: 'Boot — waiting for OK' },
      { id: 'error', label: 'Boot — Discord silent' },
      { id: 'error-retry', label: 'Boot — can retry' },
      { id: 'unavailable', label: 'Series unavailable' },
      { id: 'expired', label: 'Session ended' },
      { id: 'load-error', label: 'Gallery didn’t load' },
      { id: 'landing', label: 'Browser landing page' },
    ],
  },
  {
    name: 'Minimised',
    tile: true,
    items: [
      { id: 'tile-day', label: 'Tile — a day' },
      { id: 'tile-series', label: 'Tile — a series' },
      { id: 'tile-empty', label: 'Tile — no days yet' },
      { id: 'tile-list', label: 'Tile — series list' },
      { id: 'tile-boot', label: 'Tile — starting' },
      { id: 'tile-error', label: 'Tile — didn’t start' },
      { id: 'tile-expired', label: 'Tile — session ended' },
    ],
  },
] as const satisfies readonly ScreenGroup[];

/** The id of a listed screen. */
export type ScreenId = (typeof SCREEN_GROUPS)[number]['items'][number]['id'];

export interface ListedScreen {
  id: ScreenId;
  label: string;
  /** Opened at a tile's size, not a phone's (see {@link TILE_SIZES}). */
  tile: boolean;
}

/** Every listed screen, in sidebar order. */
export const SCREENS: readonly ListedScreen[] = SCREEN_GROUPS.flatMap((group) =>
  group.items.map((item) => ({ ...item, tile: 'tile' in group })),
);

/** The screens a phone or a desktop shows at its own size. */
export const FULL_SCREENS: readonly ListedScreen[] = SCREENS.filter((screen) => !screen.tile);

/** leaf as Discord shows it minimised. */
export const TILE_SCREENS: readonly ListedScreen[] = SCREENS.filter((screen) => screen.tile);
