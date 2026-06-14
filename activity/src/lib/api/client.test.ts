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
