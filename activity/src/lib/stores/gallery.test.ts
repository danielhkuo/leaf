import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Session } from '../sdk/handshake';
import type { Day, DaySummary, Eligibility, Series } from '../types/api';
import type * as ErrorsModule from '../utils/errors';
import type * as StoreModule from './gallery.svelte';

// The store's module state (client, caches) is per import, so each test loads
// a fresh copy. The API client is mocked at its module boundary.
const client = vi.hoisted(() => ({
  options: [] as { token: string; expiresAt?: number; onUnauthorized?: () => void }[],
  listSeries: vi.fn(),
  getEligibility: vi.fn(),
  listDays: vi.fn(),
  getDay: vi.fn(),
  getLaunchIntent: vi.fn(),
  ensureFresh: vi.fn(),
  dispose: vi.fn(),
}));

vi.mock('../api/client', () => ({
  LeafApi: class {
    listSeries = client.listSeries;
    getEligibility = client.getEligibility;
    listDays = client.listDays;
    getDay = client.getDay;
    getLaunchIntent = client.getLaunchIntent;
    ensureFresh = client.ensureFresh;
    dispose = client.dispose;
    constructor(opts: { token: string }) {
      client.options.push(opts);
    }
  },
}));

type Store = typeof StoreModule;
type Errors = typeof ErrorsModule;

const SESSION: Session = {
  user: { id: 'u1', username: 'ann' },
  guildId: 'g1',
  channelId: 'c1',
  platform: 'desktop',
  appName: 'leaf',
  customId: null,
  token: 'tok',
  expiresAt: 1_000_000,
};

function series(id: number, extra: Partial<Series> = {}): Series {
  return {
    id,
    name: `Series ${id}`,
    description: '',
    creator_id: 'u1',
    cadence: 'daily',
    emoji: '🍃',
    start_day: 1,
    max_day: 3,
    ...extra,
  };
}

const CAN_CREATE: Eligibility = { can_create: true, violations: [] };

/** A whole-index row, as a current server sends it. */
function row(day: number): DaySummary {
  return { day, posted_at: day * 100, thumb_url: null, local_date: '2026-10-01' };
}

/** A row from an older server: no `local_date`. */
function oldRow(day: number): DaySummary {
  return { day, posted_at: day * 100, thumb_url: null };
}

function dayData(day: number): Day {
  return { day, caption: `Day ${day}`, posted_at: day * 100, jump_url: '', media: [] };
}

/**
 * A Map-backed `localStorage`. Stubbed rather than borrowed from jsdom: newer
 * Node versions define their own `localStorage` global, which is unusable
 * without a CLI flag and hides jsdom's.
 */
function memoryStorage(): Pick<Storage, 'getItem' | 'setItem' | 'clear'> {
  const items = new Map<string, string>();
  return {
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => void items.set(key, value),
    clear: () => items.clear(),
  };
}

let store: Store;
// Loaded after each module reset so `instanceof` matches the store's copy.
let ApiError: Errors['ApiError'];

beforeEach(async () => {
  vi.resetModules();
  client.options.length = 0;
  for (const fn of Object.values(client)) {
    if (typeof fn === 'function') fn.mockReset();
  }
  client.listSeries.mockResolvedValue([series(1)]);
  client.getEligibility.mockResolvedValue(CAN_CREATE);
  client.getLaunchIntent.mockResolvedValue(null);
  client.ensureFresh.mockResolvedValue(undefined);
  vi.stubGlobal('localStorage', memoryStorage());
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  store = await import('./gallery.svelte');
  ({ ApiError } = await import('../utils/errors'));
});

