import { describe, expect, it, vi } from 'vitest';

import { AdminApi, AdminApiError } from './client';

function fetchMock(respond: () => Response) {
  return vi.fn<typeof fetch>(() => Promise.resolve(respond()));
}
function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

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
    await expect(api.listGuilds()).rejects.toBeInstanceOf(AdminApiError);
  });
});
