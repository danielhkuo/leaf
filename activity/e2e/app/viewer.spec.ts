// The day viewer over the real media proxy: photos and videos come from
// `/api/media/<attachment>` under the signed addresses the API hands out,
// straight into <img> and <video>. Also what the viewer says when a day is
// gone, and what it asks of Discord to open the original post.

import type { FrameLocator, Locator, Response } from '@playwright/test';

import {
  expect,
  expectRefusal,
  test,
  type Discord,
  type DiscordClient,
  type LeafServer,
} from '../support/app';
import { cell } from '../support/screens';

/** The seed's clips (see the server's fixtures). */
const CLIPS = {
  // What an iPhone plays.
  mp4: { day: 'mp4_day', type: 'video/mp4', bytes: 155_042 },
  // Not every build of Playwright's browsers decodes H.264; all of them play VP9.
  webm: { day: 'webm_day', type: 'video/webm', bytes: 123_460 },
} as const;

/** The gallery on Daily Sketch, as a member who opened leaf in its channel. */
async function openDailySketch(
  discord: Discord,
  leaf: LeafServer,
  platform: 'mobile' | 'desktop' = 'mobile',
): Promise<DiscordClient> {
  const client = await discord.launch('viewer', {
    channelId: leaf.seed.guilds.main.channels.daily_sketch.id,
    platform,
  });
  const app = client.activity;
  await expect(app.getByRole('heading', { level: 1, name: 'Daily Sketch' })).toBeVisible();
  await expect(cell(app, leaf.seed.series.long.last_day)).toBeVisible();
  return client;
}

/** What the API lists for a day of Daily Sketch. */
interface Listed {
  /** Where the media proxy serves each of the day's files, in order: `/api/media/<attachment>`. */
  files: string[];
  /** The post's own address in Discord. */
  jumpUrl: string;
}

async function listed(leaf: LeafServer, day: number): Promise<Listed> {
  const answer = await leaf.get('viewer', `/series/${leaf.seed.series.long.id}/days/${day}`);
  const { media, jump_url } = (await answer.json()) as {
    media: { url: string }[];
    jump_url: string;
  };
  const files = media.map((file) => new URL(file.url, leaf.seed.origin).pathname);
  for (const path of files) expect(path).toMatch(/^\/api\/media\/\d+$/);
  return { files, jumpUrl: jump_url };
}

/** Whether a response is the original (not the thumbnail) of the file served at `path`. */
function isOriginal(response: Response, path: string): boolean {
  const url = new URL(response.url());
  return url.pathname === path && !url.searchParams.has('thumb');
}

interface Photo {
  path: string;
  signed: boolean;
  size: string;
}

/**
 * The picture on show: its address and the size the browser decoded. Until
 * the browser has chosen what to load there is no address, and no size.
 */
function shownPhoto(viewer: Locator): Promise<Photo> {
  return viewer.getByRole('img').evaluate((img: HTMLImageElement): Photo => {
    if (img.currentSrc === '') return { path: '', signed: false, size: '0x0' };
    const url = new URL(img.currentSrc);
    return {
      path: url.pathname,
      signed: url.searchParams.has('sig') && url.searchParams.has('exp'),
      size: `${img.naturalWidth}x${img.naturalHeight}`,
    };
  });
}