describe('initGallery', () => {
  it('loads the series list and eligibility', async () => {
    await store.initGallery(SESSION);

    expect(store.gallery.status).toBe('ready');
    expect(store.gallery.series).toHaveLength(1);
    expect(store.gallery.eligibilityStatus).toBe('ready');
    expect(store.getGuildId()).toBe('g1');
    expect(client.options[0]).toMatchObject({ token: 'tok', expiresAt: 1_000_000 });
  });

  it('adopts a load the boot started early instead of fetching twice', async () => {
    store.preloadGallery({ guildId: 'g1', token: 'tok', expiresAt: 1_000_000 });
    await store.initGallery(SESSION);

    expect(store.gallery.status).toBe('ready');
    expect(client.listSeries).toHaveBeenCalledOnce();
    expect(client.options).toHaveLength(1);

    // A later call is a retry and loads again.
    await store.initGallery(SESSION);
    expect(client.listSeries).toHaveBeenCalledTimes(2);
  });

  it('ignores an early load made for another token', async () => {
    store.preloadGallery({ guildId: 'g1', token: 'older', expiresAt: 1_000_000 });
    await vi.waitFor(() => expect(store.gallery.status).toBe('ready'));
    await store.initGallery(SESSION);

    expect(client.listSeries).toHaveBeenCalledTimes(2);
    expect(client.options.map((o) => o.token)).toEqual(['older', 'tok']);
    expect(store.gallery.status).toBe('ready');
  });

  it('explains a launch outside a server without offering a retry', async () => {
    await store.initGallery({ ...SESSION, guildId: null });

    expect(store.gallery.status).toBe('error');
    expect(store.gallery.errorKind).toBe('no_guild');
    expect(store.gallery.error).toContain('inside a server');
    expect(client.options).toHaveLength(0);
  });

  it('reports a failed load as one sentence, never the request label', async () => {
    client.listSeries.mockRejectedValue(new ApiError(500, 'GET /guilds/g1/series → 500'));

    await store.initGallery(SESSION);

    expect(store.gallery.status).toBe('error');
    expect(store.gallery.errorKind).toBe('server');
    expect(store.gallery.error).not.toContain('GET');
    expect(store.gallery.error).toMatch(/Try again/);
  });

  it('keeps the gallery usable when only eligibility fails', async () => {
    client.getEligibility.mockRejectedValue(new ApiError(500, 'x'));

    await store.initGallery(SESSION);

    expect(store.gallery.status).toBe('ready');
    expect(store.gallery.eligibilityStatus).toBe('failed');
  });

  it('reuses the client on a retry so a renewed token is not thrown away', async () => {
    client.listSeries.mockRejectedValueOnce(new ApiError(0, 'x', undefined, { kind: 'network' }));

    await store.initGallery(SESSION);
    await store.initGallery(SESSION);

    expect(store.gallery.status).toBe('ready');
    expect(client.options).toHaveLength(1);
  });

  it('switches to the expired state when the server refuses the session', async () => {
    client.listSeries.mockImplementation(() => {
      client.options[0]?.onUnauthorized?.();
      return Promise.reject(new ApiError(401, 'x'));
    });

    await store.initGallery(SESSION);

    expect(store.gallery.status).toBe('expired');
  });
});

describe('refreshAll', () => {
  it('refetches the list, drops cached days and bumps the epoch', async () => {
    await store.initGallery(SESSION);
    client.listDays.mockResolvedValue([row(1)]);
    client.getDay.mockResolvedValue(dayData(1));
    await store.loadDaysIndex(1, 3);
    await store.getDay(1, 1);

    client.listSeries.mockResolvedValue([series(1, { max_day: 4 })]);
    client.listDays.mockResolvedValue([row(1), row(4)]);
    const before = store.gallery.epoch;

    await expect(store.refreshAll()).resolves.toBe(true);

    expect(client.ensureFresh).toHaveBeenCalledTimes(1);
    expect(store.gallery.epoch).toBe(before + 1);
    expect(store.gallery.series[0]?.max_day).toBe(4);
    expect(store.gallery.refreshing).toBe(false);
    await expect(store.loadDaysIndex(1, 4)).resolves.toHaveLength(2);
    await store.getDay(1, 1);
    expect(client.getDay).toHaveBeenCalledTimes(2);
  });

  it('refreshes eligibility too, so a newly granted role shows up', async () => {
    client.getEligibility.mockResolvedValue({ can_create: false, violations: [] });
    await store.initGallery(SESSION);
    client.getEligibility.mockResolvedValue(CAN_CREATE);

    await store.refreshAll();

    expect(store.gallery.eligibility?.can_create).toBe(true);
  });

  it('leaves everything in place when the list cannot be fetched', async () => {
    await store.initGallery(SESSION);
    client.listDays.mockResolvedValue([row(1)]);
    await store.loadDaysIndex(1, 3);
    client.listSeries.mockRejectedValue(new ApiError(0, 'x', undefined, { kind: 'network' }));

    await expect(store.refreshAll()).resolves.toBe(false);

    expect(store.gallery.status).toBe('ready');
    expect(store.gallery.epoch).toBe(0);
    expect(store.gallery.series).toHaveLength(1);
    await store.loadDaysIndex(1, 3);
    expect(client.listDays).toHaveBeenCalledTimes(1);
  });

  it('shares one run between overlapping calls', async () => {
    await store.initGallery(SESSION);

    await Promise.all([store.refreshAll(), store.refreshAll()]);

    // Once at init, once for the two refreshes.
    expect(client.listSeries).toHaveBeenCalledTimes(2);
    expect(store.gallery.epoch).toBe(1);
  });

  it('skips the work when the last load is recent enough', async () => {
    await store.initGallery(SESSION);

    await expect(store.refreshAll({ ifOlderThanMs: 30_000 })).resolves.toBe(true);

    expect(client.listSeries).toHaveBeenCalledTimes(1);
    expect(store.gallery.epoch).toBe(0);
  });

  it('still renews a due session when the data is recent enough to skip', async () => {
    await store.initGallery(SESSION);

    await store.refreshAll({ ifOlderThanMs: 30_000 });

    expect(client.ensureFresh).toHaveBeenCalledTimes(1);
  });

  it('does nothing before the gallery has loaded', async () => {
    await expect(store.refreshAll()).resolves.toBe(false);
  });

  it('does not let a fetch that began before the refresh repopulate the cache', async () => {
    await store.initGallery(SESSION);
    let release: (rows: DaySummary[]) => void = () => undefined;
    client.listDays.mockReturnValueOnce(
      new Promise<DaySummary[]>((resolve) => {
        release = resolve;
      }),
    );
    const stale = store.loadDaysIndex(1, 3);

    await store.refreshAll();
    release([row(1)]);
    await stale;

    client.listDays.mockResolvedValue([row(1), row(2)]);
    await expect(store.loadDaysIndex(1, 3)).resolves.toHaveLength(2);
  });
});

