// What an open gallery does about things that happen in chat while it is
// open: a day the bot archives, and a press on a post's "Open gallery"
// button. Nothing tells the Activity either happened. It looks again when
// the person comes back to it, which Discord reports as a change of layout
// mode, and the page is never reloaded: inside Discord it cannot be.

import { expect, test, watchApi } from '../support/app';
import { cell, seriesNames } from '../support/screens';

/** leaf treats signals this close together as one return (actions.ts). */
const SAME_RETURN_MS = 1_000;
/** A return skips the refetch when the data is younger than this (Gallery.svelte). */
const FRESH_FOR_MS = 30_000;
/** The tile Discord for Android shrinks a minimised Activity to, in CSS px. */
const TILE = { width: 120, height: 120 };
/** How often a minimised leaf asks for a press in chat (Gallery.svelte). */
const TILE_ASKS_EVERY_MS = 4_000;

test.describe('a day archived while the gallery is open', () => {
  test('is on the calendar when the person comes back to leaf', async ({
    discord,
    leaf,
    page,
    time,
  }) => {
    const client = await discord.launch('viewer', {
      channelId: leaf.seed.guilds.main.channels.daily_sketch.id,
    });
    const app = client.activity;
    await expect(cell(app, 187)).toBeVisible();
    await expect(cell(app, 188)).toHaveCount(0);
    const api = watchApi(page);

    // Off to chat, with leaf shrunk to picture-in-picture. The bot archives
    // a post there.
    await client.layout('pip');
    await leaf.addDay('long');
    await time.pass(FRESH_FOR_MS + 1_000);
    await expect(cell(app, 188)).toHaveCount(0);

    await client.layout('focused');
    await expect(cell(app, 188)).toHaveAccessibleName('Day 188, Tuesday, May 19, 2026');
    await expect(app.getByRole('button', { name: 'Latest: Day 188' })).toBeVisible();
    await expect(app.getByRole('region', { name: 'Series statistics' })).toContainText(
      'Days archived 151',
    );
    // Its picture is one the page had never asked for, and it arrived.
    await expect
      .poll(() =>
        cell(app, 188)
          .locator('img')
          .evaluate((img: HTMLImageElement) => img.naturalWidth),
      )
      .toBeGreaterThan(0);

    // The same page, the same session: nothing booted again.
    expect(await client.handshakes()).toHaveLength(1);
    expect(api).not.toContain('POST /api/token');
    const { guilds, series } = leaf.seed;
    expect(api).toContain(`GET /api/guilds/${guilds.main.id}/series/${series.long.id}/days`);
  });

  test('is not fetched for on a return within half a minute, until Refresh is pressed', async ({
    discord,
    leaf,
    page,
    time,
  }) => {
    const { guilds } = leaf.seed;
    const client = await discord.launch('viewer', {
      channelId: guilds.main.channels.daily_sketch.id,
    });
    const app = client.activity;
    await expect(cell(app, 187)).toBeVisible();
    await leaf.addDay('long');
    const api = watchApi(page);

    // A glance at chat and back. The return still asks whether a button was
    // pressed there, and once that has been answered nothing else was asked.
    await client.layout('pip');
    await time.pass(SAME_RETURN_MS);
    const asked = page.waitForResponse((response) => response.url().includes('/launch-intent'));
    await client.layout('focused');
    await asked;
    expect(api.map((call) => call.replace(guilds.main.id, ':guild'))).toEqual([
      'GET /api/guilds/:guild/launch-intent',
    ]);
    await expect(cell(app, 188)).toHaveCount(0);

    await app.getByRole('button', { name: 'Refresh' }).click();
    await expect(app.getByText('Up to date.')).toBeVisible();
    await expect(cell(app, 188)).toBeVisible();
  });
});

