// Gallery-wide state: the authed API client, the guild context, the series
// list, and the caches the views share. Initialized from the session after
// the handshake; `refreshAll()` brings everything up to date afterwards.

import type { LeafApi } from '../api/client';
import type { Session, SessionAccess } from '../sdk/handshake';
import type {
  CreatedSeries,
  Day,
  DaySummary,
  Eligibility,
  LaunchIntent,
  Series,
} from '../types/api';
import { ApiError, describeError, errorKind, type ApiErrorKind } from '../utils/errors';

let api: LeafApi | null = null;
let guildId = '';
/** The session token the client was built from (it renews its own copy). */
let bootToken = '';

interface GalleryState {
  /**
   * `expired`: the server no longer accepts this session and it cannot be
   * renewed. Nothing will load again; say "close leaf and open it again".
   */
  status: 'loading' | 'ready' | 'error' | 'expired';
  series: Series[];
  /** One sentence for a failed first load, fit to show as it is. */
  error: string;
  /** Why the first load failed. `no_guild` cannot be fixed by a retry. */
  errorKind: ApiErrorKind | 'unknown' | 'no_guild' | null;
  /** Whether the viewer may start a series; loaded with the series list. */
  eligibility: Eligibility | null;
  eligibilityStatus: 'loading' | 'ready' | 'failed';
  /**
   * Bumped whenever cached data was thrown away ({@link refreshAll}). Views
   * read it inside their load effects so they refetch stats, index and day.
   */
  epoch: number;
  /** True while {@link refreshAll} is fetching. */
  refreshing: boolean;
}

export const gallery = $state<GalleryState>({
  status: 'loading',
  series: [],
  error: '',
  errorKind: null,
  eligibility: null,
  eligibilityStatus: 'loading',
  epoch: 0,
  refreshing: false,
});

/** The authed API client. Throws if used before {@link initGallery}. */
export function getApi(): LeafApi {
  if (!api) throw new Error('gallery API used before initialization');
  return api;
}

/** The active guild id. */
export function getGuildId(): string {
  return guildId;
}

// --- caches ---------------------------------------------------------------

/** Matches the API's MAX_WINDOW cap on a single ranged `/days` request. */
const DAY_WINDOW = 366;
/** Ranged requests in flight at once when paging an older server. */
const PAGE_CONCURRENCY = 4;
/** Most windows paged from day 1, so one mistyped day number stays cheap. */
const MAX_WINDOWS = 60;
/**
 * Cached rows carry signed media URLs, which the server mints for at least a
 * day. Six hours (one session lifetime) keeps every cached URL well inside it.
 */
const CACHE_MAX_AGE_MS = 6 * 3600 * 1000;
/** Days kept for instant prev/next; the oldest entry is dropped first. */
const DAY_CACHE_MAX = 60;
/**
 * Longest {@link takeLaunchIntent} holds the first view for the answer that
 * was requested with the series list.
 */
const INTENT_BOOT_WAIT_MS = 3_000;
/**
 * Longest it waits on a return to the foreground, where no view is held up:
 * room for a request that hung on a connection dropped in the background
 * (the client gives up on it after 10 s) and for its retry to answer.
 *
 * The retry recovers a request that never reached the server. One whose
 * answer was lost on the way back is recovered only by a server that
 * honours the call's `attempt` id (see `getLaunchIntent`); otherwise the
 * server has forgotten the intent, the retry reads `null`, and the press in
 * chat is dropped.
 */
const INTENT_FOREGROUND_WAIT_MS = 15_000;

interface Cached<T> {
  value: T;
  at: number;
}

const indexCache = new Map<number, Cached<DaySummary[]>>();
const indexInflight = new Map<number, Promise<DaySummary[]>>();
const dayCache = new Map<string, Cached<Day>>();
const dayInflight = new Map<string, Promise<Day>>();
/**
 * Counts cache clears. A fetch that started before a clear must not write its
 * (possibly stale) result back afterwards.
 */
let generation = 0;
/** When the series list last loaded, for `refreshAll({ ifOlderThanMs })`. */
let lastLoadedAt = 0;

/** A launch intent requested at boot and not yet handed to the view. */
let pendingIntent: Promise<LaunchIntent | null> | null = null;

function unexpired<T>(entry: Cached<T> | undefined): T | undefined {
  return entry && Date.now() - entry.at < CACHE_MAX_AGE_MS ? entry.value : undefined;
}

function clearCaches(): void {
  generation += 1;
  indexCache.clear();
  indexInflight.clear();
  dayCache.clear();
  dayInflight.clear();
}

// --- loading --------------------------------------------------------------

function markExpired(): void {
  gallery.status = 'expired';
}

