// Every mock screen as a picture, at one phone width, in Chromium. Locally
// each is compared with its committed baseline, so an unintended change in
// how a screen looks fails. Baselines depend on the OS's fonts and emoji, so
// only the macOS ones are in the repo; CI (Linux) skips the comparison and
// keeps the pictures as a gallery to look through instead.
//
// A picture is the whole screen, top to bottom, not just what a phone shows
// before it is scrolled: most of the settings form and of the calendar is
// below that. The exception is the day viewer, which fills the viewport over
// a page that is inert while it is open.
//
// The minimised screens are pictured at each size of tile they are checked
// at, which is all of them there is to see. So is the card a tile shows once
// "Open gallery" was pressed in chat, which no screen opens on: over the
// day's picture, and on a day that has none.
//
// The clock is fixed (fixtures.ts), so the calendar's dates do not move, and
// animations are stopped. One width, at 1x, keeps the baselines small.

import { mkdir } from 'node:fs/promises';
import path from 'node:path';

import type { Page, TestInfo } from '@playwright/test';

import { watchChat } from '../support/chat';
import { CI } from '../support/env';
import { expect, FULL_SCREENS, test, TILE_SCREENS, TILE_SIZES } from '../support/mock';
import { viewport } from '../support/viewports';

test.use({ ...viewport(375).use, deviceScaleFactor: 1 });

/**
 * How far a pixel's colour may be off and still count as the same (0 to 1).
 * Tighter than Playwright's 0.2: white cards on a cream page differ by so
 * little that a changed corner radius passed at the default.
 */
const COLOUR_TOLERANCE = 0.05;

/**
 * Loads the pictures that wait to be scrolled to (`loading="lazy"`). A
 * picture of the whole page shows them all, and which of them a browser has
 * fetched by then is its own business: without this, a different set each run.
 */
async function loadLazyImages(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const waiting = [...document.images].filter((img) => !img.complete);
    for (const img of waiting) img.loading = 'eager';
    // A picture that fails to decode is the page guard's to report, not this.
    await Promise.all(waiting.map((img) => img.decode().catch(() => undefined)));
  });
}

/**
 * Saves the screen's picture to the gallery (every screen's, in one folder,
 * whatever happens next) and, outside CI, compares it with its baseline.
 */
async function picture(
  page: Page,
  testInfo: TestInfo,
  name: string,
  fullPage: boolean,
): Promise<void> {
  const gallery = path.join(testInfo.project.outputDir, 'gallery');
  await mkdir(gallery, { recursive: true });
  await page.screenshot({ path: path.join(gallery, name), animations: 'disabled', fullPage });
  if (!CI) {
    await expect(page).toHaveScreenshot(name, { threshold: COLOUR_TOLERANCE, fullPage });
  }
}

for (const { id } of FULL_SCREENS) {
  test(id, async ({ page, mock }, testInfo) => {
    await mock.open(id);
    const fullPage = (await page.locator('[aria-modal="true"]').count()) === 0;
    if (fullPage) await loadLazyImages(page);
    await picture(page, testInfo, `${id}.png`, fullPage);
  });
}

for (const size of TILE_SIZES) {
  const named = `${size.width}x${size.height}`;
  test.describe(`minimised, ${named}`, () => {
    test.use({ viewport: size });

    for (const { id } of TILE_SCREENS) {
      test(id, async ({ page, mock }, testInfo) => {
        await mock.open(id);
        await picture(page, testInfo, `${id}-${named}.png`, false);
      });
    }

    // Daily Sketch's Day 126 has a picture; Day 119 was imported without one.
    for (const [name, day] of [
      ['tile-waiting', 126],
      ['tile-waiting-no-picture', 119],
    ] as const) {
      test(`${name}: a press in chat, waiting for a tap`, async ({ page, mock }, testInfo) => {
        const chat = await watchChat(page);
        await mock.open('tile-series');
        await chat.pressOpenGallery(7, day);
        await chat.nextAsk();
        await expect(page.getByText('Tap to open')).toBeVisible();
        await expect(page.getByRole('main').locator('img')).toHaveCount(day === 126 ? 1 : 0);
        await picture(page, testInfo, `${name}-${named}.png`, false);
      });
    }
  });
}