test('a three-photo day: each file through the media proxy, files before days', async ({
  discord,
  leaf,
  page,
}) => {
  const day = leaf.seed.long_series.three_images_day;
  const types = new Map<string, string | undefined>();
  page.on('response', (response) => {
    const { pathname, searchParams } = new URL(response.url());
    if (pathname.startsWith('/api/media/') && !searchParams.has('thumb')) {
      types.set(pathname, response.headers()['content-type']);
    }
  });
  const { activity: app } = await openDailySketch(discord, leaf);

  await cell(app, day).click();
  const viewer = app.getByRole('dialog');
  await expect(viewer).toHaveAccessibleName(`Day ${day}, Daily Sketch`);
  await expect(viewer.locator('time')).toHaveText('Dec 2, 2025');
  const dot = (n: number): Locator => viewer.getByRole('button', { name: `Photo ${n} of 3` });

  // The seed's three files for the day: a PNG, a JPEG and a PNG, each a
  // different shape, so a file served under another's address would show.
  const files = [
    { type: 'image/png', size: '640x480' },
    { type: 'image/jpeg', size: '480x640' },
    { type: 'image/png', size: '512x512' },
  ];
  // Where the API says each is served, in the order the viewer pages them.
  const paths = (await listed(leaf, day)).files;
  expect(new Set(paths).size).toBe(files.length);
  for (const [index, path] of paths.entries()) {
    const file = files[index];
    await expect(dot(index + 1)).toHaveAttribute('aria-current', 'true');
    await expect.poll(() => shownPhoto(viewer)).toEqual({ path, signed: true, size: file?.size });
    expect(types.get(path)).toBe(file?.type);
    if (index < paths.length - 1) await viewer.getByRole('button', { name: 'Next photo' }).click();
  }

  // Past the last file the same arrow turns to the next day...
  await viewer.getByRole('button', { name: 'Next day' }).click();
  await expect(viewer).toHaveAccessibleName(`Day ${day + 1}, Daily Sketch`);
  const [nextDay] = (await listed(leaf, day + 1)).files;
  await expect.poll(() => shownPhoto(viewer)).toMatchObject({ path: nextDay });
  // ...and coming back lands on the last file, not the first.
  await viewer.getByRole('button', { name: 'Previous day' }).click();
  await expect(viewer).toHaveAccessibleName(`Day ${day}, Daily Sketch`);
  await expect(dot(3)).toHaveAttribute('aria-current', 'true');
});

test('a video is asked for by byte range and answered with the part asked for', async ({
  discord,
  leaf,
  guard,
  page,
}) => {
  // The engine says which clip it can decode: H.264 where it has it (WebKit
  // on macOS, as on an iPhone), VP9 elsewhere (Playwright's Chromium, and
  // WebKit on Linux when its codecs lack H.264).
  const h264 = await page.evaluate(
    () => document.createElement('video').canPlayType('video/mp4; codecs="avc1.42E01E"') !== '',
  );
  const clip = h264 ? CLIPS.mp4 : CLIPS.webm;
  const day = leaf.seed.long_series[clip.day];
  // A player stops a download it has enough of, and again when the viewer closes.
  guard.allow(/^request failed: GET \S+\/api\/media\/\d+\?\S+ \((net::ERR_ABORTED|cancelled)\)$/);
  // Playwright's WebKit lacks some of its own player's artwork.
  guard.allow(/^console error: Button failed to load, iconName = /);
  const { activity: app } = await openDailySketch(discord, leaf);

  const [file = ''] = (await listed(leaf, day)).files;
  // The first answer with a status: WebKit on Linux drops its opening
  // request and asks again, and the dropped one is reported with status 0.
  const firstPart = page.waitForResponse(
    (response) => isOriginal(response, file) && response.status() !== 0,
  );
  await app.getByLabel('Go to day number').fill(String(day));
  await app.getByRole('button', { name: 'Go', exact: true }).click();
  const viewer = app.getByRole('dialog');
  await expect(viewer).toHaveAccessibleName(`Day ${day}, Daily Sketch`);

  // iOS will not play a source that cannot answer a range request. Players
  // differ in how they start: Chromium and WebKit on macOS open with a
  // range, WebKit on Linux asks for the whole file. Either way the answer
  // has to say that ranges are served.
  const response = await firstPart;
  const asked = response.request().headers().range;
  if (asked === undefined) {
    expect(response.status()).toBe(200);
    expect(response.headers()).toMatchObject({
      'content-length': String(clip.bytes),
      'accept-ranges': 'bytes',
      'content-type': clip.type,
    });
  } else {
    const range = /^bytes=(\d+)-(\d*)$/.exec(asked);
    expect(range, `the request’s Range header, ${asked}`).not.toBeNull();
    const first = Number(range?.[1]);
    const last = range?.[2] ? Number(range[2]) : clip.bytes - 1;
    expect(response.status()).toBe(206);
    expect(response.headers()).toMatchObject({
      'content-range': `bytes ${first}-${last}/${clip.bytes}`,
      'content-length': String(last - first + 1),
      'accept-ranges': 'bytes',
      'content-type': clip.type,
    });
  }

  // And a part asked for outright comes back as that part, whatever the
  // player in this engine chose to do.
  const part = await page.request.get(response.url(), { headers: { Range: 'bytes=1000-1999' } });
  expect(part.status()).toBe(206);
  expect(part.headers()).toMatchObject({
    'content-range': `bytes 1000-1999/${clip.bytes}`,
    'content-length': '1000',
    'accept-ranges': 'bytes',
    'content-type': clip.type,
  });
  expect((await part.body()).byteLength).toBe(1000);

  // The parts add up to something the player can read: it knows the clip's
  // size and length, which are in the file and nowhere else.
  const video = viewer.locator('video');
  await expect(video).toHaveAttribute('poster', new RegExp(`^${file}\\?.*thumb=1`));
  await expect
    .poll(() =>
      video.evaluate((v: HTMLVideoElement) => ({
        size: `${v.videoWidth}x${v.videoHeight}`,
        seconds: Math.round(v.duration),
      })),
    )
    .toEqual({ size: '480x270', seconds: 4 });
  await expect(viewer.getByText('This video didn’t play')).toHaveCount(0);
});