test.describe('an "Open gallery" press in chat', () => {
  test('before leaf opens: the gallery starts on that series and day', async ({
    discord,
    leaf,
  }) => {
    await leaf.pressOpenGallery('viewer', 'reminder', 3);
    // Opened from a channel no series lives in, so nothing else decides.
    const { activity: app } = await discord.launch('viewer');

    const viewer = app.getByRole('dialog');
    await expect(viewer).toHaveAccessibleName('Day 3, Evening Walks');
    await expect(viewer.getByRole('img')).toBeVisible();
    // Under the day is its series, and under that the list: Back never
    // dead-ends on a deep link.
    await viewer.getByRole('button', { name: 'Close' }).click();
    await expect(app.getByRole('heading', { level: 1, name: 'Evening Walks' })).toBeVisible();
    await app.getByRole('button', { name: 'Back to series list' }).click();
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
  });

  test('for a series the person may not see opens nothing', async ({ discord, leaf, page }) => {
    await leaf.pressOpenGallery('viewer', 'creator_only', 1);
    const answer = page.waitForResponse((response) => response.url().includes('/launch-intent'));
    const { activity: app } = await discord.launch('viewer');

    // The server keeps the press to itself: the answer does not even say
    // that there is such a series.
    expect(await (await answer).json()).toBeNull();
    // And the gallery opens where it would have anyway: on the list, which
    // does not have the series.
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
    await expect(seriesNames(app)).toHaveText(['Daily Sketch', 'Evening Walks']);
    await expect(app.getByRole('dialog')).toHaveCount(0);
  });

  test('while leaf is open: the day opens when the person comes back', async ({
    discord,
    leaf,
    time,
  }) => {
    const client = await discord.launch('viewer');
    const app = client.activity;
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();

    await client.layout('pip');
    await leaf.pressOpenGallery('viewer', 'long', 20);
    await time.pass(SAME_RETURN_MS);
    await client.layout('focused');

    const viewer = app.getByRole('dialog');
    await expect(viewer).toHaveAccessibleName('Day 20, Daily Sketch');
    await expect(viewer.getByRole('button', { name: 'Photo 1 of 3' })).toBeVisible();
    await viewer.getByRole('button', { name: 'Close' }).click();
    await expect(app.getByRole('heading', { level: 1, name: 'Daily Sketch' })).toBeVisible();
    expect(await client.handshakes()).toHaveLength(1);
  });

  test('while leaf is a tile: the tile says to tap, and the tap finds the day open', async ({
    discord,
    leaf,
    page,
  }) => {
    const { guilds } = leaf.seed;
    const client = await discord.launch('viewer', {
      channelId: guilds.main.channels.daily_sketch.id,
    });
    const app = client.activity;
    await expect(cell(app, 187)).toBeVisible();
    const phone = page.viewportSize();
    if (!phone) throw new Error('the page has no viewport to come back to');

    // Minimised as Discord for Android does it: a tile beside the chat. A
    // press there brings nothing forward, so a tile asks for it, on a timer.
    await client.layout('pip');
    await page.setViewportSize(TILE);
    const tile = app.getByRole('main');
    await expect(tile.locator('h1, p')).toHaveText(['Daily Sketch', 'Day 187']);
    const api = watchApi(page);

    await leaf.pressOpenGallery('viewer', 'long', 20);
    await expect(async () => {
      await page.clock.runFor(TILE_ASKS_EVERY_MS);
      await expect(tile.locator('h1, p')).toHaveText(['Daily Sketch', 'Day 20', 'Tap to open'], {
        timeout: 1_000,
      });
    }).toPass();
    expect(api).toContain(`GET /api/guilds/${guilds.main.id}/launch-intent`);
    // Behind the tile, where nothing can be pressed.
    await expect(app.getByRole('dialog')).toHaveCount(0);
    await expect(app.getByRole('button')).toHaveCount(0);

    // The tap is Discord's. It opens leaf, which has the day up already: the
    // press was handed out once, and the return finds none to ask for.
    await page.setViewportSize(phone);
    await client.layout('focused');
    const viewer = app.getByRole('dialog');
    await expect(viewer).toHaveAccessibleName('Day 20, Daily Sketch');
    await expect(viewer.getByRole('button', { name: 'Photo 1 of 3' })).toBeVisible();
    await viewer.getByRole('button', { name: 'Close' }).click();
    await expect(app.getByRole('heading', { level: 1, name: 'Daily Sketch' })).toBeVisible();
    expect(await client.handshakes()).toHaveLength(1);
  });

  test('on a post archived a moment ago: the gallery fetches it, then opens it', async ({
    discord,
    leaf,
    time,
  }) => {
    const client = await discord.launch('viewer');
    const app = client.activity;
    await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();

    // The list on screen ends at Day 187. The button is on Day 188.
    await client.layout('pip');
    await leaf.addDay('long', { caption: 'Fresh off the desk' });
    await leaf.pressOpenGallery('viewer', 'long', 188);
    await time.pass(SAME_RETURN_MS);
    await client.layout('focused');

    const viewer = app.getByRole('dialog');
    await expect(viewer).toHaveAccessibleName('Day 188, Daily Sketch');
    await expect(viewer.getByText('Fresh off the desk')).toBeVisible();
    await expect(viewer.getByRole('img')).toBeVisible();
  });
});

test('an activity link opens the series and day in its custom_id', async ({ discord, leaf }) => {
  const { id, name } = leaf.seed.series.reminder;
  const { activity: app } = await discord.launch('viewer', { customId: `s${id}d2` });
  await expect(app.getByRole('dialog')).toHaveAccessibleName(`Day 2, ${name}`);
});