/** Read through a function so an earlier assignment doesn't narrow the type. */
function isExpired(): boolean {
  return gallery.status === 'expired';
}

function applyEligibility(eligibility: Eligibility | null): void {
  if (eligibility) {
    gallery.eligibility = eligibility;
    gallery.eligibilityStatus = 'ready';
  } else if (!gallery.eligibility) {
    gallery.eligibilityStatus = 'failed';
  }
  // A failed refetch keeps the last known answer rather than hiding the CTA.
}

/** A load {@link preloadGallery} started, for the next {@link initGallery}. */
let preloaded: { token: string; done: Promise<void> } | null = null;

/**
 * Starts the first load before the session is complete. The boot calls it
 * as soon as the leaf token exists, while Discord is still confirming the
 * user, so the series list is already on its way when the gallery mounts.
 * The next {@link initGallery} for the same token adopts this load instead
 * of starting another.
 */
export function preloadGallery(access: SessionAccess): void {
  preloaded = { token: access.token, done: loadGallery(access) };
}

/**
 * Builds the API client and loads the visible series for the session's
 * guild. Safe to call again to retry a failed load: the client (and the
 * token it has renewed since) is kept while the session is the same.
 *
 * On failure `gallery.status` is `error` with a sentence in `gallery.error`,
 * or `expired` when the session itself was refused.
 */
export function initGallery(session: Session): Promise<void> {
  const early = preloaded;
  preloaded = null;
  return early?.token === session.token ? early.done : loadGallery(session);
}

async function loadGallery(session: SessionAccess): Promise<void> {
  gallery.status = 'loading';
  gallery.error = '';
  gallery.errorKind = null;
  if (!session.guildId) {
    gallery.status = 'error';
    gallery.errorKind = 'no_guild';
    gallery.error =
      'leaf’s gallery opens from inside a server. Close this, go to a server channel and open leaf from there.';
    return;
  }
  try {
    if (!api || bootToken !== session.token || guildId !== session.guildId) {
      // The client, its schemas and zod stay out of the initial chunk. The
      // SDK chunk loads the same module for the handshake, so by now this
      // resolves from memory.
      const { LeafApi: Client } = await import('../api/client');
      api?.dispose();
      api = new Client({
        token: session.token,
        expiresAt: session.expiresAt,
        onUnauthorized: markExpired,
      });
      bootToken = session.token;
      guildId = session.guildId;
      clearCaches();
      pendingIntent = null;
      gallery.eligibility = null;
    }
    const client = api;
    gallery.eligibilityStatus = gallery.eligibility ? 'ready' : 'loading';
    // Asked for alongside the list so opening on a day costs no extra round
    // trip. On a retry, an intent an earlier attempt already holds is kept:
    // the server has forgotten it by now.
    pendingIntent = (pendingIntent ?? Promise.resolve(null)).then(
      (found) => found ?? fetchIntent(client),
    );
    const [series, eligibility] = await Promise.all([
      client.listSeries(guildId),
      client.getEligibility(guildId).catch(() => null),
    ]);
    gallery.series = series;
    applyEligibility(eligibility);
    lastLoadedAt = Date.now();
    gallery.status = 'ready';
  } catch (e) {
    console.error('leaf: loading the gallery failed', e);
    // A refused session is not a load error: there is nothing to retry.
    if (isExpired() || errorKind(e) === 'unauthorized') {
      markExpired();
      return;
    }
    gallery.status = 'error';
    gallery.errorKind = errorKind(e);
    gallery.error = describeError(e);
  }
}

/**
 * Re-fetches the visible series list (after an edit) so the picker and home
 * reflect the change. Best-effort: a failure keeps the stale list rather
 * than blanking the gallery. Resolves to whether the list was refreshed.
 */
export async function refreshSeries(): Promise<boolean> {
  if (!api) return false;
  try {
    gallery.series = await api.listSeries(guildId);
    return true;
  } catch {
    /* keep the existing list on a transient failure */
    return false;
  }
}

/**
 * Re-checks whether the viewer may start a series (after creating one, or
 * when a role may have been granted). A failure keeps the last answer.
 */
export async function refreshEligibility(): Promise<void> {
  if (!api) return;
  applyEligibility(await api.getEligibility(guildId).catch(() => null));
}

let refreshInflight: Promise<boolean> | null = null;

/**
 * Brings the whole gallery up to date: renews the session if it is due,
 * refetches the series list and eligibility, throws away the cached day
 * index and days, and bumps `gallery.epoch` so open views reload. Call it
 * when the Activity returns to the foreground and from a refresh control.
 *
 * `ifOlderThanMs` skips the work when the last successful load is more
 * recent (for automatic triggers; leave it out for a button). Resolves to
 * `false` when the series list could not be fetched; everything on screen
 * is then left as it was.
 */
