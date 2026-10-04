// The admin panel: a plain browser page at /admin, outside Discord. An admin
// signs in with Discord (the consent screen is the stand-in), picks a
// server, changes its settings and manages its series. What is saved here is
// read back from the panel after a reload, from the API, and from the
// gallery a member then opens.

import type { Locator, Page } from '@playwright/test';

import { expect, expectRefusal, test, type PersonaKey, type Seed } from '../support/app';
import { standInForConsent } from '../support/discord';
import { APP_CLIENT_ID } from '../support/env';
import { seriesNames } from '../support/screens';
import { viewport } from '../support/viewports';

// Admins run servers from a desktop browser more than from a phone. (The
// mock suite measures this page at every width.) The project is a phone in
// more than its width, so the rest of that is undone here too: touch, the
// 3x screen and the iPhone's user agent. A value cannot clear the last
// (`undefined` there means "what the project says"), so it is given as the
// fixture Playwright itself starts from: the engine's own.
test.use({
  ...viewport(1280).use,
  isMobile: false,
  hasTouch: false,
  deviceScaleFactor: 1,
  userAgent: [({ contextOptions }, use) => use(contextOptions.userAgent), { scope: 'test' }],
});

/** Signs in at /admin as `persona`, through leaf's own login and callback routes. */
async function signIn(page: Page, seed: Seed, persona: PersonaKey): Promise<URL[]> {
  const asked = await standInForConsent(page, { code: seed.personas[persona].code });
  await page.goto('/admin');
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();
  return asked;
}

