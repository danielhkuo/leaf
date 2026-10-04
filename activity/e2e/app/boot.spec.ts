// How the Activity starts inside Discord: the SDK's handshake with the
// client, the permission prompt, the code exchange with leaf's server, and
// what the person is told when one of those does not happen. The client is
// the stand-in (support/discord.ts); the Activity, its SDK and the server
// are the real ones.

import type { Page } from '@playwright/test';

import { expect, expectRefusal, test, watchApi } from '../support/app';
import { FRAME_ID } from '../support/discord';
import { APP_CLIENT_ID, APP_NAME, FIXED_NOW } from '../support/env';

/** leaf gives Discord this long to answer the handshake (handshake.ts). */
const READY_TIMEOUT_MS = 12_000;
/** After this long on one step the boot screen admits it is slow (session.svelte.ts). */
const SLOW_AFTER_MS = 4_000;

/**
 * A command only the Embedded App SDK names (leaf never sends it). The
 * script it is found in is the one the SDK was built into, whatever the
 * build calls that file and however it splits the code: a file's name says
 * nothing, since a build that folded the SDK into the entry script would
 * still put out a small `discord-…js` that passes it on.
 */
const SDK_MARK = 'ENCOURAGE_HW_ACCELERATION';

interface Scripts {
  /** The script files the page and its frames have asked for, as paths. */
  paths: string[];
  /** Those of them that hold the SDK, once every one asked for has arrived. */
  withSdk(): Promise<string[]>;
}

/** Watches the scripts a page loads from now on. */
function watchScripts(page: Page): Scripts {
  const paths: string[] = [];
  const bodies: Promise<{ path: string; text: string | null }>[] = [];
  page.on('request', (request) => {
    if (request.resourceType() !== 'script') return;
    const path = new URL(request.url()).pathname;
    paths.push(path);
    const body = request.response().then((response) => response?.text() ?? null);
    bodies.push(
      body.then(
        (text) => ({ path, text }),
        () => ({ path, text: null }),
      ),
    );
  });
  return {
    paths,
    async withSdk() {
      const loaded = await Promise.all(bodies);
      // A script that could not be read might be the one: that is not a "no".
      expect(loaded.filter(({ text }) => text === null)).toEqual([]);
      return loaded.filter(({ text }) => text?.includes(SDK_MARK)).map(({ path }) => path);
    },
  };
}

/**
 * Stops the page's clock at the usual instant, so that leaf's timers move
 * only when a test runs them with `page.clock.runFor`. How long the browser
 * takes to reach a line is then no part of the test.
 */
async function holdClock(page: Page): Promise<void> {
  await page.clock.install({ time: FIXED_NOW });
  // A clock stops at an instant it has not passed yet: a second on is one.
  await page.clock.pauseAt(FIXED_NOW.getTime() + 1_000);
}

/** What the SDK opens with: the application the bundle was built for, in the frame Discord made. */
const HANDSHAKE = {
  v: 1,
  encoding: 'json',
  client_id: APP_CLIENT_ID,
  frame_id: FRAME_ID,
  sdk_version: expect.stringMatching(/^\d+\.\d+\.\d+$/),
};

test.describe('a launch from a server channel', () => {
  test('on a phone: one handshake, one prompt, one exchange, then the gallery', async ({
    discord,
    leaf,
    page,
  }) => {
    const { personas, guilds } = leaf.seed;
    const api = watchApi(page);
    const scripts = watchScripts(page);
    const client = await discord.launch('creator', { platform: 'mobile' });
    const app = client.activity;

    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
    // Signed in as the creator: only they are offered their own series.
    await expect(app.getByRole('button', { name: 'Manage my series' })).toBeVisible();
    // The SDK arrived, in one script. (The plain-tab test below holds the
    // landing page to loading none that has it, which means something only
    // while the SDK can be told by its mark.)
    expect(await scripts.withSdk()).toHaveLength(1);

    // The SDK introduced itself once, as this application, in this frame.
    expect(await client.handshakes()).toEqual([HANDSHAKE]);
    // `identify` and nothing more, without a consent screen where Discord
    // already has the person's OK.
    expect(await client.commands('AUTHORIZE')).toEqual([
      {
        cmd: 'AUTHORIZE',
        evt: null,
        args: {
          client_id: APP_CLIENT_ID,
          response_type: 'code',
          state: '',
          prompt: 'none',
          scope: ['identify'],
        },
      },
    ]);
    // The token Discord is shown is the one leaf's server got for the code.
    expect(await client.commands('AUTHENTICATE')).toEqual([
      { cmd: 'AUTHENTICATE', evt: null, args: { access_token: personas.creator.access_token } },
    ]);
    await expect
      .poll(() => client.commands('SUBSCRIBE'))
      .toEqual([{ cmd: 'SUBSCRIBE', evt: 'ACTIVITY_LAYOUT_MODE_UPDATE', args: null }]);
    expect(api.filter((call) => call.includes('/token'))).toEqual(['POST /api/token']);
    expect(api).toContain(`GET /api/guilds/${guilds.main.id}/series`);

    // The steps for archiving are the phone's, and name the series' channel.
    await app.getByRole('button', { name: /^Daily Sketch/ }).click();
    await app.getByText('How to archive a post').click();
    await expect(app.getByText('Minimise leaf, then post your photo or video in')).toBeVisible();
    await expect(app.getByText('#daily-sketch')).toBeVisible();
    await expect(app.getByText('Press and hold your message.')).toBeVisible();
    await expect(app.getByText('Scroll down in the menu to find it.')).toBeVisible();
    // The app is named as Discord lists it (the stand-in's application name),
    // not as "leaf".
    await expect(
      app.getByText(`Choose ${APP_NAME}, then Archive to Series.`, { exact: true }),
    ).toBeVisible();
  });

  test('on desktop: the same handshake, and the steps are the desktop ones', async ({
    discord,
  }) => {
    const client = await discord.launch('creator', { platform: 'desktop' });
    const app = client.activity;
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();

    expect(await client.handshakes()).toEqual([HANDSHAKE]);
    await app.getByRole('button', { name: /^Daily Sketch/ }).click();
    await app.getByText('How to archive a post').click();
    await expect(app.getByText('Right-click your message, choose')).toBeVisible();
  });
});

