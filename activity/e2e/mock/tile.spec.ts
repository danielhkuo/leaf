// leaf minimised. Discord shrinks an Activity the person has stepped away
// from to a small tile beside the chat (about 120 x 120 CSS px on Android),
// and a tap on it is Discord's own: it opens leaf again. So in a viewport
// that small leaf shows one card and nothing else, and the screen that was
// open waits behind it, as it was left.
//
// The mock's minimised screens are ordinary screens opened in a viewport the
// size of a tile: it is the size that turns them into the card, as it is
// inside Discord.

import type { Locator, Page } from '@playwright/test';

import { axeViolations } from '../support/axe';
import { watchChat } from '../support/chat';
import { expect, test, TILE_SCREENS, TILE_SIZES, type ScreenId } from '../support/mock';
import { auditTile, CARD } from '../support/tile';
import { viewport } from '../support/viewports';

/** A phone's touch and user agent; each test brings its own size. */
const PHONE = viewport(360).use;
const PHONE_SIZE = { width: 360, height: 740 };
/** The longest name leaf allows, with nowhere to wrap (`&long=1`). */
const LONGEST_NAME = 'W'.repeat(40);

interface Card {
  /** A series' name, or leaf's own. */
  name: string;
  /** The line under the name; `null` when the card has none. */
  detail: string | null;
  /** Whether a thumbnail fills the tile behind the words. */
  pictured: boolean;
  /** Whether the name is someone's text, which `&long=1` stretches. */
  named?: true;
}

/** What each minimised screen's card says. */
const CARDS: Partial<Record<ScreenId, Card>> = {
  'tile-day': { name: 'Daily Sketch', detail: 'Day 125', pictured: true, named: true },
  'tile-series': { name: 'Daily Sketch', detail: 'Day 128', pictured: true, named: true },
  'tile-empty': { name: 'Pressed Flowers', detail: 'No days yet', pictured: false, named: true },
  'tile-list': { name: 'leaf', detail: null, pictured: false },
  'tile-boot': { name: 'leaf', detail: 'Opening…', pictured: false },
  'tile-error': { name: 'leaf', detail: 'Didn’t open', pictured: false },
  'tile-expired': { name: 'leaf', detail: 'Session ended', pictured: false },
};

function cardFor(id: ScreenId): Card {
  const card = CARDS[id];
  if (!card) throw new Error(`tile.spec.ts does not say what ${id} shows`);
  return card;
}

const card = (page: Page): Locator => page.locator(CARD);

/**
 * The card is all there is: it covers the tile, none of it is past an edge,
 * nothing behind it can be reached, and no word is cut off but a name too
 * long for any tile (`&long=1`), which ends in an ellipsis on its second line.
 */
async function expectCardFits(page: Page, longText: boolean): Promise<void> {
  // Nothing to press: the tap belongs to Discord.
  await expect(page.getByRole('button')).toHaveCount(0);
  await expect(page.getByRole('link')).toHaveCount(0);

  const tile = await auditTile(page);
  expect(tile.parts).toBeGreaterThan(2);
  expect.soft(tile.scroll, 'the page scrolls by this many px').toEqual({ x: 0, y: 0 });
  expect.soft(tile.outside, 'parts of the card past the edge of the tile').toEqual([]);
  expect.soft(tile.cropped, 'words outside the square Discord shows').toEqual([]);
  expect.soft(tile.behind, 'left behind the card').toEqual([]);
  expect.soft(tile.uncovered, 'points the card does not cover').toEqual([]);
  if (longText) {
    expect.soft(tile.cutOff, 'text cut off').toHaveLength(1);
    expect.soft(tile.cutOff[0], 'text cut off').toMatch(/^h1 /);
  } else {
    expect.soft(tile.cutOff, 'text cut off').toEqual([]);
  }
  expect.soft(await axeViolations(page), 'axe violations').toEqual([]);
}