/** Signs in as the admin and opens the seeded, set-up server's panel. */
async function openGarden(page: Page, seed: Seed): Promise<void> {
  await signIn(page, seed, 'admin');
  await page.getByRole('button', { name: /^Leaf Test Garden/ }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Leaf Test Garden' })).toBeVisible();
  await expect(page.getByLabel('Timezone')).toBeVisible();
}

/** One series' row in the panel's list. */
function row(page: Page, name: string): Locator {
  return page
    .getByRole('region', { name: 'Series' })
    .getByRole('listitem')
    .filter({ hasText: name });
}

test('the panel is opened in a desktop browser, not the project’s phone', async ({ page }) => {
  await page.goto('/admin');
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  const browser = await page.evaluate(() => ({
    userAgent: navigator.userAgent,
    width: window.innerWidth,
    pixelRatio: window.devicePixelRatio,
    touchPoints: navigator.maxTouchPoints,
    mouse: window.matchMedia('(hover: hover) and (pointer: fine)').matches,
  }));
  expect(browser.userAgent).not.toMatch(/iPhone|Android|Mobile/);
  expect(browser).toMatchObject({ width: 1280, pixelRatio: 1, touchPoints: 0, mouse: true });
});

test.describe('signing in', () => {
  test('goes through Discord’s consent and comes back to the servers the admin manages', async ({
    page,
    leaf,
  }) => {
    const { seed } = leaf;
    await page.goto('/admin');
    await expect(
      page.getByRole('heading', { level: 1, name: 'Manage leaf in your server' }),
    ).toBeVisible();

    const asked = await standInForConsent(page, { code: seed.personas.admin.code });
    await page.getByRole('link', { name: 'Sign in with Discord' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Choose a server' })).toBeVisible();

    // What leaf asked Discord for: this application, the guild list (to know
    // what the person manages), an answer at its own callback, and a state
    // to recognise the answer by.
    expect(asked).toHaveLength(1);
    const [authorize] = asked as [URL];
    expect(authorize.origin + authorize.pathname).toBe('https://discord.com/oauth2/authorize');
    expect(Object.fromEntries(authorize.searchParams)).toEqual({
      response_type: 'code',
      client_id: APP_CLIENT_ID,
      scope: 'identify guilds',
      redirect_uri: `${seed.origin}/admin/callback`,
      state: expect.stringMatching(/^\d+\.[\w-]{20,}$/),
      prompt: 'none',
    });

    // Both servers the admin manages, with their names from Discord and
    // their series counted. The token is out of the address bar.
    await expect(page.getByRole('list').getByRole('button')).toHaveText([
      /Leaf Test Garden\s*6 series/,
      /Unconfigured Server\s*0 series/,
    ]);
    expect(page.url()).toBe(`${seed.origin}/admin`);

    // A server where /setup was never run says so, and still opens.
    await page.getByRole('button', { name: /^Unconfigured Server/ }).click();
    await expect(
      page.getByRole('heading', { level: 1, name: 'Unconfigured Server' }),
    ).toBeVisible();
    await expect(page.getByText('leaf isn’t set up in this server yet')).toBeVisible();
    expect(page.url()).toBe(`${seed.origin}/admin?guild=${seed.guilds.unset.id}`);
    await expect(page.getByText('No series yet.')).toBeVisible();
  });

  for (const persona of ['viewer', 'outsider'] as const) {
    test(`${persona}: someone who manages no server gets a card that says so, not an error body`, async ({
      page,
      leaf,
    }) => {
      const documents: string[] = [];
      page.on('response', (response) => {
        if (response.request().resourceType() !== 'document') return;
        documents.push(`${response.status()} ${response.headers()['content-type'] ?? ''}`);
      });
      await signIn(page, leaf.seed, persona);

      await expect(
        page.getByRole('heading', { level: 1, name: 'No server to manage' }),
      ).toBeVisible();
      await expect(page.getByText('You need the Manage Server permission')).toBeVisible();
      await expect(page.getByRole('link', { name: 'Sign in again' })).toHaveAttribute(
        'href',
        '/admin/login',
      );
      // The page is leaf's own, back at /admin: every document on the way
      // was a page or a redirect, never a JSON refusal left on screen.
      expect(page.url()).toBe(`${leaf.seed.origin}/admin`);
      expect(documents.filter((seen) => !/^(200 text\/html|30\d )/.test(seen))).toEqual([]);
      await expect(page.getByRole('button', { name: 'Sign out' })).toHaveCount(0);

      // And the API has nothing for them.
      const list = await page.request.get('/api/admin/guilds', {
        headers: { authorization: `Bearer ${await leaf.mintAdminToken(persona)}` },
      });
      expect(await list.json()).toEqual([]);
    });
  }

  test('a cancelled consent comes back to the sign-in card, saying nothing changed', async ({
    page,
  }) => {
    await standInForConsent(page, { error: 'access_denied' });
    await page.goto('/admin');
    await page.getByRole('link', { name: 'Sign in with Discord' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Sign-in cancelled' })).toBeVisible();
    await expect(
      page.getByText('You cancelled the Discord sign-in, so nothing changed.'),
    ).toBeVisible();
  });
});

test('the creator role and the timezone are picked by name, saved, and still there after a reload', async ({
  page,
  leaf,
}) => {
  const { seed } = leaf;
  const { roles } = seed.guilds.main;
  const patches: unknown[] = [];
  page.on('request', (request) => {
    if (request.method() === 'PATCH') patches.push(request.postDataJSON());
  });
  await openGarden(page, seed);

  // The stored values, shown by name: nobody types an id or a zone.
  const timezone = page.getByLabel('Timezone');
  const creatorRole = page.getByLabel('Creator role');
  await expect(timezone).toHaveValue('America/Chicago');
  await expect(creatorRole).toHaveValue(roles.artists.id);
  await expect(creatorRole.locator('option')).toHaveText([
    'Anyone can start a series',
    '@Artists',
    '@Patrons',
    '@Members',
  ]);
  const save = page.getByRole('button', { name: 'Save changes' });
  await expect(save).toBeDisabled();

  await timezone.selectOption('Asia/Tokyo');
  await creatorRole.selectOption({ label: '@Patrons' });
  await save.click();
  await expect(page.getByRole('region', { name: 'Settings' }).getByRole('status')).toHaveText(
    'Saved',
  );
  // Only the two settings that changed were sent.
  expect(patches).toEqual([{ timezone: 'Asia/Tokyo', creator_role_id: roles.patrons.id }]);

  // A reload asks the server again: this is what it stored.
  await page.reload();
  await expect(page.getByRole('heading', { level: 1, name: 'Leaf Test Garden' })).toBeVisible();
  await expect(page.getByLabel('Timezone')).toHaveValue('Asia/Tokyo');
  await expect(page.getByLabel('Creator role')).toHaveValue(roles.patrons.id);
  await expect(page.getByRole('button', { name: 'Save changes' })).toBeDisabled();

  // And it is what the gallery now goes by. The patron may start a series;
  // the creator, who holds the old role, is told which one it takes now.
  const patron = await (await leaf.get('patron', '/series/eligibility')).json();
  expect(patron).toMatchObject({ can_create: true, violations: [] });
  const creator = await (await leaf.get('creator', '/series/eligibility')).json();
  expect(creator).toMatchObject({
    can_create: false,
    violations: [{ code: 'missing_creator_role', params: { role_name: 'Patrons' } }],
  });
  const series = (await (await leaf.get('viewer', '/series')).json()) as { timezone: string }[];
  expect(series.map((listed) => listed.timezone)).toEqual(['Asia/Tokyo', 'Asia/Tokyo']);
});

test('revoking a series takes it out of the gallery, and restoring brings it back', async ({
  page,
  leaf,
  discord,
}) => {
  const { seed } = leaf;
  await openGarden(page, seed);
  const sketch = row(page, 'Daily Sketch');
  await expect(sketch).toContainText('by Mika · Active');

  // Revoke asks first, in place of the buttons, so a second tap cannot land on it.
  await sketch.getByRole('button', { name: 'Revoke' }).click();
  await expect(sketch.getByRole('group')).toContainText('Hide “Daily Sketch” from the gallery');
  await sketch.getByRole('group').getByRole('button', { name: 'Revoke' }).click();
  await expect(sketch.getByRole('status')).toHaveText(/^Revoked\./);
  await expect(sketch).toContainText('by Mika · Revoked');
  await expect(sketch.getByRole('button', { name: 'Restore' })).toBeVisible();

  // leaf said so in the server's log channel.
  const log = await leaf.postedMessages();
  expect(log).toHaveLength(1);
  expect(log[0]?.channel_id).toBe(seed.guilds.main.channels.leaf_log.id);
  expect(log[0]?.content).toContain('Daily Sketch');
  expect(log[0]?.content).toMatch(/revoked/i);

  // A member's gallery no longer has it, and the API no longer serves it.
  expect((await leaf.get('viewer', `/series/${seed.series.long.id}/days`)).status()).toBe(404);
  const { activity: app } = await discord.launch('viewer');
  await expect(app.getByRole('heading', { level: 1, name: 'Evening Walks' })).toBeVisible();
  await app.getByRole('button', { name: 'Back to series list' }).click();
  await expect(seriesNames(app)).toHaveText(['Evening Walks']);

  // Back in the panel (still signed in), the admin restores it.
  await page.goto(`/admin?guild=${seed.guilds.main.id}`);
  await row(page, 'Daily Sketch').getByRole('button', { name: 'Restore' }).click();
  await expect(row(page, 'Daily Sketch').getByRole('status')).toHaveText('Restored.');
  await expect(row(page, 'Daily Sketch')).toContainText('by Mika · Active');
  const listed = (await (await leaf.get('viewer', '/series')).json()) as { name: string }[];
  expect(listed.map((series) => series.name)).toEqual(['Daily Sketch', 'Evening Walks']);
});

test('an admin session the server no longer accepts returns to sign-in, and signing in comes back to the same server', async ({
  page,
  leaf,
  guard,
}) => {
  const { seed } = leaf;
  // A token that has run out, as an hour-old sign-in would have.
  const stale = await leaf.mintAdminToken('admin', -60);
  expectRefusal(guard, 401, 'GET', /\/api\/admin\/guilds/);
  await standInForConsent(page, { code: seed.personas.admin.code });

  await page.goto(`/admin?guild=${seed.guilds.main.id}#token=${stale}`);
  await expect(page.getByRole('heading', { level: 1, name: 'Your session expired' })).toBeVisible();
  await page.getByRole('link', { name: 'Sign in again' }).click();
  // Straight to the server the link was for, not the list.
  await expect(page.getByRole('heading', { level: 1, name: 'Leaf Test Garden' })).toBeVisible();
  expect(page.url()).toBe(`${seed.origin}/admin?guild=${seed.guilds.main.id}`);
});