describe('loadDaysIndex', () => {
  beforeEach(async () => {
    await store.initGallery(SESSION);
  });

  it('asks for the whole index in one request and caches it', async () => {
    client.listDays.mockResolvedValue([row(1), row(2)]);

    const first = await store.loadDaysIndex(1, 900);
    const second = await store.loadDaysIndex(1, 900);

    expect(first).toHaveLength(2);
    expect(second).toBe(first);
    expect(client.listDays).toHaveBeenCalledTimes(1);
    expect(client.listDays).toHaveBeenCalledWith('g1', 1);
  });

  it('never caches an empty index', async () => {
    client.listDays.mockResolvedValue([]);
    await expect(store.loadDaysIndex(1, 0)).resolves.toEqual([]);

    client.listDays.mockResolvedValue([row(1)]);
    await expect(store.loadDaysIndex(1, 0)).resolves.toHaveLength(1);
  });

  it('takes an empty answer as the whole index, whatever max_day says', async () => {
    client.listDays.mockResolvedValue([]);

    // A stale max_day (the only day was just removed) must not start paging.
    await expect(store.loadDaysIndex(1, 800)).resolves.toEqual([]);
    expect(client.listDays).toHaveBeenCalledTimes(1);
  });

  it('shares one request between callers that ask at the same time', async () => {
    client.listDays.mockResolvedValue([row(1)]);

    const [a, b] = await Promise.all([store.loadDaysIndex(1, 3), store.loadDaysIndex(1, 3)]);

    expect(a).toBe(b);
    expect(client.listDays).toHaveBeenCalledTimes(1);
  });

  it('does not cache a failure', async () => {
    client.listDays.mockRejectedValueOnce(new ApiError(500, 'x'));
    await expect(store.loadDaysIndex(1, 3)).rejects.toBeInstanceOf(ApiError);

    client.listDays.mockResolvedValue([row(1)]);
    await expect(store.loadDaysIndex(1, 3)).resolves.toHaveLength(1);
  });

  it('pages an older server in 366-day windows up to max_day', async () => {
    client.listDays.mockImplementation(
      (_gid: string, _sid: number, range?: { from: number; to: number }) =>
        Promise.resolve(range ? [oldRow(range.from)] : [oldRow(700)]),
    );

    const rows = await store.loadDaysIndex(1, 800);

    expect(rows.map((r) => r.day)).toEqual([1, 367, 733]);
    expect(client.listDays).toHaveBeenCalledWith('g1', 1, { from: 1, to: 366 });
    expect(client.listDays).toHaveBeenCalledWith('g1', 1, { from: 367, to: 732 });
    expect(client.listDays).toHaveBeenCalledWith('g1', 1, { from: 733, to: 800 });
    expect(client.listDays).toHaveBeenCalledTimes(4);
  });

  it('treats an older server’s 400 for an empty series as no days', async () => {
    client.listDays.mockRejectedValue(new ApiError(400, 'x', 'bad_request'));

    await expect(store.loadDaysIndex(1, 0)).resolves.toEqual([]);
    expect(client.listDays).toHaveBeenCalledTimes(1);
  });

  it('caps the paging when one day number is absurdly high', async () => {
    client.listDays.mockImplementation(
      (_gid: string, _sid: number, range?: { from: number; to: number }) =>
        Promise.resolve(range ? [] : [oldRow(20_261_001)]),
    );

    await store.loadDaysIndex(1, 20_261_001);

    // The probe, 60 windows from day 1, and the window holding the last day.
    expect(client.listDays).toHaveBeenCalledTimes(62);
    const lastCall = client.listDays.mock.calls.at(-1) as [string, number, { to: number }];
    expect(lastCall[2].to).toBe(20_261_001);
  });
});