test('a day removed after the calendar loaded: the viewer says so, and a refresh drops it', async ({
  discord,
  leaf,
  guard,
}) => {
  const day = leaf.seed.series.long.last_day;
  const { activity: app } = await openDailySketch(discord, leaf);
  // The calendar has the day, and its picture.
  await expect
    .poll(() =>
      cell(app, day)
        .locator('img')
        .evaluate((img: HTMLImageElement) => img.naturalWidth),
    )
    .toBeGreaterThan(0);

  await leaf.removeDay('long', day);
  const { id } = leaf.seed.series.long;
  expectRefusal(guard, 404, 'GET', new RegExp(`/api/guilds/\\d+/series/${id}/days/${day}`));
  guard.allow(new RegExp(`^console error: leaf: loading Day ${day} failed`));

  await cell(app, day).click();
  const viewer = app.getByRole('dialog');
  await expect(viewer).toHaveAccessibleName(`Day ${day}, Daily Sketch`);
  await expect(viewer.getByRole('alert')).toContainText('Couldn’t load this day');
  await expect(viewer.getByRole('alert')).toContainText('This day isn’t in the archive.');
  // Asking again cannot bring it back, so nothing offers to; the way on is
  // still there.
  await expect(viewer.getByRole('button', { name: 'Try again' })).toHaveCount(0);
  await expect(viewer.getByRole('button', { name: 'Open original post' })).toBeDisabled();
  await viewer.getByRole('button', { name: 'Previous day' }).click();
  await expect(viewer).toHaveAccessibleName(`Day ${day - 1}, Daily Sketch`);
  await expect(viewer.getByRole('img')).toBeVisible();
  await viewer.getByRole('button', { name: 'Close' }).click();

  // The calendar still shows the day until it is asked to look again.
  await expect(cell(app, day)).toBeVisible();
  await app.getByRole('button', { name: 'Refresh' }).click();
  await expect(app.getByText('Up to date.')).toBeVisible();
  await expect(cell(app, day)).toHaveCount(0);
  await expect(app.getByRole('button', { name: `Latest: Day ${day - 1}` })).toBeVisible();
});