export function refreshAll(opts: { ifOlderThanMs?: number } = {}): Promise<boolean> {
  const client = api;
  if (!client || gallery.status !== 'ready') return Promise.resolve(false);
  if (opts.ifOlderThanMs !== undefined && Date.now() - lastLoadedAt < opts.ifOlderThanMs) {
    // The data is recent enough, but the session may still be due a renewal.
    void client.ensureFresh();
    return Promise.resolve(true);
  }
  refreshInflight ??= runRefresh(client).finally(() => {
    refreshInflight = null;
    gallery.refreshing = false;
  });
  return refreshInflight;
}

async function runRefresh(client: LeafApi): Promise<boolean> {
  gallery.refreshing = true;
  await client.ensureFresh();
  let series: Series[];
  let eligibility: Eligibility | null;
  try {
    [series, eligibility] = await Promise.all([
      client.listSeries(guildId),
      client.getEligibility(guildId).catch(() => null),
    ]);
  } catch (e) {
    console.error('leaf: refreshing the gallery failed', e);
    return false;
  }
  // The client was replaced while this ran (a new session): drop the result.
  if (client !== api) return false;
  gallery.series = series;
  applyEligibility(eligibility);
  clearCaches();
  lastLoadedAt = Date.now();
  gallery.epoch += 1;
  return true;
}

/**
 * Puts a just-created series into the list so the app can navigate to it
 * without waiting on a second request. An older server returns only part of
 * the series; the list is refetched then. Resolves to whether the series is
 * now in `gallery.series`.
 */
export async function adoptSeries(created: CreatedSeries): Promise<boolean> {
  const { description, creator_id, cadence, start_day } = created;
  if (
    description === undefined ||
    creator_id === undefined ||
    cadence === undefined ||
    start_day === undefined
  ) {
    await refreshSeries();
    return gallery.series.some((s) => s.id === created.id);
  }
  const series: Series = {
    ...created,
    description,
    creator_id,
    cadence,
    start_day,
    max_day: created.max_day ?? null,
  };
  const at = gallery.series.findIndex((s) => s.id === series.id);
  if (at === -1) gallery.series.push(series);
  else gallery.series[at] = series;
  return true;
}

// --- day index ------------------------------------------------------------

/**
 * The full ordered present-day list (with thumbnails) for a series, cached
 * per id until the next {@link refreshAll}. Feeds the calendar, the day
 * viewer's gap-aware prev/next and its adjacent-thumbnail preload.
 *
 * One request against a current server. `maxDay` (the series' `max_day`) is
 * only used to page an older server that has no whole-index response. An
 * empty result is never cached, so a first archive shows up on the next call.
 */
export function loadDaysIndex(seriesId: number, maxDay: number): Promise<DaySummary[]> {
  const cached = unexpired(indexCache.get(seriesId));
  if (cached) return Promise.resolve(cached);
  const pending = indexInflight.get(seriesId);
  if (pending) return pending;

  const started = generation;
  const request = fetchIndex(getApi(), getGuildId(), seriesId, maxDay)
    .then((rows) => {
      if (rows.length > 0 && started === generation) {
        indexCache.set(seriesId, { value: rows, at: Date.now() });
      }
      return rows;
    })
    .finally(() => {
      if (indexInflight.get(seriesId) === request) indexInflight.delete(seriesId);
    });
  indexInflight.set(seriesId, request);
  return request;
}

async function fetchIndex(
  client: LeafApi,
  gid: string,
  seriesId: number,
  maxDay: number,
): Promise<DaySummary[]> {
  let rows: DaySummary[];
  try {
    rows = await client.listDays(gid, seriesId);
  } catch (e) {
    // An older server answers 400 for a series with nothing archived (its
    // default window is empty). Anything else is a real failure.
    if (e instanceof ApiError && e.status === 400) return [];
    throw e;
  }
  // An empty answer is the whole index of an empty series (an older server
  // never sends one), and only a whole-index response carries `local_date`.
  // Indexed rather than `rows.at(-1)`: the iOS 15.0-15.3 webview has no `at`.
  const newest = rows[rows.length - 1];
  if (!newest || newest.local_date !== undefined) return rows;

  // Older server: `rows` is just its default window of recent days. Page the
  // full range in ranged requests, a few at a time.
  const last = Math.max(maxDay, newest.day);
  const starts: number[] = [];
  for (let from = 1; from <= last && starts.length < MAX_WINDOWS; from += DAY_WINDOW) {
    starts.push(from);
  }
  // Past the cap, still fetch the window holding the highest day.
  const tail = last - ((last - 1) % DAY_WINDOW);
  if (!starts.includes(tail)) starts.push(tail);

  const all: DaySummary[] = [];
  for (let i = 0; i < starts.length; i += PAGE_CONCURRENCY) {
    const batch = starts.slice(i, i + PAGE_CONCURRENCY);
    const pages = await Promise.all(
      batch.map((from) =>
        client.listDays(gid, seriesId, { from, to: Math.min(from + DAY_WINDOW - 1, last) }),
      ),
    );
    for (const page of pages) all.push(...page);
  }
  return all;
}