describe('getDay', () => {
  beforeEach(async () => {
    await store.initGallery(SESSION);
  });

  it('caches a day and exposes it synchronously', async () => {
    client.getDay.mockResolvedValue(dayData(5));
    expect(store.peekDay(1, 5)).toBeNull();

    const first = await store.getDay(1, 5);
    const second = await store.getDay(1, 5);

    expect(second).toBe(first);
    expect(store.peekDay(1, 5)).toBe(first);
    expect(client.getDay).toHaveBeenCalledTimes(1);
    expect(client.getDay).toHaveBeenCalledWith('g1', 1, 5);
  });

  it('refetches when asked for a fresh copy', async () => {
    client.getDay.mockResolvedValue(dayData(5));
    await store.getDay(1, 5);
    await store.getDay(1, 5, { fresh: true });

    expect(client.getDay).toHaveBeenCalledTimes(2);
  });

  it('does not cache a failure', async () => {
    client.getDay.mockRejectedValueOnce(new ApiError(404, 'x'));
    await expect(store.getDay(1, 5)).rejects.toMatchObject({ kind: 'not_found' });

    client.getDay.mockResolvedValue(dayData(5));
    await expect(store.getDay(1, 5)).resolves.toMatchObject({ day: 5 });
  });

  it('keeps the cache bounded, dropping the oldest day first', async () => {
    client.getDay.mockImplementation((_g: string, _s: number, day: number) =>
      Promise.resolve(dayData(day)),
    );
    for (let day = 1; day <= 61; day += 1) await store.getDay(1, day);

    expect(store.peekDay(1, 1)).toBeNull();
    expect(store.peekDay(1, 2)).not.toBeNull();
    expect(store.peekDay(1, 61)).not.toBeNull();
  });
});