for (const size of TILE_SIZES) {
  test.describe(`${size.width} x ${size.height}`, () => {
    test.use({ ...PHONE, viewport: size });

    for (const { id } of TILE_SCREENS) {
      const expected = cardFor(id);
      for (const longText of expected.named ? [false, true] : [false]) {
        test(`${id}${longText ? ', long text' : ''}`, async ({ page, mock }) => {
          await mock.open(id, { longText });

          // The card is the page: its one landmark, with its one heading.
          await expect(page.getByRole('main')).toHaveCount(1);
          await expect(card(page)).toBeVisible();
          const name = card(page).getByRole('heading', { level: 1 });
          await expect(name).toHaveText(longText ? LONGEST_NAME : expected.name);
          await expect(card(page).locator('p')).toHaveText(
            expected.detail === null ? [] : [expected.detail],
          );
          await expect(card(page).locator('img')).toHaveCount(expected.pictured ? 1 : 0);
          await expectCardFits(page, longText);
        });
      }
    }
  });
}

test.describe('what counts as a tile', () => {
  test.use(PHONE);

  /** A viewport of this size shows the card (or does not), on a series' screen. */
  async function expectCard(page: Page, shown: boolean): Promise<void> {
    await expect(card(page)).toHaveCount(shown ? 1 : 0);
    // The series' own screen is there for anyone to use exactly when the card is not.
    await expect(page.getByRole('button', { name: 'Refresh' })).toHaveCount(shown ? 0 : 1);
  }

  for (const size of [
    // The narrowest phones, upright and on their side.
    { width: 320, height: 568 },
    { width: 568, height: 320 },
    PHONE_SIZE,
    // One side short of a tile is not a tile.
    { width: 260, height: 260 },
    { width: 200, height: 600 },
  ]) {
    test(`${size.width} x ${size.height} is not one: the screens are themselves`, async ({
      page,
      mock,
    }) => {
      await page.setViewportSize(size);
      for (const { id } of TILE_SCREENS) {
        await mock.open(id);
        await expect(card(page)).toHaveCount(0);
        await expect(page.getByRole('main')).toHaveCount(1);
      }
      await mock.open('tile-series');
      await expectCard(page, false);
    });
  }

  test('259 x 259 is one', async ({ page, mock }) => {
    await page.setViewportSize({ width: 259, height: 259 });
    await mock.open('tile-series');
    await expectCard(page, true);
  });
});

const TILE = TILE_SIZES[0];

/** The calendar cell holding a day. */
const dayCell = (page: Page, day: number): Locator => page.locator(`[data-days~="${day}"]`);

/** Discord shrinks leaf to a tile: the card comes up. */
async function minimise(page: Page): Promise<void> {
  await page.setViewportSize(TILE);
  await expect(card(page)).toBeVisible();
}

/** And opens it again: the card goes. */
async function restore(page: Page): Promise<void> {
  await page.setViewportSize(PHONE_SIZE);
  await expect(card(page)).toHaveCount(0);
}

