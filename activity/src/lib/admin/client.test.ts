import { afterEach, describe, expect, it, vi } from 'vitest';

import { AdminApi, AdminApiError, isUnauthorized } from './client';

function fetchMock(respond: () => Response) {
  return vi.fn<typeof fetch>(() => Promise.resolve(respond()));
}
function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}
/** The error a call rejects with (fails the test when it resolves). */
async function failure(call: Promise<unknown>): Promise<AdminApiError> {
  const e: unknown = await call.then(
    () => undefined,
    (reason: unknown) => reason,
  );
  expect(e).toBeInstanceOf(AdminApiError);
  return e as AdminApiError;
}

const SETTINGS = {
  timezone: 'UTC',
  creator_role_id: null,
  log_channel_id: null,
  max_series_per_user: 3,
  min_account_age_days: 0,
  min_membership_age_days: 0,
  sprout_enabled: false,
  sprout_threshold: 3,
};

afterEach(() => {
  vi.useRealTimers();
});

describe('AdminApi', () => {
  it('sends the bearer token and lists guilds', async () => {
    const f = fetchMock(() => json([{ guild_id: 'g1', series_count: 3 }]));
    const api = new AdminApi('tok', f);

    const guilds = await api.listGuilds();

    expect(guilds[0]?.guild_id).toBe('g1');
    const [url, init] = f.mock.calls[0]!;
    expect(url).toBe('/api/admin/guilds');
    expect((init as RequestInit | undefined)?.headers).toMatchObject({
      authorization: 'Bearer tok',
    });
  });

  it('PATCHes a series with a JSON body', async () => {
    const f = fetchMock(() =>
      json({
        id: 1,
        name: 'a',
        creator_id: 'u',
        privacy: 'public',
        privacy_role_id: null,
        state: 'revoked',
      }),
    );
    const api = new AdminApi('tok', f);

    const out = await api.patchSeries('g1', 1, { state: 'revoked' });

    expect(out.state).toBe('revoked');
    const [url, init] = f.mock.calls[0]!;
    expect(url).toBe('/api/admin/guilds/g1/series/1');
    expect((init as RequestInit | undefined)?.method).toBe('PATCH');
    expect((init as RequestInit | undefined)?.body).toBe('{"state":"revoked"}');
  });

  it('throws AdminApiError on a non-2xx response (e.g. an expired token)', async () => {
    const f = fetchMock(() => new Response('no', { status: 401 }));
    const api = new AdminApi('tok', f);

    const e = await failure(api.listGuilds());

    expect(e.status).toBe(401);
    expect(e.kind).toBe('unauthorized');
    expect(isUnauthorized(e)).toBe(true);
  });

  it('reads the names an up-to-date server sends, and copes with null', async () => {
    const f = fetchMock(() =>
      json({
        guild_id: 'g1',
        name: 'Walpurgis',
        icon_url: null,
        setup_complete: false,
        settings: SETTINGS,
        series: [
          {
            id: 1,
            name: 'a',
            creator_id: 'u',
            creator_name: 'Mika',
            archived_days: 2,
            privacy: 'role_gated',
            privacy_role_id: 'r1',
            privacy_role_name: null,
            state: 'sprout',
          },
        ],
      }),
    );

    const detail = await new AdminApi('tok', f).guild('g1');

    expect(detail.name).toBe('Walpurgis');
    expect(detail.icon_url).toBeUndefined();
    expect(detail.setup_complete).toBe(false);
    expect(detail.series[0]).toMatchObject({ creator_name: 'Mika', archived_days: 2 });
    expect(detail.series[0]?.privacy_role_name).toBeUndefined();
  });

  it('still reads an older server that sends none of the new fields', async () => {
    const f = fetchMock(() => json({ guild_id: 'g1', settings: SETTINGS, series: [] }));

    const detail = await new AdminApi('tok', f).guild('g1');

    expect(detail.name).toBeUndefined();
    expect(detail.setup_complete).toBeUndefined();
  });

  it('drops a new field of the wrong shape instead of failing the response', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const f = fetchMock(() => json([{ guild_id: 'g1', series_count: 1, name: 42 }]));

    const guilds = await new AdminApi('tok', f).listGuilds();

    expect(guilds[0]?.name).toBeUndefined();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('fetches the picker lists and escapes the guild id in the path', async () => {
    const f = fetchMock(() =>
      json({
        roles: [{ id: 'r1', name: 'Artist' }],
        channels: [
          { id: 'c1', name: 'art' },
          { id: 'c2', name: null },
        ],
        roles_unavailable: false,
      }),
    );

    const options = await new AdminApi('tok', f).options('1/2');

    expect(f.mock.calls[0]?.[0]).toBe('/api/admin/guilds/1%2F2/options');
    expect(options.roles).toEqual([{ id: 'r1', name: 'Artist' }]);
    expect(options.channels?.[1]).toEqual({ id: 'c2', name: undefined });
    expect(options.channels_unavailable).toBeUndefined();
  });

  it('reports how many sprouts a settings save published', async () => {
    const f = fetchMock(() => json({ ...SETTINGS, sprouts_published: 2 }));

    const saved = await new AdminApi('tok', f).patchSettings('g1', { sprout_enabled: false });

    expect(saved.sprouts_published).toBe(2);
    expect((f.mock.calls[0]?.[1] as RequestInit | undefined)?.body).toBe(
      '{"sprout_enabled":false}',
    );
  });

  it('reads a settings answer that wraps the settings, too', async () => {
    const f = fetchMock(() => json({ settings: SETTINGS, sprouts_published: 1 }));

    const saved = await new AdminApi('tok', f).patchSettings('g1', { sprout_threshold: 1 });

    expect(saved).toMatchObject({ timezone: 'UTC', sprouts_published: 1 });
  });
});

describe('AdminApi failures', () => {
  it('carries the server’s code and sentence for a refused value', async () => {
    const f = fetchMock(() =>
      json({ error: 'invalid_limit', message: 'Series per member must be at least 1.' }, 422),
    );

    const e = await failure(new AdminApi('tok', f).patchSettings('g1', { max_series_per_user: 0 }));

    expect(e.status).toBe(422);
    expect(e.kind).toBe('rejected');
    expect(e.code).toBe('invalid_limit');
    expect(e.detail).toBe('Series per member must be at least 1.');
    expect(e.retryable).toBe(false);
    // The developer label never doubles as the sentence shown to the admin.
    expect(e.message).toBe('PATCH /guilds/g1/settings → 422');
  });

  it('honours the body’s retryable flag', async () => {
    const f = fetchMock(() => json({ error: 'discord_unavailable', retryable: true }, 503));

    const e = await failure(new AdminApi('tok', f).options('g1'));

    expect(e.kind).toBe('server');
    expect(e.retryable).toBe(true);
  });

  it('calls a request that never got an answer a network failure', async () => {
    const f = vi.fn<typeof fetch>(() => Promise.reject(new TypeError('Failed to fetch')));

    const e = await failure(new AdminApi('tok', f).listGuilds());

    expect(e.status).toBe(0);
    expect(e.kind).toBe('network');
    expect(e.retryable).toBe(true);
  });

  it('gives up on a request that hangs', async () => {
    vi.useFakeTimers();
    const f = vi.fn<typeof fetch>(
      (_url, init) =>
        new Promise((_resolve, reject) => {
          init?.signal?.addEventListener('abort', () => {
            reject(new DOMException('aborted', 'AbortError'));
          });
        }),
    );

    const pending = failure(new AdminApi('tok', f).listGuilds());
    await vi.advanceTimersByTimeAsync(15_000);
    const e = await pending;

    expect(e.kind).toBe('timeout');
    expect(e.status).toBe(0);
  });

  it('calls a 2xx it cannot read a bad response, not a crash', async () => {
    const shape = fetchMock(() => json({ nope: true }));
    const html = fetchMock(() => new Response('<!doctype html>', { status: 200 }));

    const wrongShape = await failure(new AdminApi('tok', shape).guild('g1'));
    const notJson = await failure(new AdminApi('tok', html).guild('g1'));

    expect(wrongShape.kind).toBe('bad_response');
    expect(notJson.kind).toBe('bad_response');
  });

  it('treats a 404 as not found, so an older server’s missing route is recognisable', async () => {
    const f = fetchMock(() => json({ error: 'not_found' }, 404));

    const e = await failure(new AdminApi('tok', f).options('g1'));

    expect(e.kind).toBe('not_found');
    expect(isUnauthorized(e)).toBe(false);
  });
});
