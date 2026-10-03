import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError, exchangeToken, LeafApi } from './client';

/** A fetch double; typing it as `typeof fetch` keeps `mock.calls` well-typed. */
function fetchMock(respond: () => Response) {
  return vi.fn<typeof fetch>(() => Promise.resolve(respond()));
}

/** A fetch double that answers each call from a queue (the last entry repeats). */
function fetchSequence(...steps: (() => Response | Promise<Response>)[]) {
  let call = 0;
  return vi.fn<typeof fetch>(() => {
    const step = steps[Math.min(call, steps.length - 1)]!;
    call += 1;
    return Promise.resolve().then(step);
  });
}

/** A fetch double that never answers until its signal aborts, like a hung request. */
function hangingFetch() {
  return vi.fn<typeof fetch>(
    (_url, init) =>
      new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener('abort', () => {
          reject(new DOMException('aborted', 'AbortError'));
        });
      }),
  );
}

/**
 * A fetch double whose headers arrive at once (2xx) but whose body fails:
 * with `fail` when given, otherwise only when the request is aborted.
 */
function brokenBodyFetch(fail?: () => unknown) {
  return vi.fn<typeof fetch>((_url, init) => {
    const json = (): Promise<unknown> =>
      new Promise((_resolve, reject) => {
        if (fail) reject(fail());
        init?.signal?.addEventListener('abort', () => {
          reject(new DOMException('aborted', 'AbortError'));
        });
      });
    return Promise.resolve({ ok: true, status: 200, json } as Response);
  });
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function bearer(call: Parameters<typeof fetch> | undefined): string | undefined {
  return (call?.[1]?.headers as Record<string, string> | undefined)?.authorization;
}

const SERIES = {
  id: 1,
  name: 'Daily Johan',
  description: '',
  creator_id: 'u',
  cadence: 'daily',
  emoji: '🍃',
  start_day: 1,
  max_day: 3,
};

afterEach(() => {
  vi.useRealTimers();
});

describe('LeafApi', () => {
  it('sends the bearer token and parses a series list', async () => {
    const fetchImpl = fetchMock(() => jsonResponse([SERIES]));
    const api = new LeafApi({ token: 'tok', baseUrl: '/api', fetch: fetchImpl });

    const out = await api.listSeries('g1');

    expect(out).toHaveLength(1);
    expect(out[0]?.name).toBe('Daily Johan');
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('/api/guilds/g1/series');
    expect(init?.headers).toMatchObject({ authorization: 'Bearer tok' });
  });

  it('builds the day-range query only from provided bounds', async () => {
    const fetchImpl = fetchMock(() => jsonResponse([]));
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl });

    await api.listDays('g1', 7, { from: 10 });
    await api.listDays('g1', 7);

    expect(fetchImpl.mock.calls[0]?.[0]).toBe('/api/guilds/g1/series/7/days?from=10');
    expect(fetchImpl.mock.calls[1]?.[0]).toBe('/api/guilds/g1/series/7/days');
  });

  it('throws ApiError on a non-2xx response', async () => {
    const fetchImpl = fetchMock(() => new Response('nope', { status: 403 }));
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    await expect(api.getStats('g1', 1)).rejects.toBeInstanceOf(ApiError);
    await expect(api.getStats('g1', 1)).rejects.toMatchObject({ status: 403, kind: 'forbidden' });
  });

  it('rejects a payload that violates the schema as a bad response, without retrying', async () => {
    const fetchImpl = fetchMock(() => jsonResponse([{ id: 'not-a-number' }]));
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.listSeries('g1')).rejects.toMatchObject({
      kind: 'bad_response',
      retryable: false,
    });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('reports a 2xx that is not JSON as a bad response', async () => {
    const fetchImpl = fetchMock(() => new Response('<!doctype html>', { status: 200 }));
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    await expect(api.getLaunchIntent('g1')).rejects.toMatchObject({ kind: 'bad_response' });
  });

  it('falls back to the global fetch when none is injected', async () => {
    const spy = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(
        new Response('[]', { status: 200, headers: { 'content-type': 'application/json' } }),
      );
    // No fetch injected → must use a correctly-bound global fetch, not call
    // it as a method (which throws "Illegal invocation" in a real browser).
    const out = await new LeafApi({ token: 't', baseUrl: '/api' }).listSeries('g1');
    expect(out).toEqual([]);
    expect(spy).toHaveBeenCalledWith(
      '/api/guilds/g1/series',
      expect.objectContaining({ headers: { authorization: 'Bearer t' } }),
    );
    spy.mockRestore();
  });

  it('accepts the fields a newer server adds and reads null as absent', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse([
        {
          ...SERIES,
          state: 'sprout',
          privacy: 'public',
          is_owner: true,
          channel_ids: ['c1'],
          timezone: 'America/Chicago',
          last_posted_at: null,
          total_days: 2,
          sprout: { archived: 2, threshold: 3 },
        },
      ]),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    const [series] = await api.listSeries('g1');

    expect(series).toMatchObject({
      state: 'sprout',
      is_owner: true,
      channel_ids: ['c1'],
      sprout: { archived: 2, threshold: 3 },
    });
    expect(series?.last_posted_at).toBeUndefined();
  });

  it('drops a new field of the wrong shape instead of failing the response', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const fetchImpl = fetchMock(() =>
      jsonResponse([{ ...SERIES, sprout: 'almost', total_days: '2', state: 'active' }]),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    const [series] = await api.listSeries('g1');

    expect(series).toMatchObject({ id: 1, state: 'active' });
    expect(series?.sprout).toBeUndefined();
    expect(series?.total_days).toBeUndefined();
    expect(warn).toHaveBeenCalledTimes(2);
    warn.mockRestore();
  });

  it('parses whole-index rows with local_date, count and missing', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse([
        {
          day: 1,
          posted_at: 10,
          thumb_url: null,
          local_date: '2026-10-01',
          count: 2,
          missing: true,
        },
        { day: 2, posted_at: 20, thumb_url: '/api/media/a?thumb=1' },
      ]),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    const rows = await api.listDays('g1', 7);

    expect(rows[0]).toMatchObject({ local_date: '2026-10-01', count: 2, missing: true });
    expect(rows[1]?.local_date).toBeUndefined();
  });

  it('parses a launch intent, with and without a day, and null', async () => {
    const fetchImpl = fetchSequence(
      () => jsonResponse({ series_id: 4, day: 12 }),
      () => jsonResponse({ series_id: 4, day: null }),
      () => jsonResponse(null),
    );
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl });

    expect(await api.getLaunchIntent('g1')).toEqual({ series_id: 4, day: 12 });
    expect((await api.getLaunchIntent('g1'))?.day).toBeUndefined();
    expect(await api.getLaunchIntent('g1')).toBeNull();
    expect(fetchImpl.mock.calls[0]?.[0]).toMatch(
      /^\/api\/guilds\/g1\/launch-intent\?attempt=[a-z0-9]+$/,
    );
    // A new call is a new attempt.
    expect(fetchImpl.mock.calls[1]?.[0]).not.toBe(fetchImpl.mock.calls[0]?.[0]);
  });

  it('repeats a launch intent request under the same attempt id', async () => {
    // The first answer is lost; the server may already have forgotten the
    // intent, so the retry has to be recognisable as the same call.
    const fetchImpl = fetchSequence(
      () => Promise.reject(new TypeError('connection dropped')),
      () => jsonResponse({ series_id: 4, day: 12 }),
    );
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl, retryDelayMs: 0 });

    expect(await api.getLaunchIntent('g1')).toEqual({ series_id: 4, day: 12 });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    expect(fetchImpl.mock.calls[1]?.[0]).toBe(fetchImpl.mock.calls[0]?.[0]);
  });
});