test.describe('coming back from a tile', () => {
  test.use({ ...PHONE, viewport: PHONE_SIZE });

  test('the calendar is where it was left', async ({ page, mock }) => {
    await mock.open('home');
    // Well down the page: Day 60 is months back.
    await dayCell(page, 60).scrollIntoViewIfNeeded();
    await dayCell(page, 60).focus();
    const left = await page.evaluate(() => window.scrollY);
    expect(left).toBeGreaterThan(500);
    const where = await dayCell(page, 60).boundingBox();

    await minimise(page);
    await expect(card(page).getByRole('heading', { level: 1 })).toHaveText('Daily Sketch');
    await expect(card(page).locator('p')).toHaveText('Day 128');

    await restore(page);
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(left);
    await expect(dayCell(page, 60)).toBeInViewport();
    expect(await dayCell(page, 60).boundingBox()).toEqual(where);
    // And focus is back where it was, not at the top of the page.
    await expect(dayCell(page, 60)).toBeFocused();
  });

  test('the viewer is on the day and the photo it was left on, and the card names that day', async ({
    page,
    mock,
  }) => {
    await mock.open('viewer');
    const viewer = (day: number): Locator =>
      page.getByRole('dialog', { name: `Day ${day}, Daily Sketch` });
    await page.getByRole('button', { name: 'Next photo' }).click();
    await expect(page.getByRole('button', { name: 'Photo 2 of 3' })).toHaveAttribute(
      'aria-current',
      'true',
    );

    await minimise(page);
    await expect(card(page).locator('p')).toHaveText('Day 125');
    await expect(page.getByRole('dialog')).toHaveCount(0);

    await restore(page);
    await expect(viewer(125)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Photo 2 of 3' })).toHaveAttribute(
      'aria-current',
      'true',
    );

    // Paged on to another day, the card follows: Day 126, and its picture.
    await page.getByRole('button', { name: 'Next photo' }).click();
    await page.getByRole('button', { name: 'Next day' }).click();
    await expect(viewer(126)).toBeVisible();
    const shown = await dayCell(page, 126).locator('img').getAttribute('src');
    await minimise(page);
    await expect(card(page).locator('p')).toHaveText('Day 126');
    await expect(card(page).locator('img')).toHaveAttribute('src', shown ?? '');

    await restore(page);
    await expect(viewer(126)).toBeVisible();
    // Closing still lands on the day's cell.
    await page.getByRole('button', { name: 'Close' }).click();
    await expect(dayCell(page, 126)).toBeFocused();
  });

  test('a form keeps what was typed into it', async ({ page, mock }) => {
    await mock.open('create');
    const name = page.getByLabel('Series name');
    await name.fill('Evening Walks');

    await minimise(page);
    // A form is nobody's business on a tile: just leaf.
    await expect(card(page).getByRole('heading', { level: 1 })).toHaveText('leaf');
    await expect(card(page).locator('p')).toHaveCount(0);

    await restore(page);
    await expect(name).toHaveValue('Evening Walks');
    await expect(name).toBeFocused();
  });

  test('keys pressed at the tile do not reach the viewer behind it', async ({ page, mock }) => {
    await mock.open('viewer');
    await expect(page.getByRole('dialog', { name: 'Day 125, Daily Sketch' })).toBeVisible();

    await minimise(page);
    // Enough arrows to leave the day's three photos for the next day, and
    // the key that closes the viewer.
    for (const key of ['ArrowRight', 'ArrowRight', 'ArrowRight', 'ArrowLeft', 'Escape']) {
      await page.keyboard.press(key);
    }
    await expect(card(page).locator('p')).toHaveText('Day 125');

    await restore(page);
    await expect(page.getByRole('dialog', { name: 'Day 125, Daily Sketch' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Photo 1 of 3' })).toHaveAttribute(
      'aria-current',
      'true',
    );
    // The keys are the viewer's again.
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('button', { name: 'Photo 2 of 3' })).toHaveAttribute(
      'aria-current',
      'true',
    );
  });

  test('a video that was playing is paused, and stays paused', async ({ page, mock, guard }) => {
    // A player stops a download it has enough of.
    guard.allow(/^request failed: GET \S+\/clip\.(webm|mp4)\S* \((net::ERR_ABORTED|cancelled)\)$/);
    await mock.open('viewer-video');
    // By its tag: behind the card the dialog around it has no role to find it by.
    const video = page.locator('video');
    const state = (): Promise<{ paused: boolean; at: number }> =>
      video.evaluate((v: HTMLVideoElement) => ({ paused: v.paused, at: v.currentTime }));
    await video.evaluate(async (v: HTMLVideoElement) => {
      v.muted = true;
      // A clip that ran out would be paused by itself.
      v.loop = true;
      await v.play();
    });
    await expect.poll(async () => (await state()).at).toBeGreaterThan(0);
    expect((await state()).paused).toBe(false);

    // Nothing on a tile could stop it.
    await minimise(page);
    await expect.poll(async () => (await state()).paused).toBe(true);

    await restore(page);
    await expect(video).toBeVisible();
    const back = await state();
    expect(back.paused).toBe(true);
    // Where it stopped, for the person to press Play again.
    expect(back.at).toBeGreaterThan(0);
  });

  test('a day opened while leaf was a tile has focus when leaf is opened', async ({
    page,
    mock,
  }) => {
    await page.setViewportSize(TILE);
    await mock.open('tile-day');
    await expect(card(page)).toBeVisible();

    await restore(page);
    const viewer = page.getByRole('dialog', { name: 'Day 125, Daily Sketch' });
    await expect(viewer).toBeVisible();
    // In the dialog, not on the page under it, which is inert.
    await expect(viewer).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(viewer).toHaveCount(0);
  });
});

// "Open gallery" pressed on a post in chat. Discord for Android does not
// bring a running Activity forward for it, so a minimised leaf asks the
// server every few seconds whether there was one, opens it behind the card,
// and the card says to tap. At any other size it asks nothing: the press is
// fetched when the person comes back (e2e/app/freshness.spec.ts).
test.describe('"Open gallery" pressed in chat', () => {
  test.use({ ...PHONE, viewport: PHONE_SIZE });

  /** The card's words: the name, then the lines under it. */
  const words = (page: Page): Locator => card(page).locator('h1, p');

  test('at a phone’s size leaf asks once, as it opens, and not again', async ({ page, mock }) => {
    const chat = await watchChat(page);
    await mock.open('home');
    expect(await chat.asks()).toBe(1);

    // A press nothing comes for: it is still unfetched a quarter of an hour on.
    await chat.pressOpenGallery(7, 126);
    await page.clock.runFor(15 * 60_000);
    expect(await chat.asks()).toBe(1);
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Refresh' })).toBeVisible();
  });

  test('a tile asks every few seconds: the day opens behind the card, which says to tap', async ({
    page,
    mock,
  }) => {
    const chat = await watchChat(page);
    await mock.open('home');
    const thumb = await dayCell(page, 126).locator('img').getAttribute('src');
    expect(thumb).toMatch(/^data:image/);

    await minimise(page);
    await expect(words(page)).toHaveText(['Daily Sketch', 'Day 128']);
    // Nothing was pressed: it asks, and asks again, and the card stays as it is.
    expect(await chat.asks()).toBe(1);
    for (const asked of [2, 3, 4]) {
      await chat.nextAsk();
      expect(await chat.asks()).toBeGreaterThanOrEqual(asked);
    }
    await expect(words(page)).toHaveText(['Daily Sketch', 'Day 128']);

    await chat.pressOpenGallery(7, 126);
    await chat.nextAsk();
    await expect(words(page)).toHaveText(['Daily Sketch', 'Day 126', 'Tap to open']);
    await expect(card(page).locator('img')).toHaveAttribute('src', thumb ?? '');
    // The day is open behind the card, where nothing can reach it.
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expectCardFits(page, false);

    // Tapped: Discord opens leaf, and it is on that day already.
    await restore(page);
    const viewer = page.getByRole('dialog', { name: 'Day 126, Daily Sketch' });
    await expect(viewer).toBeVisible();
    await expect(viewer).toBeFocused();
    // Open again, it stops asking.
    const asked = await chat.asks();
    await page.clock.runFor(15 * 60_000);
    expect(await chat.asks()).toBe(asked);

    // Minimised again on the same day, nothing is waiting behind the card.
    await minimise(page);
    await expect(words(page)).toHaveText(['Daily Sketch', 'Day 126']);
  });

  test('a press on a series opens its calendar behind the card', async ({ page, mock }) => {
    const chat = await watchChat(page);
    await mock.open('picker');
    await minimise(page);
    await expect(words(page)).toHaveText(['leaf']);

    // Pressed Flowers has nothing archived yet.
    await chat.pressOpenGallery(8);
    await chat.nextAsk();
    await expect(words(page)).toHaveText(['Pressed Flowers', 'No days yet', 'Tap to open']);

    await restore(page);
    await expect(page.getByRole('heading', { level: 1, name: 'Pressed Flowers' })).toBeVisible();
  });

  // The card with one line more on it, at every size of tile: over the
  // day's picture, and on a day that has none (Day 119 was imported with no
  // file), where the words have the tile to themselves.
  for (const size of TILE_SIZES) {
    for (const { day, pictured } of [
      { day: 126, pictured: true },
      { day: 119, pictured: false },
    ]) {
      for (const longText of [false, true]) {
        const title =
          `${size.width} x ${size.height}: the card fits ` +
          `${pictured ? 'over a picture' : 'with no picture'}${longText ? ', long text' : ''}`;
        test(title, async ({ page, mock }) => {
          const chat = await watchChat(page);
          await page.setViewportSize(size);
          await mock.open('tile-series', { longText });

          await chat.pressOpenGallery(7, day);
          await chat.nextAsk();
          await expect(card(page).locator('p')).toHaveText([`Day ${day}`, 'Tap to open']);
          await expect(card(page).locator('img')).toHaveCount(pictured ? 1 : 0);
          await expectCardFits(page, longText);
        });
      }
    }
  }
});
