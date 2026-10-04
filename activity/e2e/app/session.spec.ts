// The gallery session: a token leaf's server signs at sign-in, good for six
// hours, renewable until seven days after the sign-in. The Activity renews
// it by itself and, once the server will have no more of it, says the
// session has ended rather than failing request by request.
//
// The server keeps real time, so a session in the state a test needs is
// minted through a control route and handed to the Activity in place of the
// answer to its own code exchange. Everything after that is the real API.

import type { Page } from '@playwright/test';

import { expect, expectRefusal, test, type MintedSession } from '../support/app';

/** Answers the Activity's code exchange with `session`, as the server would have. */
async function signInWith(page: Page, session: MintedSession): Promise<void> {
  await page.route('**/api/token', (route) => route.fulfill({ json: session }));
}

/** The client renews once this share of a token's life has passed (client.ts). */
const REFRESH_AT = 0.75;

test('a session past its renewal point is renewed on the way back in, and the new token is used', async ({
  discord,
  leaf,
  page,
  time,
}) => {
  const first = await leaf.mintSession('viewer', { ttl_secs: 120 });
  await signInWith(page, first);
  const client = await discord.launch('viewer');
  const app = client.activity;
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();

  // Two minutes of life, three quarters gone by the page's clock. On the
  // server, which keeps its own, the token is still good.
  await client.layout('pip');
  await time.pass(120_000 * REFRESH_AT + 1_000);
  const renewal = page.waitForResponse((response) => response.url().endsWith('/api/token/refresh'));
  const listed = page.waitForResponse((response) =>
    new URL(response.url()).pathname.endsWith('/series'),
  );
  await client.layout('focused');

  const renewed = await renewal;
  expect(renewed.request().headers().authorization).toBe(`Bearer ${first.token}`);
  expect(renewed.status()).toBe(200);
  const next = (await renewed.json()) as { token: string; expires_in: number };
  expect(next.token).not.toBe(first.token);
  // A full six hours again: the sign-in was moments ago, nowhere near the cap.
  expect(next.expires_in).toBe(6 * 3600);

  // The list fetched for the return went out under the new token, and was served.
  const list = await listed;
  expect(list.request().headers().authorization).toBe(`Bearer ${next.token}`);
  expect(list.status()).toBe(200);
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();
});

test('a session past the seven-day cap keeps working until it lapses, then the gallery says it has ended', async ({
  discord,
  leaf,
  guard,
  page,
  time,
}) => {
  // Good for five more minutes, but signed in more than seven days ago.
  const capped = await leaf.mintSession('viewer', { kind: 'capped' });
  await signInWith(page, capped);
  const client = await discord.launch('viewer');
  const app = client.activity;
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();

  // Past its renewal point: the server refuses to renew it, and the gallery
  // carries on with the token it has.
  expectRefusal(guard, 401, 'POST', /\/api\/token\/refresh/);
  await client.layout('pip');
  await time.pass(capped.expires_in * 1000 * REFRESH_AT + 1_000);
  const renewal = page.waitForResponse((response) => response.url().endsWith('/api/token/refresh'));
  const listed = page.waitForResponse((response) =>
    new URL(response.url()).pathname.endsWith('/series'),
  );
  await client.layout('focused');
  expect((await renewal).status()).toBe(401);
  expect((await listed).status()).toBe(200);
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();
  await expect(app.getByText('Your session has ended')).toHaveCount(0);

  // The five minutes pass in a moment: from here on the server is shown the
  // same session as it will be once it has lapsed.
  const lapsed = await leaf.mintSession('viewer', { kind: 'capped', ttl_secs: -60 });
  await page.route('**/api/guilds/**', (route) =>
    route.continue({
      headers: { ...route.request().headers(), authorization: `Bearer ${lapsed.token}` },
    }),
  );
  const opened = leaf.seed.series.long.id;
  expectRefusal(guard, 401, 'GET', new RegExp(`/api/guilds/\\d+/series/${opened}/(stats|days)`));

  await app.getByRole('button', { name: /^Daily Sketch/ }).click();
  await expect(
    app.getByRole('heading', { level: 1, name: 'Your session has ended' }),
  ).toBeVisible();
  await expect(app.getByText('Close leaf and open it again to keep browsing.')).toBeVisible();
  // One way out, and it is Discord's to act on.
  await expect(app.getByRole('button')).toHaveText(['Close leaf']);
  await app.getByRole('button', { name: 'Close leaf' }).click();
  await expect
    .poll(() => client.closes())
    .toEqual([{ code: 1000, message: 'Closed by the user', nonce: expect.any(String) }]);
});

test('a session that lapsed before the gallery loaded goes straight to the same screen', async ({
  discord,
  leaf,
  guard,
  page,
}) => {
  await signInWith(page, await leaf.mintSession('viewer', { kind: 'expired' }));
  // Everything the first load asks for is refused, and so is the renewal it tries.
  expectRefusal(
    guard,
    401,
    'GET',
    /\/api\/guilds\/\d+\/(series|series\/eligibility|launch-intent)(\?.*)?/,
  );
  expectRefusal(guard, 401, 'POST', /\/api\/token\/refresh/);
  guard.allow(/^console error: leaf: loading the gallery failed/);

  const { activity: app } = await discord.launch('viewer');
  await expect(
    app.getByRole('heading', { level: 1, name: 'Your session has ended' }),
  ).toBeVisible();
  // Not the load-failure screen: there is nothing to try again.
  await expect(app.getByText('Couldn’t load the gallery')).toHaveCount(0);
  await expect(app.getByRole('button')).toHaveText(['Close leaf']);
});