describe('LeafApi failure handling', () => {
  it('reads code, message and retryable from the error body', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse(
        { error: 'discord_unavailable', message: 'Discord is down.', retryable: false },
        503,
      ),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.listSeries('g1')).rejects.toMatchObject({
      status: 503,
      kind: 'server',
      code: 'discord_unavailable',
      detail: 'Discord is down.',
      retryable: false,
    });
    // The server said not to bother: no automatic retry.
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('retries a GET once after a network failure', async () => {
    const fetchImpl = fetchSequence(
      () => Promise.reject(new TypeError('Failed to fetch')),
      () => jsonResponse([SERIES]),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.listSeries('g1')).resolves.toHaveLength(1);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('retries a GET once after a 5xx and then gives up', async () => {
    const fetchImpl = fetchMock(() => jsonResponse({ error: 'internal' }, 500));
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.getStats('g1', 1)).rejects.toMatchObject({ kind: 'server', retryable: true });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('reports a network failure with kind network and status 0', async () => {
    const fetchImpl = fetchMock(() => {
      throw new TypeError('Failed to fetch');
    });
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.getStats('g1', 1)).rejects.toMatchObject({ kind: 'network', status: 0 });
  });

  it('does not retry a 404 or a refusal', async () => {
    const fetchImpl = fetchSequence(
      () => jsonResponse({ error: 'not_found' }, 404),
      () => jsonResponse({ error: 'bad_request' }, 400),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.getDay('g1', 1, 5)).rejects.toMatchObject({ kind: 'not_found' });
    await expect(api.listDays('g1', 1)).rejects.toMatchObject({ kind: 'rejected', status: 400 });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('never repeats a write, even when the failure is transient', async () => {
    const fetchImpl = fetchMock(() => jsonResponse({ error: 'internal' }, 500));
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.patchSeries('g1', 1, { description: 'x' })).rejects.toMatchObject({
      kind: 'server',
    });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('treats a connection dropped part-way through the body as a network failure', async () => {
    const fetchImpl = brokenBodyFetch(() => new TypeError('network error'));
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    await expect(api.listSeries('g1')).rejects.toMatchObject({ kind: 'network', retryable: true });
    // Worth one more try, unlike a body that arrived and made no sense.
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('gives up on a hung GET after the time limit, retrying once', async () => {
    vi.useFakeTimers();
    const fetchImpl = hangingFetch();
    const api = new LeafApi({ token: 't', fetch: fetchImpl, retryDelayMs: 0 });

    const outcome = api.listSeries('g1').catch((e: unknown) => e);
    // Runs the first time limit, the pause, and the retry's time limit.
    await vi.runAllTimersAsync();

    expect(await outcome).toMatchObject({ kind: 'timeout', status: 0, retryable: true });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });
});

describe('LeafApi session renewal', () => {
  const HOUR = 3600 * 1000;

  it('renews the token at 75% of its life and uses the new one', async () => {
    vi.useFakeTimers();
    const fetchImpl = fetchSequence(
      () => jsonResponse({ token: 'tok2', expires_in: 6 * 3600 }),
      () => jsonResponse([]),
    );
    const api = new LeafApi({
      token: 'tok1',
      baseUrl: '/api',
      fetch: fetchImpl,
      expiresAt: Date.now() + 4 * HOUR,
    });

    await vi.advanceTimersByTimeAsync(3 * HOUR - 1);
    expect(fetchImpl).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);

    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('/api/token/refresh');
    expect(init?.method).toBe('POST');
    expect(bearer(fetchImpl.mock.calls[0])).toBe('Bearer tok1');

    await api.listSeries('g1');
    expect(bearer(fetchImpl.mock.calls[1])).toBe('Bearer tok2');
    api.dispose();
  });

  it('ensureFresh renews only once the refresh point has passed', async () => {
    let now = 0;
    const fetchImpl = fetchMock(() => jsonResponse({ token: 'tok2', expires_in: 3600 }));
    const api = new LeafApi({
      token: 'tok1',
      fetch: fetchImpl,
      expiresAt: 4 * HOUR,
      now: () => now,
    });

    await api.ensureFresh();
    expect(fetchImpl).not.toHaveBeenCalled();

    now = 3 * HOUR;
    await Promise.all([api.ensureFresh(), api.ensureFresh()]);
    // Single-flight: two callers, one request.
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    api.dispose();
  });

  it('replays a request once after a 401 when the token can be renewed', async () => {
    const fetchImpl = fetchSequence(
      () => jsonResponse({ error: 'unauthorized' }, 401),
      () => jsonResponse({ token: 'tok2', expires_in: 3600 }),
      () => jsonResponse([SERIES]),
    );
    const onUnauthorized = vi.fn();
    const api = new LeafApi({ token: 'tok1', fetch: fetchImpl, onUnauthorized });

    await expect(api.listSeries('g1')).resolves.toHaveLength(1);

    expect(bearer(fetchImpl.mock.calls[2])).toBe('Bearer tok2');
    expect(onUnauthorized).not.toHaveBeenCalled();
    api.dispose();
  });

  it('reports the session as over when a 401 cannot be renewed, once', async () => {
    const fetchImpl = fetchMock(() => jsonResponse({ error: 'unauthorized' }, 401));
    const onUnauthorized = vi.fn();
    const api = new LeafApi({ token: 'tok1', fetch: fetchImpl, onUnauthorized });

    await expect(api.listSeries('g1')).rejects.toMatchObject({ kind: 'unauthorized' });
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
    // The request and one refresh attempt.
    expect(fetchImpl).toHaveBeenCalledTimes(2);

    // Later calls fail at once without asking the server again.
    await expect(api.getStats('g1', 1)).rejects.toMatchObject({ kind: 'unauthorized' });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
  });

  it('replays a refused write once the token is renewed', async () => {
    const fetchImpl = fetchSequence(
      () => jsonResponse({ error: 'unauthorized' }, 401),
      () => jsonResponse({ token: 'tok2', expires_in: 3600 }),
      () => jsonResponse({ id: 9, name: 'New', emoji: '🍃' }, 201),
    );
    const api = new LeafApi({ token: 'tok1', fetch: fetchImpl });

    await expect(
      api.createSeries('g1', {
        name: 'New',
        channel_id: 'c1',
        cadence: 'daily',
        privacy: 'public',
      }),
    ).resolves.toMatchObject({ id: 9 });

    expect(fetchImpl).toHaveBeenCalledTimes(3);
    expect(bearer(fetchImpl.mock.calls[2])).toBe('Bearer tok2');
    api.dispose();
  });

  it('ends the session when the retry of a failed GET is refused', async () => {
    const fetchImpl = fetchSequence(
      () => jsonResponse({ error: 'internal' }, 500),
      () => jsonResponse({ error: 'unauthorized' }, 401),
    );
    const onUnauthorized = vi.fn();
    const api = new LeafApi({ token: 'tok1', fetch: fetchImpl, retryDelayMs: 0, onUnauthorized });

    await expect(api.listSeries('g1')).rejects.toMatchObject({ kind: 'unauthorized' });

    // The request, its retry, and one refresh attempt.
    expect(fetchImpl).toHaveBeenCalledTimes(3);
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
  });

  it('gives up after one replay when the renewed token is refused too', async () => {
    const fetchImpl = fetchSequence(
      () => jsonResponse({ error: 'unauthorized' }, 401),
      () => jsonResponse({ token: 'tok2', expires_in: 3600 }),
      () => jsonResponse({ error: 'unauthorized' }, 401),
    );
    const onUnauthorized = vi.fn();
    const api = new LeafApi({ token: 'tok1', fetch: fetchImpl, onUnauthorized });

    await expect(api.listSeries('g1')).rejects.toMatchObject({ kind: 'unauthorized' });

    expect(fetchImpl).toHaveBeenCalledTimes(3);
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
  });

  it('never plans a renewal sooner than 30 seconds', async () => {
    vi.useFakeTimers();
    const fetchImpl = fetchMock(() => jsonResponse({ token: 'tok2', expires_in: 1 }));
    const api = new LeafApi({ token: 'tok1', fetch: fetchImpl, expiresAt: Date.now() + 4_000 });

    await vi.advanceTimersByTimeAsync(29_999);
    expect(fetchImpl).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    // The one-second token it was handed does not bring the next one forward.
    await vi.advanceTimersByTimeAsync(29_999);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    api.dispose();
  });

  it('keeps the session when an older server has no refresh route', async () => {
    let now = 0;
    const fetchImpl = fetchSequence(
      () => jsonResponse({ error: 'not_found' }, 404),
      () => jsonResponse([]),
      () => jsonResponse([]),
    );
    const onUnauthorized = vi.fn();
    const api = new LeafApi({
      token: 'tok1',
      fetch: fetchImpl,
      expiresAt: 4 * HOUR,
      now: () => now,
      onUnauthorized,
    });

    now = 3 * HOUR;
    await api.ensureFresh();
    await api.listSeries('g1');
    await api.listSeries('g1');

    // One refresh attempt, then the old token is simply used.
    expect(fetchImpl).toHaveBeenCalledTimes(3);
    expect(bearer(fetchImpl.mock.calls[2])).toBe('Bearer tok1');
    expect(onUnauthorized).not.toHaveBeenCalled();
    api.dispose();
  });
});

describe('LeafApi creator endpoints', () => {
  it('parses eligibility with violations and their params', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse({
        can_create: false,
        owns_any: true,
        violations: [
          { code: 'max_series', message: 'too many', params: { limit: 3, current: 3 } },
          { code: 'missing_creator_role', params: { role_name: 'Artists', eligible_at: null } },
        ],
      }),
    );
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl });

    const out = await api.getEligibility('g1');

    expect(out.can_create).toBe(false);
    expect(out.owns_any).toBe(true);
    expect(out.violations[0]).toMatchObject({ code: 'max_series', params: { limit: 3 } });
    expect(out.violations[1]).toMatchObject({ message: '', params: { role_name: 'Artists' } });
    expect(fetchImpl.mock.calls[0]?.[0]).toBe('/api/guilds/g1/series/eligibility');
  });

  it('parses eligibility from an older server', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse({
        can_create: false,
        violations: [{ code: 'max_series', message: 'too many' }],
      }),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    const out = await api.getEligibility('g1');

    expect(out.owns_any).toBeUndefined();
    expect(out.violations[0]?.params).toBeUndefined();
  });

  it('parses options with unnamed channels and unavailable roles', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse({
        channels: [{ id: 'c1', name: null }],
        roles: [{ id: 'r1', name: 'Artists', held: true }],
        cadences: ['daily'],
        privacy_modes: ['public'],
        guild_timezone: 'UTC',
        sprout_enabled: true,
        sprout_threshold: 3,
        roles_unavailable: true,
      }),
    );
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    const out = await api.getOptions('g1');

    expect(out.channels[0]?.name).toBeNull();
    expect(out.roles[0]?.held).toBe(true);
    expect(out.roles_unavailable).toBe(true);
  });

  it('posts a create payload and parses the created series', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse({ id: 9, name: 'New', state: 'active', emoji: '🍃' }, 201),
    );
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl });

    const out = await api.createSeries('g1', {
      name: 'New',
      channel_id: 'c1',
      cadence: 'daily',
      privacy: 'public',
    });

    expect(out.id).toBe(9);
    expect(out.creator_id).toBeUndefined();
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('/api/guilds/g1/series');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toMatchObject({ name: 'New', channel_id: 'c1' });
  });

  it('parses the full series a current server returns from create', async () => {
    const fetchImpl = fetchMock(() => jsonResponse({ ...SERIES, id: 9, state: 'sprout' }, 201));
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    const out = await api.createSeries('g1', {
      name: 'New',
      channel_id: 'c1',
      cadence: 'daily',
      privacy: 'public',
    });

    expect(out).toMatchObject({ id: 9, creator_id: 'u', start_day: 1, state: 'sprout' });
  });

  it('surfaces the server error code on a failed create', async () => {
    const fetchImpl = fetchMock(() => jsonResponse({ error: 'name_taken' }, 409));
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    await expect(
      api.createSeries('g1', {
        name: 'dup',
        channel_id: 'c1',
        cadence: 'daily',
        privacy: 'public',
      }),
    ).rejects.toMatchObject({ status: 409, code: 'name_taken', kind: 'rejected' });
  });

  it('patches a series and parses the updated settings', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse({
        id: 1,
        name: 'S',
        description: 'updated',
        emoji: '🍃',
        cadence: 'daily',
        privacy: 'public',
        privacy_role_id: null,
        channel_id: 'c1',
        detection_mode: 'context_menu',
        state: 'active',
        reminder_enabled: false,
        reminder_time: null,
        reminder_timezone: null,
        reminder_dm: true,
        reminder_error: 'dm_closed',
        reminder_error_at: 1700,
      }),
    );
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl });

    const out = await api.patchSeries('g1', 1, { description: 'updated', reminder_timezone: '' });

    expect(out.description).toBe('updated');
    expect(out.reminder_error).toBe('dm_closed');
    expect(fetchImpl.mock.calls[0]?.[1]?.method).toBe('PATCH');
    expect(JSON.parse(fetchImpl.mock.calls[0]?.[1]?.body as string)).toEqual({
      description: 'updated',
      reminder_timezone: '',
    });
  });
});

