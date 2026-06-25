import { describe, expect, it, vi } from 'vitest';

import { ApiError, exchangeToken, LeafApi } from './client';

/** A fetch double; typing it as `typeof fetch` keeps `mock.calls` well-typed. */
function fetchMock(respond: () => Response) {
  return vi.fn<typeof fetch>(() => Promise.resolve(respond()));
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
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

    expect(fetchImpl.mock.calls[0]?.[0]).toBe('/api/guilds/g1/series/7/days?from=10');
  });

  it('throws ApiError on a non-2xx response', async () => {
    const fetchImpl = fetchMock(() => new Response('nope', { status: 403 }));
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    await expect(api.getStats('g1', 1)).rejects.toBeInstanceOf(ApiError);
  });

  it('rejects a payload that violates the schema', async () => {
    const fetchImpl = fetchMock(() => jsonResponse([{ id: 'not-a-number' }]));
    const api = new LeafApi({ token: 't', fetch: fetchImpl });

    await expect(api.listSeries('g1')).rejects.toBeTruthy();
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
    expect(spy).toHaveBeenCalledWith('/api/guilds/g1/series', {
      headers: { authorization: 'Bearer t' },
    });
    spy.mockRestore();
  });
});

describe('LeafApi creator endpoints', () => {
  it('parses eligibility with violations', async () => {
    const fetchImpl = fetchMock(() =>
      jsonResponse({
        can_create: false,
        violations: [{ code: 'max_series', message: 'too many' }],
      }),
    );
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl });

    const out = await api.getEligibility('g1');

    expect(out.can_create).toBe(false);
    expect(out.violations[0]?.code).toBe('max_series');
    expect(fetchImpl.mock.calls[0]?.[0]).toBe('/api/guilds/g1/series/eligibility');
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
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('/api/guilds/g1/series');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toMatchObject({ name: 'New', channel_id: 'c1' });
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
    ).rejects.toMatchObject({ status: 409, code: 'name_taken' });
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
      }),
    );
    const api = new LeafApi({ token: 't', baseUrl: '/api', fetch: fetchImpl });

    const out = await api.patchSeries('g1', 1, { description: 'updated' });

    expect(out.description).toBe('updated');
    expect(fetchImpl.mock.calls[0]?.[1]?.method).toBe('PATCH');
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
});
