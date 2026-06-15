// Gallery-wide state: the authed API client, the guild context, and the
// series list. Initialized once from the session after the handshake.

import { LeafApi } from '../api/client';
import type { Session } from '../sdk/handshake';
import type { DaySummary, Series } from '../types/api';

let api: LeafApi | null = null;
let guildId = '';

interface GalleryState {
  status: 'loading' | 'ready' | 'error';
  series: Series[];
  error: string;
}

export const gallery = $state<GalleryState>({ status: 'loading', series: [], error: '' });

/** The authed API client. Throws if used before {@link initGallery}. */
export function getApi(): LeafApi {
  if (!api) throw new Error('gallery API used before initialization');
  return api;
}

/** The active guild id. */
export function getGuildId(): string {
  return guildId;
}

/** Builds the API client and loads the visible series for the session's guild. */
export async function initGallery(session: Session): Promise<void> {
  gallery.status = 'loading';
  if (!session.guildId) {
    gallery.status = 'error';
    gallery.error = 'The gallery must be launched inside a server.';
    return;
  }
  guildId = session.guildId;
  api = new LeafApi({ token: session.token });
  try {
    gallery.series = await api.listSeries(guildId);
    gallery.status = 'ready';
  } catch (e) {
    gallery.status = 'error';
    gallery.error = e instanceof Error ? e.message : String(e);
  }
}

/** Matches the API's MAX_WINDOW cap on a single `/days` range. */
const DAY_WINDOW = 366;
const indexCache = new Map<number, DaySummary[]>();

/**
 * The full ordered present-day list (with thumbnails) for a series, paged in
 * and cached per id. Feeds the day viewer's gap-aware prev/next and its
 * adjacent-thumbnail preload. Cheap — day numbers and short URLs, no images.
 */
export async function loadDaysIndex(seriesId: number, maxDay: number): Promise<DaySummary[]> {
  const cached = indexCache.get(seriesId);
  if (cached) return cached;
  const client = getApi();
  const gid = getGuildId();
  const all: DaySummary[] = [];
  for (let from = 1; from <= Math.max(1, maxDay); from += DAY_WINDOW) {
    const to = Math.min(from + DAY_WINDOW - 1, maxDay);
    if (from > to) break;
    all.push(...(await client.listDays(gid, seriesId, { from, to })));
  }
  indexCache.set(seriesId, all);
  return all;
}

const LAST_KEY = 'leaf:lastSeries';

/** Remembers the last opened series (best-effort; ignores storage failures). */
export function rememberSeries(id: number): void {
  try {
    localStorage.setItem(LAST_KEY, String(id));
  } catch {
    /* private mode / disabled storage — non-fatal */
  }
}

/** The last opened series id, if any and still parseable. */
export function lastSeries(): number | null {
  try {
    const raw = localStorage.getItem(LAST_KEY);
    if (raw === null) return null;
    const n = Number(raw);
    return Number.isInteger(n) ? n : null;
  } catch {
    return null;
  }
}