test.describe('a day whose file was never saved', () => {
  /** Opens the day and returns the viewer. */
  async function openMissing(app: FrameLocator, day: number): Promise<Locator> {
    await app.getByLabel('Go to day number').fill(String(day));
    await app.getByRole('button', { name: 'Go', exact: true }).click();
    const viewer = app.getByRole('dialog');
    await expect(viewer).toHaveAccessibleName(`Day ${day}, Daily Sketch`);
    await expect(viewer.getByText('No file was saved for this day')).toBeVisible();
    await expect(viewer.locator('img, video')).toHaveCount(0);
    return viewer;
  }

  test('points at the original post, which Discord is asked to open', async ({
    discord,
    leaf,
    page,
  }) => {
    const { guilds, series } = leaf.seed;
    const day = leaf.seed.long_series.missing_media_day;
    // The API lists the file the post had, flagged as never saved, and the
    // post's own address: this server, the series' channel, the message.
    const { files, jumpUrl } = await listed(leaf, day);
    expect(files).toHaveLength(1);
    const { daily_sketch: channel } = guilds.main.channels;
    expect(jumpUrl).toMatch(
      new RegExp(`^https://discord\\.com/channels/${guilds.main.id}/${channel.id}/\\d+$`),
    );

    const asked: string[] = [];
    page.on('request', (request) => asked.push(new URL(request.url()).pathname));
    const client = await openDailySketch(discord, leaf);
    const viewer = await openMissing(client.activity, day);
    // The media proxy was not asked for a file the API said is not there.
    expect(asked).toContain(`/api/guilds/${guilds.main.id}/series/${series.long.id}/days/${day}`);
    expect(asked).not.toContain(files[0]);

    await viewer.getByRole('button', { name: 'Open original post' }).click();
    // That address, handed to the client: an Activity cannot open a link
    // by itself.
    await expect
      .poll(() => client.commands('OPEN_EXTERNAL_LINK'))
      .toEqual([{ cmd: 'OPEN_EXTERNAL_LINK', evt: null, args: { url: jumpUrl } }]);
    await expect(viewer.getByText('Discord didn’t open the post.')).toHaveCount(0);
    // This client is a phone: the post is now open behind leaf, out of
    // sight, so the viewer says so and how to get to it.
    await expect(viewer.getByRole('status')).toHaveText(
      'Opened in the channel, behind leaf. Minimise leaf to see it: tap the arrow at the top left, or press Back on Android.',
    );
  });

  test('on a desktop client, where the channel is in view, it says only that the post opened', async ({
    discord,
    leaf,
  }) => {
    const client = await openDailySketch(discord, leaf, 'desktop');
    const viewer = await openMissing(client.activity, leaf.seed.long_series.missing_media_day);

    await viewer.getByRole('button', { name: 'Open original post' }).click();
    await expect.poll(() => client.commands('OPEN_EXTERNAL_LINK')).toHaveLength(1);
    await expect(viewer.getByRole('status')).toHaveText('Opened in the channel.');
  });

  test('a client that refuses to open links gets the link to copy instead', async ({
    discord,
    leaf,
    guard,
  }) => {
    guard.allow(/^console error: leaf: opening a link through Discord failed/);
    const { guilds, long_series: long } = leaf.seed;
    const client = await openDailySketch(discord, leaf);
    await client.behave({ openLink: 'refused' });
    const viewer = await openMissing(client.activity, long.missing_media_day);

    await viewer.getByRole('button', { name: 'Open original post' }).click();
    await expect(viewer.getByRole('status')).toContainText('Discord didn’t open the post.');
    await expect(viewer.getByRole('status')).toContainText(
      `https://discord.com/channels/${guilds.main.id}/`,
    );

    // Staying on Discord's own "leaving" prompt is a choice: nothing is said.
    await client.behave({ openLink: 'cancelled' });
    await viewer.getByRole('button', { name: 'Next day' }).click();
    await viewer.getByRole('button', { name: 'Open original post' }).click();
    await expect.poll(() => client.commands('OPEN_EXTERNAL_LINK')).toHaveLength(2);
    await expect(viewer.getByText('Discord didn’t open the post.')).toHaveCount(0);
  });
});