// --- days -----------------------------------------------------------------

function dayKey(seriesId: number, day: number): string {
  return `${seriesId}:${day}`;
}

/** A day already in the cache, for painting without a loading state. */
export function peekDay(seriesId: number, day: number): Day | null {
  return unexpired(dayCache.get(dayKey(seriesId, day))) ?? null;
}

/**
 * One day with its media, cached until the next {@link refreshAll} so
 * stepping back and forth in the viewer is instant. `fresh: true` skips the
 * cache (a Retry after the media failed to load). Failures are not cached.
 */
export function getDay(
  seriesId: number,
  day: number,
  opts: { fresh?: boolean } = {},
): Promise<Day> {
  const key = dayKey(seriesId, day);
  if (!opts.fresh) {
    const cached = unexpired(dayCache.get(key));
    if (cached) return Promise.resolve(cached);
    const pending = dayInflight.get(key);
    if (pending) return pending;
  }

  const started = generation;
  const request = getApi()
    .getDay(getGuildId(), seriesId, day)
    .then((data) => {
      if (started === generation) {
        dayCache.delete(key);
        dayCache.set(key, { value: data, at: Date.now() });
        const oldest = dayCache.keys().next();
        if (dayCache.size > DAY_CACHE_MAX && !oldest.done) dayCache.delete(oldest.value);
      }
      return data;
    })
    .finally(() => {
      if (dayInflight.get(key) === request) dayInflight.delete(key);
    });
  dayInflight.set(key, request);
  return request;
}

// --- launch intent --------------------------------------------------------

async function fetchIntent(client: LeafApi): Promise<LaunchIntent | null> {
  try {
    const intent = await client.getLaunchIntent(guildId);
    return intent ? { seriesId: intent.series_id, day: intent.day ?? null } : null;
  } catch {
    // No intent is the normal case, and an older server has no such route:
    // either way the gallery opens where it otherwise would.
    return null;
  }
}

/**
 * The series or day an "Open gallery" button in chat asked for, or `null`.
 * The server hands each intent out once and drops it after two minutes, so
 * act on the result straight away. Call it after {@link initGallery} (the
 * answer was requested with the series list) and again whenever the Activity
 * returns to the foreground. Never throws.
 *
 * The first call after a load answers within three seconds, because the
 * first view waits on it. Later calls hold nothing up and wait up to fifteen,
 * so a slow first request after a resume still opens the day that was asked
 * for; do not block a view on them.
 *
 * The series may be missing from a stale `gallery.series`; run
 * {@link refreshAll} before deciding it is not viewable.
 */
export function takeLaunchIntent(): Promise<LaunchIntent | null> {
  const stashed = pendingIntent;
  pendingIntent = null;
  const found = stashed ?? (api ? fetchIntent(api) : null);
  if (!found) return Promise.resolve(null);
  // The server has already forgotten an intent by the time it answers, so one
  // that arrives after the limit is lost. At boot that is the price of not
  // holding up the first view. On a return to the foreground the limit only
  // stops the gallery from moving long after the button was pressed.
  const waitMs = stashed ? INTENT_BOOT_WAIT_MS : INTENT_FOREGROUND_WAIT_MS;
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(null), waitMs);
    void found.then((intent) => {
      clearTimeout(timer);
      resolve(intent);
    });
  });
}

// --- last series ----------------------------------------------------------

/** Before per-server keys, one global key held the last series. */
const LEGACY_LAST_KEY = 'leaf:lastSeries';

function lastKey(): string {
  return guildId ? `${LEGACY_LAST_KEY}:${guildId}` : LEGACY_LAST_KEY;
}

/**
 * Remembers the last opened series for this server (best-effort; ignores
 * storage failures).
 */
export function rememberSeries(id: number): void {
  try {
    localStorage.setItem(lastKey(), String(id));
  } catch {
    /* private mode / disabled storage — non-fatal */
  }
}

/** The series last opened in this server, if any and still parseable. */
export function lastSeries(): number | null {
  try {
    // Series ids are global, so a value saved under the old shared key is
    // still a valid hint; the caller checks it against the visible list.
    const raw = localStorage.getItem(lastKey()) ?? localStorage.getItem(LEGACY_LAST_KEY);
    if (raw === null) return null;
    const n = Number(raw);
    return Number.isInteger(n) ? n : null;
  } catch {
    return null;
  }
}
