// Who sees which series. The seed has one of each kind, all the creator's:
// public (Daily Sketch, Evening Walks), a sprout still short of its three
// days (Morning Coffee), one limited to the Patrons role (Patron Studies),
// one for its creator only (Private Notes) and one an admin revoked (Old
// Polaroids). Each persona's gallery lists exactly what the server's policy
// allows them, and asking the API directly for the rest is refused.

import type { Locator } from '@playwright/test';

import { expect, expectRefusal, test, type PersonaKey } from '../support/app';
import { seriesNames } from '../support/screens';

const PUBLIC = ['Daily Sketch', 'Evening Walks'];

const SEES: [PersonaKey, string, string[]][] = [
  ['viewer', 'a member sees the public series only', PUBLIC],
  [
    'patron',
    'a member with the role also sees the series limited to it',
    ['Daily Sketch', 'Patron Studies', 'Evening Walks'],
  ],
  ['newcomer', 'a member with no roles sees the public series', PUBLIC],
  ['admin', 'a server admin sees what any other member sees', PUBLIC],
];

for (const [persona, title, names] of SEES) {
  test(`${persona}: ${title}`, async ({ discord }) => {
    const { activity: app } = await discord.launch(persona);
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
    await expect(seriesNames(app)).toHaveText(names);
    // Nothing of the creator's own view leaks into someone else's.
    await expect(app.getByRole('button', { name: 'Manage my series' })).toHaveCount(0);
    await expect(app.getByText('Revoked')).toHaveCount(0);
  });
}

test('creator: all six of their series, with who else can see each', async ({ discord }) => {
  const { activity: app } = await discord.launch('creator');
  await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
  await expect(seriesNames(app)).toHaveText([
    'Daily Sketch',
    'Morning Coffee',
    'Patron Studies',
    'Private Notes',
    'Old Polaroids',
    'Evening Walks',
  ]);

  const card = (name: string): Locator => app.getByRole('listitem').filter({ hasText: name });
  // The sprout's numbers are the server's: the guild's threshold, and its own count.
  await expect(card('Morning Coffee')).toContainText(
    'Only you can see this until 3 days are archived (2 so far)',
  );
  await expect(card('Patron Studies')).toContainText('Only members with its role can see it.');
  await expect(card('Private Notes')).toContainText('Only you can see this series.');

  // The revoked one is listed so it does not just vanish, says why, and
  // cannot be opened: its days are no longer served.
  await expect(card('Old Polaroids')).toContainText('Revoked');
  await expect(card('Old Polaroids')).toContainText('A server admin revoked this series');
  await expect(card('Old Polaroids').getByRole('button')).toHaveCount(0);

  // Its settings still open, read-only, under the same banner.
  await app.getByRole('button', { name: 'Manage my series' }).click();
  await app.getByRole('button', { name: /^Old Polaroids/ }).click();
  await expect(app.getByText('A server admin revoked this series')).toBeVisible();
  await expect(app.getByLabel('Series name')).toHaveValue('Old Polaroids');
  await expect(app.getByLabel('Series name')).toBeDisabled();
  await expect(app.getByRole('button', { name: 'Save changes' })).toHaveCount(0);
});

test('outsider: someone who is not in the server is refused the gallery', async ({
  discord,
  guard,
}) => {
  // The series list, the eligibility check and the launch intent are all
  // asked for at once, and all refused.
  expectRefusal(
    guard,
    403,
    'GET',
    /\/api\/guilds\/\d+\/(series|series\/eligibility|launch-intent)(\?.*)?/,
  );
  guard.allow(/^console error: leaf: loading the gallery failed/);
  const { activity: app } = await discord.launch('outsider');

  await expect(
    app.getByRole('heading', { level: 1, name: 'Couldn’t load the gallery' }),
  ).toBeVisible();
  await expect(
    app.getByText('You don’t have access to this. If you think you should, ask a server admin.'),
  ).toBeVisible();
  await expect(seriesNames(app)).toHaveCount(0);
});

test.describe('asking the API directly', () => {
  /** Everything the API serves about one series. */
  const ROUTES = ['/days', '/days/1', '/stats', '/settings'];

  test('a series hidden from the caller is answered as if it did not exist', async ({ leaf }) => {
    const { series } = leaf.seed;
    for (const hidden of [series.sprout, series.role_gated, series.creator_only, series.revoked]) {
      for (const route of ROUTES) {
        const answer = await leaf.get('viewer', `/series/${hidden.id}${route}`);
        expect(answer.status(), `${hidden.name}${route}`).toBe(404);
        expect(await answer.json(), `${hidden.name}${route}`).toEqual({ error: 'not_found' });
      }
    }
    // The same answer as for a series that really is not there.
    const none = await leaf.get('viewer', '/series/999/days');
    expect(none.status()).toBe(404);
    expect(await none.json()).toEqual({ error: 'not_found' });
  });

  test('the same routes answer whoever may see the series', async ({ leaf }) => {
    const { series } = leaf.seed;
    // What makes the refusals above mean something: each of these is a
    // route that works, for the right person.
    expect((await leaf.get('patron', `/series/${series.role_gated.id}/days`)).status()).toBe(200);
    expect((await leaf.get('creator', `/series/${series.sprout.id}/days`)).status()).toBe(200);
    expect((await leaf.get('creator', `/series/${series.creator_only.id}/days/1`)).status()).toBe(
      200,
    );
    expect((await leaf.get('creator', `/series/${series.creator_only.id}/settings`)).status()).toBe(
      200,
    );
    // Settings are the owner's alone, even for a series everyone can see.
    expect((await leaf.get('viewer', `/series/${series.long.id}/days`)).status()).toBe(200);
    expect((await leaf.get('viewer', `/series/${series.long.id}/settings`)).status()).toBe(404);
    // Even its owner is not served a revoked series' days.
    expect((await leaf.get('creator', `/series/${series.revoked.id}/days`)).status()).toBe(404);
  });

  test('someone outside the server is refused every route, and no session means no answer', async ({
    leaf,
    request,
  }) => {
    const { series, guilds } = leaf.seed;
    for (const path of ['/series', `/series/${series.long.id}/days`, '/series/eligibility']) {
      expect((await leaf.get('outsider', path)).status(), path).toBe(403);
    }
    const anonymous = await request.get(`/api/guilds/${guilds.main.id}/series`);
    expect(anonymous.status()).toBe(401);
  });

  test('a signed media address opens that file and no other', async ({ leaf, request }) => {
    const { series, origin } = leaf.seed;
    /** The address the API hands `persona` for the first file of a series' Day 1. */
    const firstFile = async (persona: PersonaKey, id: number): Promise<URL> => {
      const day = await leaf.get(persona, `/series/${id}/days/1`);
      const { media } = (await day.json()) as { media: [{ url: string }] };
      return new URL(media[0].url, origin);
    };
    const own = await firstFile('viewer', series.long.id);
    expect((await request.get(own.href)).status()).toBe(200);

    // The creator's private series has a file the viewer was never given an
    // address for. The viewer's signature does not carry over to it...
    const hidden = await firstFile('creator', series.creator_only.id);
    expect(hidden.pathname).not.toBe(own.pathname);
    expect((await request.get(hidden.href)).status()).toBe(200);
    expect((await request.get(`${origin}${hidden.pathname}${own.search}`)).status()).toBe(403);
    // ...and a session is not a signature.
    const headers = await leaf.signIn('viewer');
    expect((await request.get(`${origin}${hidden.pathname}`, { headers })).status()).toBe(400);
  });
});
