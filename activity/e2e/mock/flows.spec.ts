// The journeys a person takes through the mock gallery, on a phone: browse
// to a day and page through it by touch and by button, start a series,
// change one's settings, run the admin panel, get out of a failed boot. The
// mock API keeps what is done to it until the page reloads, so each journey
// runs to its end. Last, what the screens do on a device set to reduce
// motion or to force its own colours.

import type { Locator, Page } from '@playwright/test';

import { auditLayout } from '../support/layout';
import { expect, FULL_SCREENS, test } from '../support/mock';
import { endlessMotion, RESTLESS } from '../support/motion';
import { Touch, type Point } from '../support/touch';
import { viewport } from '../support/viewports';

test.use(viewport(375).use);

/** The middle of an element, in viewport pixels. */
async function centre(target: Locator): Promise<Point> {
  const box = await target.boundingBox();
  if (!box) throw new Error('the element is not on screen');
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

const shifted = (from: Point, dx: number): Point => ({ x: from.x + dx, y: from.y });

/** The open day viewer, which is named after the day it shows. */
function viewerOn(page: Page, day: number): Locator {
  return page.getByRole('dialog', { name: `Day ${day}, Daily Sketch` });
}

/** The photo on show in the open viewer: where a finger goes to swipe or pinch. */
function photo(page: Page): Locator {
  return page.getByRole('dialog').getByRole('img');
}

/** The dot for one of a day's files; it is `aria-current` while that file shows. */
function photoDot(page: Page, n: number, of: number): Locator {
  return page.getByRole('button', { name: `Photo ${n} of ${of}` });
}

async function expectPhoto(page: Page, n: number, of: number): Promise<void> {
  await expect(photoDot(page, n, of)).toHaveAttribute('aria-current', 'true');
}

/** The calendar cell holding a day. */
function dayCell(page: Page, day: number): Locator {
  return page.locator(`[data-days~="${day}"]`);
}

test.describe('browsing', () => {
  // Daily Sketch's Day 125 has three photos; 124 was skipped and 126 has one.

  test('from the series list to a day, paging files then days by swipe and by button', async ({
    page,
    mock,
    browserName,
  }) => {
    const touch = await Touch.on(page, browserName);
    await mock.open('picker');

    await page.getByRole('button', { name: /^Daily Sketch/ }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Daily Sketch' })).toBeVisible();
    await dayCell(page, 125).click();
    await expect(viewerOn(page, 125)).toBeVisible();
    await expectPhoto(page, 1, 3);

    // Swipes go through the day's files first...
    const middle = await centre(photo(page));
    const left = [shifted(middle, 90), shifted(middle, -90)] as const;
    const right = [shifted(middle, -90), shifted(middle, 90)] as const;
    await touch.swipe(...left);
    await expectPhoto(page, 2, 3);
    await touch.swipe(...left);
    await expectPhoto(page, 3, 3);
    // ...then cross to the next day,
    await touch.swipe(...left);
    await expect(viewerOn(page, 126)).toBeVisible();
    // and coming back lands on the last file of the day before.
    await touch.swipe(...right);
    await expect(viewerOn(page, 125)).toBeVisible();
    await expectPhoto(page, 3, 3);

    // The arrows do the same. Their names say what the next press does.
    await page.getByRole('button', { name: 'Previous photo' }).click();
    await expectPhoto(page, 2, 3);
    await page.getByRole('button', { name: 'Previous photo' }).click();
    await expectPhoto(page, 1, 3);
    await page.getByRole('button', { name: 'Previous day' }).click();
    // Day 124 was never used: the day before 125 is 123.
    await expect(viewerOn(page, 123)).toBeVisible();
    await page.getByRole('button', { name: 'Next day' }).click();
    await expect(viewerOn(page, 125)).toBeVisible();
    await expectPhoto(page, 1, 3);

    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(dayCell(page, 125)).toBeFocused();
  });

  test('Close hands focus to the cell of the day that was on screen', async ({ page, mock }) => {
    await mock.open('home');
    await dayCell(page, 126).click();
    await expect(viewerOn(page, 126)).toBeVisible();
    await page.getByRole('button', { name: 'Next day' }).click();
    await expect(viewerOn(page, 127)).toBeVisible();

    await page.getByRole('button', { name: 'Close' }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(dayCell(page, 127)).toBeFocused();
    await expect(dayCell(page, 127)).toBeInViewport();
  });

  test('a pinch zooms the photo and never turns the page', async ({ page, mock, browserName }) => {
    const touch = await Touch.on(page, browserName);
    await mock.open('viewer');
    await expectPhoto(page, 1, 3);
    const zoomOut = page.getByRole('button', { name: 'Zoom out' });
    await expect(zoomOut).toBeDisabled();

    // Two fingers together, both wandering further than a swipe needs.
    const middle = await centre(photo(page));
    const a = { from: shifted(middle, 60), to: shifted(middle, 15) };
    const b = { from: shifted(middle, -60), to: shifted(middle, -15) };
    await touch.down(0, a.from);
    await touch.down(1, b.from);
    await touch.glide({ finger: 0, ...a }, { finger: 1, ...b });
    // One finger lifts; the other drags on, sideways, and then lifts. That
    // is the end of a pinch, not a swipe.
    await touch.up(1);
    await touch.glide({ finger: 0, from: a.to, to: shifted(a.to, -120) });
    await touch.up(0);
    await expectPhoto(page, 1, 3);
    await expect(viewerOn(page, 125)).toBeVisible();

    // Spreading two fingers zooms in, and still does not turn the page.
    await touch.down(0, shifted(middle, 20));
    await touch.down(1, shifted(middle, -20));
    await touch.glide(
      { finger: 0, from: shifted(middle, 20), to: shifted(middle, 110) },
      { finger: 1, from: shifted(middle, -20), to: shifted(middle, -110) },
    );
    await touch.up(1);
    await touch.up(0);
    await expect(zoomOut).toBeEnabled();
    await expectPhoto(page, 1, 3);

    // The same fingers do turn the page once the photo is back to size:
    // what was held above was the gesture, not a deaf screen.
    while (await zoomOut.isEnabled()) await zoomOut.click();
    await touch.swipe(shifted(middle, 90), shifted(middle, -90));
    await expectPhoto(page, 2, 3);
  });
});

test.describe('a video day', () => {
  // Daily Sketch's Day 118 is a 480 x 270 clip; its poster is a 256px thumbnail.

  interface Box {
    x: number;
    y: number;
    width: number;
    height: number;
  }

  interface Measured {
    /** Where the video's own box is. */
    video: Box;
    /** The stage's box inside its padding: what a photo's frame fills. */
    stage: Box;
    /** The size the player knows the clip to be; 0x0 until its metadata is in. */
    known: string;
    fit: string;
  }

  function measure(video: Locator): Promise<Measured> {
    return video.evaluate((v: HTMLVideoElement): Measured => {
      const tenth = (n: number): number => Math.round(n * 10) / 10;
      const box = (r: Box): Box => ({
        x: tenth(r.x),
        y: tenth(r.y),
        width: tenth(r.width),
        height: tenth(r.height),
      });
      const stage = v.parentElement;
      if (!stage) throw new Error('the video has no parent');
      const outer = stage.getBoundingClientRect();
      const style = getComputedStyle(stage);
      const [top, right, bottom, left] = [
        style.paddingTop,
        style.paddingRight,
        style.paddingBottom,
        style.paddingLeft,
      ].map(Number.parseFloat) as [number, number, number, number];
      return {
        video: box(v.getBoundingClientRect()),
        stage: box({
          x: outer.x + left,
          y: outer.y + top,
          width: outer.width - left - right,
          height: outer.height - top - bottom,
        }),
        known: `${v.videoWidth}x${v.videoHeight}`,
        fit: getComputedStyle(v).objectFit,
      };
    });
  }

  // A phone held upright, and on its side, where the stage is the short,
  // wide half of the screen beside the caption.
  for (const [held, size] of [
    ['upright', null],
    ['on its side', { width: 740, height: 360 }],
  ] as const) {
    test(`the video’s box is the stage’s from the first paint, through metadata and playback (${held})`, async ({
      page,
      mock,
      guard,
    }) => {
      if (size) await page.setViewportSize(size);
      // A player stops a download it has enough of.
      guard.allow(
        /^request failed: GET \S+\/clip\.(webm|mp4)\S* \((net::ERR_ABORTED|cancelled)\)$/,
      );
      // The clip is held back, as on a phone that fetches nothing until Play:
      // all the player has is the poster.
      let release = (): void => undefined;
      const held = new Promise<void>((resolve) => (release = resolve));
      // (The file itself: the module that names it ends in `?url`.)
      await page.route(/\/clip\.(webm|mp4)$/, async (route) => {
        await held;
        await route.continue();
      });
      await mock.open('viewer-video', { unsettled: true });
      const video = viewerOn(page, 118).locator('video');
      await expect(video).toBeVisible();
      await expect(video).toHaveAttribute('poster', /^data:image\/svg/);

      const before = await measure(video);
      expect(before.known).toBe('0x0');
      // Fitted inside the box, not stretched to it.
      expect(before.fit).toBe('contain');
      expect(before.video).toEqual(before.stage);
      // A box worth the name: the 256px poster would be far smaller.
      expect(before.video.width).toBeGreaterThan(300);
      expect(before.video.height).toBeGreaterThan(300);

      // The metadata arrives: the player now knows the clip's size.
      release();
      await expect.poll(async () => (await measure(video)).known).toBe('480x270');
      expect((await measure(video)).video).toEqual(before.video);

      // And it plays.
      await video.evaluate(async (v: HTMLVideoElement) => {
        v.muted = true;
        await v.play();
      });
      await expect
        .poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime))
        .toBeGreaterThan(0);
      expect((await measure(video)).video).toEqual(before.video);
      await expect(page.getByText('This video didn’t play')).toHaveCount(0);
    });
  }
});

test.describe('starting a series', () => {
  test('an empty name is refused on the field; a valid one lands on the first-post card', async ({
    page,
    mock,
  }) => {
    await mock.open('create');
    const name = page.getByLabel('Series name');
    const start = page.getByRole('button', { name: 'Start a series' });

    await start.click();
    await expect(name).toHaveAttribute('aria-invalid', 'true');
    // The message is the field's description. Its opening words, not the sentence.
    await expect(name).toHaveAccessibleDescription(/^Give the series a name/);
    await expect(name).toBeFocused();
    // Nothing was created: this is still the form.
    await expect(page.getByRole('heading', { level: 1, name: 'Start a series' })).toBeVisible();

    await name.fill('Evening Walks');
    // The message goes as soon as the field is edited.
    await expect(name).not.toHaveAttribute('aria-invalid');
    await start.click();

    await expect(page.getByRole('heading', { level: 1, name: 'Evening Walks' })).toBeVisible();
    await expect(page.getByText('Series created')).toBeVisible();
    await expect(
      page.getByRole('heading', { level: 2, name: 'Archive your first post' }),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Check again' })).toBeVisible();
  });
});

test.describe('a series whose channel is gone', () => {
  // Daily Sketch's channel was deleted in Discord: leaf can no longer see it.
  const gone = (page: Page): Locator => page.getByText('This series’ channel is gone');
  /** The first of the archive steps, behind "How to archive a post". */
  async function firstStep(page: Page): Promise<Locator> {
    const howTo = page.getByRole('group').filter({ hasText: 'How to archive a post' });
    await howTo.getByText('How to archive a post').click();
    return howTo.getByRole('listitem').first();
  }

  test('the owner is told, the steps name no channel, and Series settings takes another', async ({
    page,
    mock,
  }) => {
    await mock.open('home-channel-gone');
    await expect(gone(page)).toBeVisible();
    await expect(
      page.getByText(
        'It was deleted or hidden from leaf, so nothing posted there can be archived.',
      ),
    ).toBeVisible();
    await expect(await firstStep(page)).toHaveText(
      'Minimise leaf, then post your photo or video in one of this server’s series channels.',
    );

    await page.getByRole('button', { name: 'Choose another channel' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Series settings' })).toBeVisible();
    const channel = page.getByLabel('Channel', { exact: true });
    await expect(channel.locator('option:checked')).toHaveText(
      'Its current channel (no longer allowed)',
    );
    await channel.selectOption({ label: '#art-share' });
    await page.getByRole('button', { name: 'Save changes' }).click();
    await expect(page.getByText('Saved', { exact: true })).toBeVisible();

    // Back on the series, there is nothing left to warn about.
    await page.getByRole('button', { name: 'Back' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Daily Sketch' })).toBeVisible();
    await expect(await firstStep(page)).toHaveText(
      'Minimise leaf, then post your photo or video in #art-share.',
    );
    await expect(gone(page)).toHaveCount(0);
  });

  test('where it is still the server’s only series channel, Series settings says who can fix it', async ({
    page,
    mock,
  }) => {
    // No admin has run /setup since the channel was deleted, so the server
    // still offers it, and nothing else.
    await mock.open('settings-channel-unseen');
    const channel = page.getByLabel('Channel', { exact: true });
    const whatToDo = page.getByText(
      'leaf can’t see this channel, and this server has no other series channel it can see. ' +
        'If it was deleted or hidden, ask a server admin to choose new series channels with /setup.',
    );
    await expect(channel.locator('option')).toHaveText(['A channel leaf can’t see (…0005)']);
    await expect(whatToDo).toBeVisible();
    await expect(page.getByText(/choose another/)).toHaveCount(0);

    // It is where the series' own warning leads.
    await page.getByRole('button', { name: 'Back' }).click();
    await expect(gone(page)).toBeVisible();
    await page.getByRole('button', { name: 'Choose another channel' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Series settings' })).toBeVisible();
    await expect(whatToDo).toBeVisible();
  });

  test('a series whose channel is there says nothing of it', async ({ page, mock }) => {
    await mock.open('home');
    await expect(await firstStep(page)).toHaveText(
      'Minimise leaf, then post your photo or video in #daily-sketch.',
    );
    await expect(gone(page)).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Choose another channel' })).toHaveCount(0);
  });
});

test.describe('series settings', () => {
  const saved = (page: Page): Locator => page.getByText('Saved', { exact: true });
  const unsaved = (page: Page): Locator => page.getByText('Unsaved changes', { exact: true });
  const leave = (page: Page): Locator => page.getByRole('group', { name: 'Leave without saving?' });

  test('Save follows the form: off until a change, "Saved" after, and Back asks in between', async ({
    page,
    mock,
  }) => {
    await mock.open('settings');
    const save = page.getByRole('button', { name: 'Save changes' });
    const description = page.getByLabel('Description');
    await expect(save).toBeDisabled();

    await description.fill('Ink and wash, one page a day.');
    await expect(unsaved(page)).toBeVisible();
    await expect(save).toBeEnabled();

    // Back with unsaved changes asks first, and "Keep editing" keeps them.
    await page.getByRole('button', { name: 'Back' }).click();
    await expect(leave(page)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Keep editing' })).toBeFocused();
    await page.getByRole('button', { name: 'Keep editing' }).click();
    await expect(leave(page)).toHaveCount(0);
    await expect(description).toHaveValue('Ink and wash, one page a day.');

    await save.click();
    await expect(saved(page)).toBeVisible();
    await expect(save).toBeDisabled();

    // Nothing unsaved now: Back just leaves.
    await page.getByRole('button', { name: 'Back' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'My series' })).toBeVisible();
    // What was saved is what the form opens with next time.
    await page.getByRole('button', { name: /^Daily Sketch/ }).click();
    await expect(page.getByLabel('Description')).toHaveValue('Ink and wash, one page a day.');
  });

  test('"Discard changes" leaves and throws the edit away', async ({ page, mock }) => {
    await mock.open('settings');
    await page.getByLabel('Series name').fill('Renamed by mistake');
    await expect(unsaved(page)).toBeVisible();

    await page.getByRole('button', { name: 'Back' }).click();
    await page.getByRole('button', { name: 'Discard changes' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'My series' })).toBeVisible();

    await page.getByRole('button', { name: /^Daily Sketch/ }).click();
    await expect(page.getByLabel('Series name')).toHaveValue('Daily Sketch');
  });
});

test.describe('admin panel', () => {
  test('the pickers list the server’s roles and channels by name', async ({ page, mock }) => {
    await mock.open('admin-panel');
    await expect(page.getByLabel('Timezone', { exact: true })).toHaveValue('America/Chicago');
    await expect(page.getByLabel('Creator role').locator('option')).toHaveText([
      'Anyone can start a series',
      '@Artist',
      '@Patron',
      '@Member',
    ]);
    await expect(page.getByLabel('Log channel').locator('option')).toHaveText([
      'No log',
      '#general',
      '#daily-sketch',
      '#leaf-log',
    ]);
  });

  test('a changed setting saves', async ({ page, mock }) => {
    await mock.open('admin-panel');
    const save = page.getByRole('button', { name: 'Save changes' });
    await expect(save).toBeDisabled();

    await page.getByLabel('Series per member').fill('6');
    await expect(page.getByText('Unsaved changes', { exact: true })).toBeVisible();
    await save.click();

    await expect(page.getByText('Saved', { exact: true })).toBeVisible();
    await expect(save).toBeDisabled();
    await expect(page.getByLabel('Series per member')).toHaveValue('6');
  });

  test('Revoke asks before it acts', async ({ page, mock }) => {
    await mock.open('admin-panel');
    const row = page.getByRole('listitem').filter({ hasText: 'Daily Sketch' });
    const question = row.getByRole('group', { name: /^Hide “Daily Sketch” from the gallery/ });

    await row.getByRole('button', { name: 'Revoke' }).click();
    await expect(question).toBeFocused();
    await question.getByRole('button', { name: 'Cancel' }).click();
    await expect(question).toHaveCount(0);
    await expect(row.getByText('Active')).toBeVisible();

    await row.getByRole('button', { name: 'Revoke' }).click();
    await question.getByRole('button', { name: 'Revoke' }).click();
    await expect(row.getByRole('status')).toHaveText(/^Revoked\./);
    await expect(row.getByRole('button', { name: 'Restore' })).toBeVisible();
    await expect(row.getByRole('button', { name: 'Revoke' })).toHaveCount(0);
  });
});

test.describe('ways out of a failed start', () => {
  const button = (page: Page, name: string): Locator => page.getByRole('button', { name });

  test('Discord never answered: Close, and the detail behind a disclosure', async ({
    page,
    mock,
  }) => {
    await mock.open('error');
    await expect(
      page.getByRole('heading', { level: 1, name: 'Discord didn’t respond' }),
    ).toBeVisible();
    await expect(button(page, 'Close leaf')).toBeEnabled();
    // Trying again cannot help here, so it is not offered.
    await expect(button(page, 'Try again')).toHaveCount(0);

    await page.getByText('Details', { exact: true }).click();
    await expect(page.getByText('ready: no answer from Discord')).toBeVisible();
  });

  test('leaf’s server did not answer: Try again and Close', async ({ page, mock }) => {
    await mock.open('error-retry');
    await expect(button(page, 'Try again')).toBeEnabled();
    await expect(button(page, 'Close leaf')).toBeEnabled();
  });

  test('waiting on a permission sheet that is not there: Close', async ({ page, mock }) => {
    await mock.open('loading-consent');
    await expect(button(page, 'Close leaf')).toBeEnabled();
  });

  test('the session ended: Close', async ({ page, mock }) => {
    await mock.open('expired');
    await expect(button(page, 'Close leaf')).toBeEnabled();
  });

  test('the gallery did not load: Try again', async ({ page, mock }) => {
    await mock.open('load-error');
    await expect(button(page, 'Try again')).toBeEnabled();
  });

  test('a series that is gone: Check again, and Back to the list', async ({ page, mock }) => {
    await mock.open('unavailable');
    await expect(button(page, 'Check again')).toBeEnabled();
    await button(page, 'Back to series').click();
    await expect(page.getByRole('heading', { level: 1, name: 'Series' })).toBeVisible();
  });
});

test.describe('reduced motion', () => {
  test.use({ reducedMotion: 'reduce' });

  for (const { id } of FULL_SCREENS) {
    test(`${id} has nothing moving without end`, async ({ page, mock }) => {
      await mock.open(id);
      expect(await endlessMotion(page)).toEqual([]);
    });
  }

  // The screens above show what the app animates today. These two hold what
  // none of them shows: app.css's reset, which has to stop an animation
  // nobody wrote a rule for, and a skeleton, which is gone by the time a
  // screen has loaded. checks.spec.ts sees both move when motion is allowed.

  test('an endless animation with no rule of its own is cut to a single run', async ({
    page,
    mock,
  }) => {
    await mock.open('picker');
    await page
      .getByRole('main')
      .evaluate((main, html) => main.insertAdjacentHTML('beforeend', html), RESTLESS);
    await expect(page.getByText('Restless')).toBeVisible();
    expect(await endlessMotion(page)).toEqual([]);
  });

  test('a skeleton stands still while a screen loads', async ({ page, mock }) => {
    await mock.openLoading('picker');
    await expect(page.getByRole('status')).toHaveText(/^Loading/);
    expect(await endlessMotion(page)).toEqual([]);
  });
});

test.describe('forced colours', () => {
  test.use({ forcedColors: 'active' });
  // No WebKit browser has the mode. Playwright's answers the media query and
  // forces nothing, so a select there would be measured in a state no one sees.
  test.skip(({ browserName }) => browserName === 'webkit', 'WebKit forces no colours');

  // Forced colours drop every gradient, and the chevron app.css draws on a
  // select is two of them: there the platform's own arrow has to come back.
  test('a select is the platform’s own again, arrow included', async ({ page, mock }) => {
    await mock.open('settings');
    const select = page.getByRole('combobox').first();
    const style = await select.evaluate((el) => {
      const { appearance, backgroundImage } = getComputedStyle(el);
      return { appearance, backgroundImage };
    });
    // One `none` for each of the chevron's two layers.
    expect(style).toEqual({ appearance: 'auto', backgroundImage: 'none, none' });
    expect(await auditLayout(page)).toMatchObject({
      pageOverflow: 0,
      outside: [],
      smallTargets: [],
      smallFonts: [],
    });
  });
});
