// The series calendar, on the seeded "Daily Sketch": 150 days posted from
// November 2025 to May 2026, with a whole month never posted in, two days
// that share a date, and a day whose file was never saved. The dates come
// from the server, worked out in the guild's timezone (America/Chicago),
// while the device looking at them is in Europe/Berlin.

import type { FrameLocator } from '@playwright/test';

import { expect, test, type Discord, type LeafServer } from '../support/app';
import { cell, DRAWN, month } from '../support/screens';

/**
 * Opens the gallery from #daily-sketch as a member. Daily Sketch is the one
 * series they can see that archives from there, so the gallery opens on it.
 */
async function openDailySketch(discord: Discord, leaf: LeafServer): Promise<FrameLocator> {
  const { activity: app } = await discord.launch('viewer', {
    channelId: leaf.seed.guilds.main.channels.daily_sketch.id,
  });
  await expect(app.getByRole('heading', { level: 1, name: 'Daily Sketch' })).toBeVisible();
  await expect(cell(app, leaf.seed.series.long.last_day)).toBeVisible();
  return app;
}

test('months run newest first, and the month with no posts is one line', async ({
  discord,
  leaf,
}) => {
  const app = await openDailySketch(discord, leaf);

  // Month headings and the lines between them, in page order.
  const headings = app.locator('[data-month]').getByRole('heading', { level: 2 });
  await expect(headings.or(app.getByText(/^No days archived in /))).toHaveText([
    'May 2026',
    'April 2026',
    'March 2026',
    'No days archived in February 2026',
    'January 2026',
    'December 2025',
    'November 2025',
  ]);
  await expect(month(app, '2026-02')).toHaveCount(0);
  // The month picker offers the months that have something in them.
  await expect(app.getByLabel('Jump to month').locator('option:not([disabled])')).toHaveText([
    'May 2026',
    'April 2026',
    'March 2026',
    'January 2026',
    'December 2025',
    'November 2025',
  ]);

  // The first and the last day sit on the dates they were posted.
  await expect(cell(app, 1)).toHaveAccessibleName('Day 1, Thursday, November 13, 2025');
  await expect(cell(app, 187)).toHaveAccessibleName('Day 187, Monday, May 18, 2026');
  // Every archived day has a cell, and the two that share a date share one.
  await expect(app.locator('[data-days]')).toHaveCount(leaf.seed.series.long.archived_days - 1);
});

test('the stats are the server’s: runs, totals and the last post', async ({ discord, leaf }) => {
  const app = await openDailySketch(discord, leaf);
  const stats = app.getByRole('region', { name: 'Series statistics' });
  await expect(stats.getByRole('term')).toHaveText([
    'Latest run',
    'Longest run',
    'Days archived',
    'Skipped day numbers',
  ]);
  await expect(stats.getByRole('definition')).toHaveText(['33 days', '41 days', '150', '37']);
  await expect(stats.getByText('Last post 2 days ago')).toBeVisible();
});

test('two days posted on one date share its cell, dated in the guild’s timezone', async ({
  discord,
  leaf,
}) => {
  const [first, second] = leaf.seed.long_series.same_date_days as [number, number];
  const app = await openDailySketch(discord, leaf);

  // Day 34 was posted at 03:30 UTC on December 16: still the 15th in
  // Chicago, where the guild is, and already the 16th in Berlin, where this
  // device is. The server's date wins.
  const shared = cell(app, second);
  await expect(shared).toHaveAttribute('data-days', `${first} ${second}`);
  await expect(shared).toHaveAccessibleName(`Day ${first} and 1 more, Monday, December 15, 2025`);
  await expect(shared.getByText('15', { exact: true })).toBeVisible();
  await expect(shared.getByText('+1', { exact: true })).toBeVisible();
  await expect(app.locator('[data-days][aria-label*="December 16, 2025"]')).toHaveCount(0);
  await expect(app.getByText('Dates are in America/Chicago time.')).toBeVisible();

  // The cell opens its first day; the second is the next one, with the same date.
  await shared.click();
  const viewer = app.getByRole('dialog');
  await expect(viewer).toHaveAccessibleName(`Day ${first}, Daily Sketch`);
  await expect(viewer.locator('time')).toHaveText('Dec 15, 2025');
  await viewer.getByRole('button', { name: 'Next day' }).click();
  await expect(viewer).toHaveAccessibleName(`Day ${second}, Daily Sketch`);
  await expect(viewer.locator('time')).toHaveText('Dec 15, 2025');
  await expect(viewer.locator('time')).toHaveAttribute('datetime', '2025-12-16T03:30:00.000Z');
});