test('a launch from a DM says where leaf opens, without a permission prompt', async ({
  discord,
  guard,
  page,
}) => {
  guard.allow(/^console warning: leaf: boot stopped \(no_guild\)/);
  const api = watchApi(page);
  const client = await discord.launch('viewer', { guildId: null });
  const app = client.activity;

  await expect(
    app.getByRole('heading', { level: 1, name: 'leaf opens from a server' }),
  ).toBeVisible();
  await expect(
    app.getByText('Galleries belong to servers, so there is nothing to show'),
  ).toBeVisible();
  // A state to explain, not a failure: no alert, nothing to retry, no details.
  await expect(app.getByRole('alert')).toHaveCount(0);
  await expect(app.getByRole('button')).toHaveText(['Close leaf']);
  await expect(app.getByText('Details')).toHaveCount(0);
  // Discord was never asked for the person's permission, nor leaf for a session.
  expect(await client.commands('AUTHORIZE')).toEqual([]);
  expect(api).toEqual([]);

  // Close leaf asks Discord to close the Activity, as a normal close.
  await app.getByRole('button', { name: 'Close leaf' }).click();
  await expect
    .poll(() => client.closes())
    .toEqual([{ code: 1000, message: 'Closed by the user', nonce: expect.any(String) }]);
  // The stand-in leaves the frame up, as a client that ignores the request
  // would: leaf then says how to leave.
  await expect(
    app.getByText('If leaf is still open, close it with Discord’s own controls.'),
  ).toBeVisible();
});

test('a declined permission prompt is explained, and Grant access asks Discord again', async ({
  discord,
  guard,
  page,
}) => {
  guard.allow(/^console warning: leaf: boot stopped \(consent_declined\)/);
  const api = watchApi(page);
  const client = await discord.launch('viewer', { authorize: 'decline' });
  const app = client.activity;

  await expect(app.getByRole('heading', { level: 1, name: 'leaf needs your OK' })).toBeVisible();
  await expect(app.getByText('leaf never posts as you.')).toBeVisible();
  // What Discord said is there for whoever has to debug a setup, folded away.
  await app.getByText('Details').click();
  await expect(app.getByText('authorize: 5000 OAuth2 Error: access_denied')).toBeVisible();
  expect(api).toEqual([]);

  await client.behave({ authorize: 'grant' });
  await app.getByRole('button', { name: 'Grant access' }).click();
  await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();

  // The second prompt ran over the same connection: Discord answers one
  // handshake per frame.
  expect(await client.commands('AUTHORIZE')).toHaveLength(2);
  expect(await client.handshakes()).toHaveLength(1);
  expect(api.filter((call) => call.includes('/token'))).toEqual(['POST /api/token']);
});