describe('takeLaunchIntent', () => {
  it('hands out the intent fetched with the series list, once', async () => {
    client.getLaunchIntent.mockResolvedValueOnce({ series_id: 4, day: 12 });
    await store.initGallery(SESSION);

    await expect(store.takeLaunchIntent()).resolves.toEqual({ seriesId: 4, day: 12 });
    expect(client.getLaunchIntent).toHaveBeenCalledTimes(1);

    // Later calls (return to the foreground) ask the server again.
    await expect(store.takeLaunchIntent()).resolves.toBeNull();
    expect(client.getLaunchIntent).toHaveBeenCalledTimes(2);
  });

  it('reads a missing day as "open the series"', async () => {
    client.getLaunchIntent.mockResolvedValueOnce({ series_id: 4, day: undefined });
    await store.initGallery(SESSION);

    await expect(store.takeLaunchIntent()).resolves.toEqual({ seriesId: 4, day: null });
  });

  it('keeps an intent across a retried first load', async () => {
    client.getLaunchIntent.mockResolvedValueOnce({ series_id: 4, day: 12 });
    client.listSeries.mockRejectedValueOnce(new ApiError(500, 'x'));

    await store.initGallery(SESSION);
    await store.initGallery(SESSION);

    await expect(store.takeLaunchIntent()).resolves.toEqual({ seriesId: 4, day: 12 });
    expect(client.getLaunchIntent).toHaveBeenCalledTimes(1);
  });

  it('is null, and the gallery still loads, on a server with no such route', async () => {
    client.getLaunchIntent.mockRejectedValue(new ApiError(404, 'x'));
    await store.initGallery(SESSION);

    expect(store.gallery.status).toBe('ready');
    await expect(store.takeLaunchIntent()).resolves.toBeNull();
  });

  it('keeps asking after a passing failure', async () => {
    client.getLaunchIntent.mockRejectedValueOnce(
      new ApiError(0, 'x', undefined, { kind: 'network' }),
    );
    await store.initGallery(SESSION);
    await expect(store.takeLaunchIntent()).resolves.toBeNull();

    client.getLaunchIntent.mockResolvedValueOnce({ series_id: 2, day: 1 });
    await expect(store.takeLaunchIntent()).resolves.toEqual({ seriesId: 2, day: 1 });
  });

  it('stops waiting on a slow answer so the first view is not held up', async () => {
    client.getLaunchIntent.mockReturnValue(new Promise(() => undefined));
    await store.initGallery(SESSION);
    vi.useFakeTimers();
    try {
      const taken = store.takeLaunchIntent();
      await vi.advanceTimersByTimeAsync(3_000);
      await expect(taken).resolves.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('waits out a slow answer on a return to the foreground', async () => {
    await store.initGallery(SESSION);
    await store.takeLaunchIntent();
    vi.useFakeTimers();
    try {
      // The server forgets the intent as it answers: a dropped answer is lost.
      client.getLaunchIntent.mockReturnValueOnce(
        new Promise((resolve) => setTimeout(() => resolve({ series_id: 4, day: 12 }), 12_000)),
      );
      let settled = false;
      const taken = store.takeLaunchIntent().finally(() => {
        settled = true;
      });

      // Past the first view's limit, and past a timed-out attempt and retry.
      await vi.advanceTimersByTimeAsync(11_999);
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      await expect(taken).resolves.toEqual({ seriesId: 4, day: 12 });
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops waiting on the foreground once the press is long past', async () => {
    await store.initGallery(SESSION);
    await store.takeLaunchIntent();
    client.getLaunchIntent.mockReturnValueOnce(new Promise(() => undefined));
    vi.useFakeTimers();
    try {
      let settled = false;
      const taken = store.takeLaunchIntent().finally(() => {
        settled = true;
      });
      await vi.advanceTimersByTimeAsync(14_999);
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      await expect(taken).resolves.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('is null before the gallery has loaded', async () => {
    await expect(store.takeLaunchIntent()).resolves.toBeNull();
  });
});

describe('adoptSeries', () => {
  beforeEach(async () => {
    await store.initGallery(SESSION);
  });

  it('adds a full series without another request', async () => {
    await expect(store.adoptSeries(series(9, { max_day: null, state: 'sprout' }))).resolves.toBe(
      true,
    );

    expect(store.gallery.series.map((s) => s.id)).toEqual([1, 9]);
    expect(store.gallery.series[1]).toMatchObject({ state: 'sprout', max_day: null });
    expect(client.listSeries).toHaveBeenCalledTimes(1);
  });

  it('replaces a series it already has (a repeated create)', async () => {
    await store.adoptSeries(series(1, { name: 'Renamed' }));

    expect(store.gallery.series).toHaveLength(1);
    expect(store.gallery.series[0]?.name).toBe('Renamed');
  });

  it('refetches the list when an older server returns a partial series', async () => {
    client.listSeries.mockResolvedValue([series(1), series(9)]);

    await expect(store.adoptSeries({ id: 9, name: 'New', emoji: '🍃' })).resolves.toBe(true);

    expect(client.listSeries).toHaveBeenCalledTimes(2);
    expect(store.gallery.series).toHaveLength(2);
  });

  it('reports when the series could not be put in the list', async () => {
    client.listSeries.mockRejectedValue(new ApiError(500, 'x'));

    await expect(store.adoptSeries({ id: 9, name: 'New', emoji: '🍃' })).resolves.toBe(false);
  });
});

describe('refreshEligibility', () => {
  it('updates the answer and keeps the old one on failure', async () => {
    await store.initGallery(SESSION);
    client.getEligibility.mockResolvedValue({ can_create: false, violations: [] });

    await store.refreshEligibility();
    expect(store.gallery.eligibility?.can_create).toBe(false);

    client.getEligibility.mockRejectedValue(new ApiError(500, 'x'));
    await store.refreshEligibility();
    expect(store.gallery.eligibility?.can_create).toBe(false);
    expect(store.gallery.eligibilityStatus).toBe('ready');
  });
});

describe('last series', () => {
  it('remembers per server', async () => {
    await store.initGallery(SESSION);
    store.rememberSeries(7);

    expect(store.lastSeries()).toBe(7);
    expect(localStorage.getItem('leaf:lastSeries:g1')).toBe('7');
  });

  it('falls back to the value saved before keys were per server', async () => {
    localStorage.setItem('leaf:lastSeries', '3');
    await store.initGallery(SESSION);

    expect(store.lastSeries()).toBe(3);
  });

  it('ignores a value that is not a series id', async () => {
    await store.initGallery(SESSION);
    localStorage.setItem('leaf:lastSeries:g1', 'abc');

    expect(store.lastSeries()).toBeNull();
  });
});