test('a day with no saved file gets the hatched tile and says so', async ({ discord, leaf }) => {
  const day = leaf.seed.long_series.missing_media_day;
  const app = await openDailySketch(discord, leaf);

  await expect(cell(app, day)).toHaveAccessibleName(
    `Day ${day}, Monday, January 26, 2026, no preview`,
  );
  await expect(cell(app, day).locator('img')).toHaveCount(0);

  // The month picker brings its month to the top of the screen, where the
  // tile is drawn. Its neighbours' pictures are due now: they arrive
  // through the media proxy, and are pictures.
  await app.getByLabel('Jump to month').selectOption({ label: 'January 2026' });
  await expect(app.getByRole('heading', { level: 2, name: 'January 2026' })).toBeInViewport();
  await expect(app.getByRole('heading', { level: 2, name: 'January 2026' })).toBeFocused();
  await expect(cell(app, day).locator(DRAWN.noPicture)).toBeVisible();
  for (const neighbour of [day - 1, day + 1]) {
    const thumb = cell(app, neighbour).locator('img');
    await expect(thumb).toHaveAttribute('src', /^\/api\/media\/\d+\?.*thumb=1/);
    await expect
      .poll(() => thumb.evaluate((img: HTMLImageElement) => img.naturalWidth))
      .toBeGreaterThan(0);
  }
});

test('the day tools: the latest day, and a day number that was never archived', async ({
  discord,
  leaf,
}) => {
  const app = await openDailySketch(discord, leaf);
  const viewer = app.getByRole('dialog');

  await app.getByRole('button', { name: 'Latest: Day 187' }).click();
  await expect(viewer).toHaveAccessibleName('Day 187, Daily Sketch');
  await viewer.getByRole('button', { name: 'Close' }).click();
  await expect(viewer).toHaveCount(0);

  // Day 60 was skipped: the closest day opens, and the page says which.
  await app.getByLabel('Go to day number').fill('60');
  await app.getByRole('button', { name: 'Go', exact: true }).click();
  await expect(viewer).toHaveAccessibleName('Day 61, Daily Sketch');
  await viewer.getByRole('button', { name: 'Close' }).click();
  await expect(
    app.getByText('Day 60 isn’t archived, so Day 61, the closest, opened.'),
  ).toBeVisible();
});

test.describe('late in the evening in the guild, already tomorrow on the device', () => {
  // 02:00 UTC on May 20: 21:00 on the 19th in Chicago, 04:00 on the 20th in Berlin.
  test.use({ now: new Date('2026-05-20T02:00:00Z') });

  test('"today" is the guild’s date, not the device’s', async ({ discord, leaf }) => {
    // Posted on the 19th at 13:00 Chicago time.
    const added = await leaf.addDay('long');
    expect(added).toMatchObject({ day: 188, local_date: '2026-05-19' });
    const app = await openDailySketch(discord, leaf);

    await expect(cell(app, 188)).toHaveAccessibleName('Day 188, Tuesday, May 19, 2026, today');
    // One date in May is drawn as today, and it is that one.
    const may = month(app, '2026-05');
    await expect(may.locator(DRAWN.today)).toHaveAttribute('data-days', '188');
    // The 20th has not begun there: it is past today, not a missed day.
    await expect(may.locator(DRAWN.outsideTheSeries).filter({ hasText: /^20$/ })).toHaveCount(1);
  });
});