test.describe('a client that is slow to answer', () => {
  // These run leaf's own timers, so the page's clock is held and driven by
  // hand (`holdClock`) in place of the fixed one every other test gets.
  test.use({ now: null });

  test('no READY: the timeout screen, and the boot carries on if READY comes after all', async ({
    discord,
    guard,
    page,
  }) => {
    guard.allow(/^console error: leaf: boot stopped \(ready_timeout\)/);
    await holdClock(page);
    const api = watchApi(page);
    const client = await discord.launch('viewer', { ready: false });
    const app = client.activity;

    await expect(app.getByText('Opening leaf…')).toBeVisible();
    await expect.poll(() => client.handshakes()).toHaveLength(1);
    // Up to the last millisecond leaf says nothing more; then it admits the wait.
    await page.clock.runFor(SLOW_AFTER_MS - 1);
    await expect(app.getByText('Opening leaf…')).toBeVisible();
    await page.clock.runFor(1);
    await expect(app.getByText('Still connecting to Discord…')).toBeVisible();

    // And it gives Discord its full twelve seconds, no less.
    await page.clock.runFor(READY_TIMEOUT_MS - SLOW_AFTER_MS - 1);
    await expect(app.getByText('Still connecting to Discord…')).toBeVisible();
    await page.clock.runFor(1);
    await expect(
      app.getByRole('heading', { level: 1, name: 'Discord didn’t respond' }),
    ).toBeVisible();
    await expect(app.getByText('Close leaf and open it again.')).toBeVisible();
    // Repeating the handshake cannot help, so the only button is the way out.
    await expect(app.getByRole('button')).toHaveText(['Close leaf']);
    expect(await client.commands()).toEqual([]);
    expect(api).toEqual([]);

    // Discord answers late. Nobody has to press anything.
    await client.behave({ ready: true });
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
    expect(await client.handshakes()).toHaveLength(1);
  });

  test('a permission prompt left open: leaf says what it is waiting for and offers a way out', async ({
    discord,
    page,
  }) => {
    await holdClock(page);
    const client = await discord.launch('viewer', { authorize: 'wait' });
    const app = client.activity;

    await expect(app.getByText('Signing you in…')).toBeVisible();
    await expect.poll(() => client.commands('AUTHORIZE')).toHaveLength(1);
    await page.clock.runFor(SLOW_AFTER_MS - 1);
    await expect(app.getByText('Signing you in…')).toBeVisible();
    await page.clock.runFor(1);
    await expect(app.getByText('Waiting for your OK in Discord…')).toBeVisible();
    await expect(
      app.getByText('If Discord isn’t asking, close leaf and open it again.'),
    ).toBeVisible();
    await expect(app.getByRole('button', { name: 'Close leaf' })).toBeVisible();

    // The person answers the prompt: the same request is granted.
    await client.behave({ authorize: 'grant' });
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
    expect(await client.commands('AUTHORIZE')).toHaveLength(1);
  });
});

test('signed in while leaf waits on Discord for the membership: a placeholder, then the gallery', async ({
  discord,
  leaf,
}) => {
  // Discord holds its answer to "is this person in the server?" until told
  // otherwise, so the state in between can be looked at without racing it.
  await leaf.discordApi({ mode: 'slow', delay_ms: 3_600_000, only: ['guild_member'] });
  const client = await discord.launch('viewer');
  const app = client.activity;

  await leaf.discordHolds();
  // The handshake is done and the session exists; only the list is waiting.
  expect(await client.commands('AUTHENTICATE')).toHaveLength(1);
  await expect(app.getByRole('status').filter({ hasText: 'Loading the gallery' })).toBeVisible();
  await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toHaveCount(0);

  await leaf.discordApi({ mode: 'ok' });
  await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();
});

for (const call of ['exchange_code', 'current_user'] as const) {
  test(`Discord down for leaf's ${call} call: the screen says so, and Try again finishes the same boot`, async ({
    discord,
    leaf,
    guard,
    page,
  }) => {
    await leaf.discordApi({ mode: 'down', only: [call] });
    expectRefusal(guard, 503, 'POST', /\/api\/token/);
    guard.allow(/^console error: leaf: boot stopped \(discord_unavailable\)/);
    const api = watchApi(page);
    const client = await discord.launch('viewer');
    const app = client.activity;

    await expect(
      app.getByRole('heading', { level: 1, name: 'Discord isn’t answering' }),
    ).toBeVisible();
    await expect(
      app.getByText('leaf can’t reach Discord right now. Try again in a moment.'),
    ).toBeVisible();
    await expect(app.getByRole('button')).toHaveText(['Try again', 'Close leaf']);

    await leaf.discordApi({ mode: 'ok' });
    await app.getByRole('button', { name: 'Try again' }).click();
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
    await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();

    // The retry resumed at the exchange: the same connection, and the code
    // Discord already handed out, sent to leaf's server a second time.
    expect(await client.handshakes()).toHaveLength(1);
    expect(await client.commands('AUTHORIZE')).toHaveLength(1);
    expect(api.filter((sent) => sent.includes('/token'))).toEqual([
      'POST /api/token',
      'POST /api/token',
    ]);
  });
}

test('the public address in a plain browser tab explains where leaf lives', async ({ page }) => {
  const scripts = watchScripts(page);
  await page.goto('/');
  await expect(page.getByRole('heading', { level: 1, name: 'leaf is running' })).toBeVisible();
  await expect(
    page.getByText('The gallery opens inside Discord, not in a browser tab.'),
  ).toBeVisible();
  await expect(page.getByRole('link', { name: 'Open the admin panel' })).toHaveAttribute(
    'href',
    '/admin',
  );
  // Nobody here can sign in, so the SDK is never downloaded: the page's own
  // scripts arrived, and none of them holds it.
  expect(scripts.paths.length).toBeGreaterThan(0);
  expect(await scripts.withSdk()).toEqual([]);
});
