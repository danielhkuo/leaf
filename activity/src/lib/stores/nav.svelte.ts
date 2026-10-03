// In-app navigation as a small back-stack (no router library: bundle
// discipline). The shell renders `nav.current`. Only leaf's own controls
// move it: the on-screen Back and Close buttons, Escape in the day viewer.
// Nothing is mirrored into the browser history, so the system back gesture
// stays Discord's (whether Android Back even reaches the Activity's webview
// is untested; see docs/ux-audit.md, activity-browse-12).

import type { LaunchIntent, Series } from '../types/api';

export type View =
  | { name: 'picker' }
  /** `created`: the series was just made here; Home says so once. */
  | { name: 'home'; seriesId: number; created?: boolean }
  | { name: 'viewer'; seriesId: number; day: number }
  | { name: 'createSeries' }
  | { name: 'mySeries' }
  | { name: 'seriesSettings'; seriesId: number };

/** How the current view was reached. */
export type NavMove = 'push' | 'back' | 'reset';

interface Entry {
  view: View;
  /** Unique per entry, so a view pushed again remounts (see `nav.key`). */
  key: number;
  /** Window scroll when another view was pushed over this one. */
  scrollY: number;
}

const PICKER: View = { name: 'picker' };

let nextKey = 1;
const stack = $state<Entry[]>([{ view: PICKER, key: 0, scrollY: 0 }]);
let move = $state<NavMove>('reset');
let version = $state(0);

function entry(view: View): Entry {
  const key = nextKey;
  nextKey += 1;
  return { view, key, scrollY: 0 };
}

function windowScroll(): number {
  return typeof window === 'undefined' ? 0 : window.scrollY;
}

export const nav = {
  /** The view on top of the stack. */
  get current(): View {
    return stack[stack.length - 1]?.view ?? PICKER;
  },
  /**
   * Identity of the current entry. It changes whenever a different entry is
   * on top, even one equal in value, so `{#key nav.key}` remounts a view
   * that a deep link opens again over itself.
   */
  get key(): number {
    return stack[stack.length - 1]?.key ?? 0;
  },
  /** Whether there is somewhere to go back to. */
  get canGoBack(): boolean {
    return stack.length > 1;
  },
  /** How the current view was reached. */
  get lastMove(): NavMove {
    return move;
  },
  /**
   * The scroll to put back for the current view: where it was left when
   * something was pushed over it, if it was reached by going back; else 0.
   */
  get savedScroll(): number {
    return move === 'back' ? (stack[stack.length - 1]?.scrollY ?? 0) : 0;
  },
  /**
   * Counts every move. Compare two readings to tell whether the person went
   * anywhere in between (before following a late deep link, say).
   */
  get version(): number {
    return version;
  },
  /** Pushes a new view, remembering the scroll of the one it covers. */
  push(view: View): void {
    const top = stack[stack.length - 1];
    if (top) top.scrollY = windowScroll();
    stack.push(entry(view));
    move = 'push';
    version += 1;
  },
  /** Replaces the entire stack: `first` at the bottom, `rest` on top of it. */
  reset(first: View, ...rest: View[]): void {
    stack.splice(0, stack.length, entry(first), ...rest.map(entry));
    move = 'reset';
    version += 1;
  },
  /** Pops back one view, if possible. */
  back(): void {
    if (stack.length <= 1) return;
    stack.pop();
    move = 'back';
    version += 1;
  },
};

/**
 * Moves focus to a view's heading when the view appears, so a screen reader
 * announces the new screen and Tab continues from its top rather than from
 * a button that no longer exists. Call it from the view's `onMount` with
 * the view's root; the heading needs `tabindex="-1"`.
 */
export function focusHeading(root: HTMLElement | undefined): void {
  const heading = root?.querySelector<HTMLElement>('h1');
  heading?.focus({ preventScroll: true });
}

// --- where the gallery opens -------------------------------------------------

/** A series (and optionally one of its days) to open. */
export interface StartTarget {
  seriesId: number;
  day: number | null;
}

/** What the gallery knows at launch, in the order it is consulted. */
export interface StartHints {
  /** An "Open gallery" press in chat, from the server. */
  intent: LaunchIntent | null;
  /** The activity link's `custom_id`, parsed. */
  link: LaunchIntent | null;
  /** The channel leaf was launched from. */
  channelId: string | null;
  /** The series last opened in this server. */
  remembered: number | null;
}

/** Whether the series can be opened: its days load. A revoked one 404s. */
export function isNavigable(series: Series): boolean {
  return series.state !== 'revoked';
}

/**
 * The series (and day) a request names, if this viewer can open it. A day
 * past the newest archived one opens the series home instead.
 */
export function resolveTarget(
  series: readonly Series[],
  wanted: LaunchIntent | null,
): StartTarget | null {
  if (!wanted) return null;
  const found = series.find((s) => s.id === wanted.seriesId);
  if (!found || !isNavigable(found)) return null;
  const day = wanted.day !== null && wanted.day <= (found.max_day ?? 0) ? wanted.day : null;
  return { seriesId: found.id, day };
}

/**
 * Where the gallery should open: a press in chat, then the activity link,
 * then the one series that lives in the launch channel, then the series
 * last opened here, then the only series there is. `null` is the picker.
 */
export function startTarget(series: readonly Series[], hints: StartHints): StartTarget | null {
  const asked = resolveTarget(series, hints.intent) ?? resolveTarget(series, hints.link);
  if (asked) return asked;
  const open = series.filter(isNavigable);
  const { channelId } = hints;
  if (channelId !== null) {
    const here = open.filter((s) => s.channel_ids?.includes(channelId) ?? false);
    const [only] = here;
    if (here.length === 1 && only) return { seriesId: only.id, day: null };
  }
  const remembered = open.find((s) => s.id === hints.remembered);
  if (remembered) return { seriesId: remembered.id, day: null };
  const [only] = open;
  return open.length === 1 && only ? { seriesId: only.id, day: null } : null;
}

/**
 * The stack for a target: the picker at the bottom (so Back always reaches
 * the full list and "Start a series"), the series home, then the day.
 */
export function stackFor(target: StartTarget | null): [View, ...View[]] {
  if (!target) return [PICKER];
  const home: View = { name: 'home', seriesId: target.seriesId };
  if (target.day === null) return [PICKER, home];
  return [PICKER, home, { name: 'viewer', seriesId: target.seriesId, day: target.day }];
}