describe('exchangeToken', () => {
  it('posts the code and validates the token response', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse({ token: 't', access_token: 'a', expires_in: 3600 }),
    );

    const result = await exchangeToken('CODE', fetchImpl);

    expect(result.token).toBe('t');
    expect(result.access_token).toBe('a');
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('/api/token');
    expect(init?.method).toBe('POST');
  });

  it('throws ApiError when the exchange fails', async () => {
    const fetchImpl = fetchMock(() => new Response('bad', { status: 400 }));
    await expect(exchangeToken('CODE', fetchImpl)).rejects.toBeInstanceOf(ApiError);
  });

  it('carries the server code so the boot screen can tell the causes apart', async () => {
    const rejected = fetchMock(() => jsonResponse({ error: 'code_rejected' }, 400));
    await expect(exchangeToken('CODE', rejected)).rejects.toMatchObject({
      code: 'code_rejected',
      retryable: false,
    });

    const down = fetchMock(() => jsonResponse({ error: 'discord_unavailable' }, 503));
    await expect(exchangeToken('CODE', down)).rejects.toMatchObject({
      code: 'discord_unavailable',
      retryable: true,
    });
  });

  it('allows a slow body longer than the wait for the first byte', async () => {
    vi.useFakeTimers();
    const fetchImpl = brokenBodyFetch();
    let outcome: unknown = 'pending';
    void exchangeToken('CODE', fetchImpl, 5_000).catch((e: unknown) => {
      outcome = e;
    });

    // Past the limit for the response to start: the body is still coming.
    await vi.advanceTimersByTimeAsync(19_999);
    expect(outcome).toBe('pending');
    await vi.advanceTimersByTimeAsync(1);
    expect(outcome).toMatchObject({ kind: 'timeout' });
  });

  it('times out instead of hanging, and does not retry', async () => {
    vi.useFakeTimers();
    const fetchImpl = hangingFetch();

    const outcome = exchangeToken('CODE', fetchImpl, 5_000).catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(5_000);

    expect(await outcome).toMatchObject({ kind: 'timeout' });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });
});
