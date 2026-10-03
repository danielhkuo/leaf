// The mock API is only useful while it answers what the real client asks
// for, in shapes the client's schemas accept. So these go through LeafApi.

import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError, LeafApi } from '../lib/api/client';
import { createMockApi, type Scenario } from './api';
import {
  dayIndex,
  eligibilityBlocked,
  GUILD_ID,
  options,
  series,
  stats,
  VIEWER_DAY,
  worstCase,
} from './fixtures';

function client(scenario: Scenario = {}): LeafApi {
  return new LeafApi({ token: 'mock-token', fetch: createMockApi(scenario), retryDelayMs: 0 });
}

/** The `ApiError` a call ends in. */
async function failure(call: Promise<unknown>): Promise<ApiError> {
  const error: unknown = await call.then(
    () => null,
    (e: unknown) => e,
  );
  if (!(error instanceof ApiError)) throw new Error('expected the call to fail with an ApiError');
  return error;
}

describe('mock gallery API', () => {
  it('answers every read the gallery makes, in a shape the client accepts', async () => {
    const api = client();

    const listed = await api.listSeries(GUILD_ID);
    expect(listed.map((s) => s.id)).toEqual(series.map((s) => s.id));
    // The optional fields survive the schema, so the views get them.
    expect(listed.find((s) => s.id === 1)?.sprout).toEqual({ archived: 2, threshold: 3 });
    expect(listed.find((s) => s.id === 9)?.state).toBe('revoked');

    expect(await api.getLaunchIntent(GUILD_ID)).toBeNull();
    expect((await api.getEligibility(GUILD_ID)).can_create).toBe(true);
    expect(await api.getOptions(GUILD_ID)).toEqual(options);
    expect(await api.getStats(GUILD_ID, 7)).toEqual(stats);

    const index = await api.listDays(GUILD_ID, 7);
    expect(index).toHaveLength(dayIndex.length);
    // Every row is dated, so the client treats it as the whole index.
    expect(index.every((row) => row.local_date !== undefined)).toBe(true);

    const day = await api.getDay(GUILD_ID, 7, VIEWER_DAY);
    expect(day.media).toHaveLength(3);
  });

  it('leaves some days without a picture', () => {
    expect(dayIndex.some((row) => row.missing && row.thumb_url === null)).toBe(true);
  });

  it('answers the owner’s own reads, and 404s them for anyone else’s series', async () => {
    const api = client();

    const mine = await api.listMySeries(GUILD_ID);
    expect(mine.map((s) => s.id)).toEqual([7, 1, 8, 9]);
    expect(mine.find((s) => s.id === 7)).toMatchObject({
      channel_name: 'daily-sketch',
      archived_days: 124,
      reminder_enabled: true,
    });

    const settings = await api.getSettings(GUILD_ID, 1);
    expect(settings).toMatchObject({ state: 'sprout', start_day: 1, reminder_error: 'dm_closed' });

    expect((await failure(api.getSettings(GUILD_ID, 2))).kind).toBe('not_found');
  });

  it('serves no days for a revoked series, as the server does', async () => {
    const api = client();
    expect((await failure(api.listDays(GUILD_ID, 9))).kind).toBe('not_found');
    expect((await failure(api.getStats(GUILD_ID, 9))).kind).toBe('not_found');
    expect((await api.getSettings(GUILD_ID, 9)).state).toBe('revoked');
  });

  it('404s a day that is not archived and a series that is not there', async () => {
    const api = client();
    // Day 124 is one of Daily Sketch's skipped numbers.
    expect((await failure(api.getDay(GUILD_ID, 7, 124))).kind).toBe('not_found');
    expect((await failure(api.listDays(GUILD_ID, 4242))).kind).toBe('not_found');
  });

  it('keeps a created series: listed, owned, with nothing archived yet', async () => {
    const api = client();
    const [channel] = options.channels;

    const created = await api.createSeries(GUILD_ID, {
      name: 'Bird Log',
      channel_id: channel!.id,
      cadence: 'weekly',
      privacy: 'creator_only',
    });
    expect(created).toMatchObject({ name: 'Bird Log', max_day: null, is_owner: true });

    expect((await api.listSeries(GUILD_ID)).some((s) => s.id === created.id)).toBe(true);
    expect((await api.listMySeries(GUILD_ID)).find((s) => s.id === created.id)).toMatchObject({
      channel_name: channel!.name,
      archived_days: 0,
    });
    expect(await api.listDays(GUILD_ID, created.id)).toEqual([]);
  });

  it('refuses a name that is taken, with the server’s code', async () => {
    const error = await failure(
      client().createSeries(GUILD_ID, {
        name: 'daily sketch',
        channel_id: options.channels[0]!.id,
        cadence: 'daily',
        privacy: 'public',
      }),
    );
    expect(error.code).toBe('name_taken');
  });

  it('applies saved settings, so the form reads back what it sent', async () => {
    const api = client();

    const stored = await api.patchSeries(GUILD_ID, 1, {
      name: 'Morning Tea',
      start_day: 2,
      reminder_dm: false,
      reminder_timezone: '',
    });
    expect(stored).toMatchObject({
      name: 'Morning Tea',
      start_day: 2,
      reminder_dm: false,
      reminder_timezone: null,
    });
    // A new route clears the recorded failure, as the server does.
    expect(stored.reminder_error).toBeUndefined();

    expect((await api.listSeries(GUILD_ID)).find((s) => s.id === 1)?.name).toBe('Morning Tea');
  });

  it('keeps a recorded reminder failure until the route really changes', async () => {
    const api = client();

    // The stored values sent again, and unrelated fields: nothing rerouted.
    const same = await api.patchSeries(GUILD_ID, 1, {
      reminder_enabled: true,
      reminder_dm: true,
      reminder_time: '09:00',
      channel_id: options.channels[1]!.id,
    });
    expect(same.reminder_error).toBe('dm_closed');

    // Switched off: the failure no longer applies.
    const off = await api.patchSeries(GUILD_ID, 1, { reminder_enabled: false });
    expect(off.reminder_error).toBeUndefined();
    expect(off.reminder_error_at).toBeUndefined();
  });

  it('refuses to change a revoked series, with the server’s status and code', async () => {
    const error = await failure(client().patchSeries(GUILD_ID, 9, { name: 'x' }));
    expect(error.status).toBe(403);
    expect(error.code).toBe('revoked');
  });

  it('starts every mock from the fixtures: one page’s changes do not leak', async () => {
    await client().patchSeries(GUILD_ID, 7, { name: 'Renamed' });
    expect((await client().getSettings(GUILD_ID, 7)).name).toBe('Daily Sketch');
    expect(series.find((s) => s.id === 7)?.name).toBe('Daily Sketch');
  });

  it('serves a scenario’s own series list and eligibility', async () => {
    const api = client({ series: [], eligibility: eligibilityBlocked });
    expect(await api.listSeries(GUILD_ID)).toEqual([]);
    expect((await api.getEligibility(GUILD_ID)).violations.map((v) => v.code)).toEqual([
      'missing_creator_role',
      'membership_too_new',
    ]);
    expect(await api.listMySeries(GUILD_ID)).toEqual([]);
  });

  it('leaves a held day unanswered, so the viewer keeps loading', async () => {
    const api = client({ holdDays: true });
    const settled = await Promise.race([
      api.getDay(GUILD_ID, 7, VIEWER_DAY).then(
        () => 'answered',
        () => 'failed',
      ),
      new Promise((resolve) => setTimeout(() => resolve('pending'), 20)),
    ]);
    expect(settled).toBe('pending');
    // The index still loads: the calendar under the viewer is there.
    expect(await api.listDays(GUILD_ID, 7)).toHaveLength(dayIndex.length);
  });

  it('refuses the session for good in the expired scenario', async () => {
    let ended = false;
    const api = new LeafApi({
      token: 'mock-token',
      fetch: createMockApi({ listFails: 'expired' }),
      onUnauthorized: () => (ended = true),
    });
    expect((await failure(api.listSeries(GUILD_ID))).kind).toBe('unauthorized');
    expect(ended).toBe(true);
  });

  it('fails the list in a way a retry could fix in the unavailable scenario', async () => {
    const error = await failure(client({ listFails: 'unavailable' }).listSeries(GUILD_ID));
    expect(error.kind).toBe('server');
    expect(error.retryable).toBe(true);
    // The server's only 503: it could not reach Discord, and says so.
    expect(error.status).toBe(503);
    expect(error.code).toBe('discord_unavailable');
    expect(error.detail).toMatch(/can't reach Discord/);
  });

  it('rejects anything that is not a gallery API request', async () => {
    const mock = createMockApi();
    await expect(mock('/api/admin/guilds')).rejects.toThrow(TypeError);
    await expect(mock('/assets/app.js')).rejects.toThrow(TypeError);
  });

  it('404s a gallery API path it has no answer for, and says so in the console', async () => {
    const said = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const mock = createMockApi();
    const series = `/api/guilds/${GUILD_ID}/series`;

    expect((await mock('/api/nothing/here')).status).toBe(404);
    expect((await mock(`${series}/7`)).status).toBe(404);
    expect((await mock(`${series}/7/likes`)).status).toBe(404);
    expect(said.mock.calls).toEqual([
      ['mock API: nothing answers GET /nothing/here'],
      [`mock API: nothing answers GET /guilds/${GUILD_ID}/series/7`],
      [`mock API: nothing answers GET /guilds/${GUILD_ID}/series/7/likes`],
    ]);

    // What the real API also answers 404 is not worth a line: a day or a
    // series that is not there, someone else's settings.
    said.mockClear();
    expect((await mock(`${series}/7/days/124`)).status).toBe(404);
    expect((await mock(`${series}/4242/days`)).status).toBe(404);
    expect((await mock(`${series}/2/settings`)).status).toBe(404);
    expect((await mock(`${series}/2`, { method: 'PATCH', body: '{}' })).status).toBe(404);
    expect(said).not.toHaveBeenCalled();
    said.mockRestore();
  });
});

describe('fixture dates', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  // The fixtures are built from the current time. Whenever that is (just
  // after a midnight in the server's zone, on either side of a clock change
  // there), each day has to land on its own date, bar the one catch-up.
  it.each([
    '2026-06-15T18:00:00Z',
    '2026-10-03T05:20:00Z',
    '2026-11-01T06:30:00Z',
    '2026-11-07T08:30:00Z',
    '2026-11-09T05:59:00Z',
    '2026-03-08T08:05:00Z',
    '2026-03-16T04:55:00Z',
  ])('puts Day 120 and Day 121 on one date and every other day on its own, at %s', async (now) => {
    vi.useFakeTimers({ now: new Date(now), toFake: ['Date'] });
    vi.resetModules();
    const fresh = await import('./fixtures');

    const dates = fresh.dayIndex.map((row) => row.local_date);
    const date = (day: number): string | undefined =>
      fresh.dayIndex.find((row) => row.day === day)?.local_date;
    expect(date(121)).toBe(date(120));
    expect(new Set(dates).size).toBe(dates.length - 1);
    // Nothing is dated in the future, and the newest post is under a day old.
    const newest = Math.max(...fresh.dayIndex.map((row) => row.posted_at));
    const age = Date.now() / 1000 - newest;
    expect(age).toBeGreaterThanOrEqual(0);
    expect(age).toBeLessThan(86_400);
    expect(fresh.homeSeries.last_posted_at).toBe(newest);
  });
});

describe('worst-case text', () => {
  it('stretches every name, description and caption, and nothing else', async () => {
    const api = client({ longText: true });

    const [first] = await api.listSeries(GUILD_ID);
    expect(first?.name).toBe('W'.repeat(40));
    expect(first?.description).toHaveLength(200);
    expect(first?.emoji).toBe('✏️');

    const day = await api.getDay(GUILD_ID, 7, VIEWER_DAY);
    expect(day.caption).toMatch(/^W{120} https:\/\//);
    expect(day.jump_url).toMatch(/^https:\/\/discord\.com\/channels\//);

    const [mine] = await api.listMySeries(GUILD_ID);
    expect(mine?.channel_name).toBe('w'.repeat(60));
  });

  it('copies rather than changes what it is given', () => {
    const before = structuredClone(options);
    const long = worstCase(options);
    expect(options).toEqual(before);
    expect(long.channels[0]?.name).toBe('W'.repeat(40));
    // A channel leaf cannot see has no name to stretch.
    expect(long.channels.find((c) => c.id === options.channels[3]?.id)?.name).toBeNull();
    expect(long.guild_timezone).toBe(options.guild_timezone);
  });
});
