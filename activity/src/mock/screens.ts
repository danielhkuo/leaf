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
  items: readonly ScreenLink[];
}

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
      { id: 'viewer', label: 'Day viewer' },
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
] as const satisfies readonly ScreenGroup[];

/** The id of a listed screen. */
export type ScreenId = (typeof SCREEN_GROUPS)[number]['items'][number]['id'];

/** Every listed screen, in sidebar order. */
export const SCREENS: readonly { id: ScreenId; label: string }[] = SCREEN_GROUPS.flatMap(
  (group) => [...group.items],
);
