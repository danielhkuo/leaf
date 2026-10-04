// The creator's side of the gallery against the real policy and the real
// database: starting a series, being told why one cannot be started yet,
// changing a series' settings, and hearing that a reminder did not arrive.
// "Saved" here means stored: each change is read back after the Activity is
// closed and opened again, and from the API.

import type { FrameLocator } from '@playwright/test';

import { expect, expectRefusal, test, type LeafServer } from '../support/app';
import { seriesNames } from '../support/screens';

/** What the creator's own list says about the series of that name, from the API. */
async function mine(leaf: LeafServer, name: string): Promise<Record<string, unknown> | undefined> {
  const list = (await (await leaf.get('creator', '/series/mine')).json()) as { name: string }[];
  return list.find((series) => series.name === name);
}

test('an eligible member starts a series, and it is there when leaf is opened again', async ({
  discord,
  leaf,
  page,
}) => {
  const { guilds } = leaf.seed;
  const photoShare = guilds.main.channels.photo_share.id;
  const writes: { method: string; path: string; body: unknown }[] = [];
  page.on('request', (request) => {
    if (request.method() === 'GET' || !request.url().includes('/api/guilds/')) return;
    writes.push({
      method: request.method(),
      path: new URL(request.url()).pathname,
      body: request.postDataJSON() as unknown,
    });
  });

  const client = await discord.launch('creator');
  let app = client.activity;
  await app.getByRole('button', { name: 'Start a series' }).click();
  await expect(app.getByRole('heading', { level: 1, name: 'Start a series' })).toBeVisible();

  await app.getByLabel('Series name').fill('Tide Pools');
  await app.getByLabel('Description').fill('Low tide, every Sunday.');
  // "More options" starts open when the channel is only a guess, as it is
  // for a launch from a channel no series may use.
  const more = app.locator('details').filter({ hasText: 'More options' });
  if (!(await more.evaluate((details: HTMLDetailsElement) => details.open))) {
    await more.locator('summary').click();
  }
  // The channels on offer are the ones an admin allowed for series.
  await expect(app.getByLabel('Channel').locator('option')).toHaveText([
    '#daily-sketch',
    '#photo-share',
  ]);
  await app.getByLabel('Channel').selectOption({ label: '#photo-share' });
  await app.getByLabel('How often you’ll post').selectOption({ label: 'Weekly' });
  await app.getByLabel('First day number').fill('40');
  // The threshold is the guild's, from the server's options.
  await expect(app.getByText('only you can see this one until 3 days are archived')).toBeVisible();
  await app.getByRole('button', { name: 'Start a series' }).click();

  // Straight to the new series, which says what to do next, for this phone
  // and this channel.
  await expect(app.getByRole('heading', { level: 1, name: 'Tide Pools' })).toBeVisible();
  await expect(app.getByText('Series created')).toBeVisible();
  await expect(
    app.getByRole('heading', { level: 2, name: 'Archive your first post' }),
  ).toBeVisible();
  await expect(app.getByText('Minimise leaf, then post your photo or video in')).toBeVisible();
  await expect(app.getByText('#photo-share')).toBeVisible();
  await expect(app.getByText('🌱 Only you can see this series for now')).toBeVisible();
  await expect(app.getByText('once 3 days are archived (0 so far)')).toBeVisible();

  // What was sent is what the form held, under the names the server reads.
  expect(writes).toEqual([
    {
      method: 'POST',
      path: `/api/guilds/${guilds.main.id}/series`,
      body: expect.objectContaining({
        name: 'Tide Pools',
        description: 'Low tide, every Sunday.',
        channel_id: photoShare,
        cadence: 'weekly',
        privacy: 'public',
        start_day: 40,
      }),
    },
  ]);

  // Stored: the creator's own list has it, as a sprout with nothing archived.
  expect(await mine(leaf, 'Tide Pools')).toMatchObject({
    id: expect.any(Number),
    name: 'Tide Pools',
    state: 'sprout',
    cadence: 'weekly',
    channel_id: photoShare,
    channel_name: 'photo-share',
    archived_days: 0,
  });
  // And hidden from everyone else until it has its three days.
  const others = (await (await leaf.get('viewer', '/series')).json()) as { name: string }[];
  expect(others.map((series) => series.name)).toEqual(['Daily Sketch', 'Evening Walks']);

  // leaf closed and opened again: it opens on the series last used in this
  // server, and the new one is in every list.
  app = (await discord.launch('creator')).activity;
  await expect(app.getByRole('heading', { level: 1, name: 'Tide Pools' })).toBeVisible();
  await expect(app.getByText('Series created')).toHaveCount(0);
  await app.getByRole('button', { name: 'Back to series list' }).click();
  await expect(seriesNames(app)).toHaveText([
    'Daily Sketch',
    'Morning Coffee',
    'Patron Studies',
    'Private Notes',
    'Old Polaroids',
    'Evening Walks',
    'Tide Pools',
  ]);
  await app.getByRole('button', { name: 'Manage my series' }).click();
  await app.getByRole('button', { name: /^Tide Pools/ }).click();
  await expect(app.getByLabel('Series name')).toHaveValue('Tide Pools');
  await expect(app.getByLabel('Description')).toHaveValue('Low tide, every Sunday.');
  await expect(app.getByLabel('How often you post')).toHaveValue('weekly');
  await expect(app.getByLabel('Channel')).toHaveValue(photoShare);
  await expect(app.getByLabel('First day number')).toHaveValue('40');
});

test('a name another series already has is refused under the name field', async ({
  discord,
  guard,
}) => {
  expectRefusal(guard, 409, 'POST', /\/api\/guilds\/\d+\/series/);
  guard.allow(/^console error: leaf: creating the series failed/);
  const { activity: app } = await discord.launch('creator');
  await app.getByRole('button', { name: 'Start a series' }).click();
  const name = app.getByLabel('Series name');
  await name.fill('Daily Sketch');
  await app.getByRole('button', { name: 'Start a series' }).click();

  await expect(name).toHaveAttribute('aria-invalid', 'true');
  await expect(name).toHaveAccessibleDescription(/^That name is already used in this server/);
  await expect(name).toBeFocused();
  // Still the form, with what was typed.
  await expect(name).toHaveValue('Daily Sketch');
});

test('a member who may not start a series is told what is in the way, with the specifics', async ({
  discord,
  leaf,
  request,
}) => {
  const { activity: app } = await discord.launch('newcomer');
  await expect(app.getByRole('heading', { level: 1, name: 'Series', exact: true })).toBeVisible();
  // Nothing invites them to start one; the reasons wait behind a question.
  await expect(app.getByRole('button', { name: 'Start a series' })).toHaveCount(0);
  await app.getByText('Want your own series?').click();
  // The role is the one the guild requires, and the date is seven days
  // after the day the server says they joined.
  const reasons = app.locator('details').filter({ hasText: 'Want your own series?' });
  await expect(reasons.getByRole('listitem')).toHaveText([
    /needs the @Artists role/,
    /You can start one on Jan 8, 2099/,
  ]);

  // The server, asked directly, refuses the same person for the same reasons.
  const headers = await leaf.signIn('newcomer');
  const refused = await request.post(`/api/guilds/${leaf.seed.guilds.main.id}/series`, {
    headers,
    data: {
      name: 'Not allowed',
      channel_id: leaf.seed.guilds.main.channels.daily_sketch.id,
      cadence: 'daily',
      privacy: 'public',
    },
  });
  expect(refused.status()).toBe(403);
  expect(((await refused.json()) as { error: string }).error).toBe('missing_creator_role');
});

test.describe('series settings', () => {
  test('a changed name, first day number and reminder are stored', async ({
    discord,
    leaf,
    page,
    request,
  }) => {
    const { guilds } = leaf.seed;
    // A series with nothing archived yet, so its first day number is free
    // to move: it can never be past the earliest archived day.
    const headers = await leaf.signIn('creator');
    const made = await request.post(`/api/guilds/${guilds.main.id}/series`, {
      headers,
      data: {
        name: 'Field Notes',
        channel_id: guilds.main.channels.daily_sketch.id,
        cadence: 'daily',
        privacy: 'public',
      },
    });
    expect(made.status()).toBe(201);
    const { id } = (await made.json()) as { id: number };

    const patches: unknown[] = [];
    page.on('request', (sent) => {
      if (sent.method() === 'PATCH') patches.push(sent.postDataJSON());
    });
    const openSettings = async (): Promise<FrameLocator> => {
      const { activity: app } = await discord.launch('creator');
      await app.getByRole('button', { name: 'Manage my series' }).click();
      await app.getByRole('button', { name: /^(Field Notes|Margin Notes)/ }).click();
      await expect(app.getByRole('heading', { level: 1, name: 'Series settings' })).toBeVisible();
      await expect(app.getByLabel('Series name')).toBeVisible();
      return app;
    };

    let app = await openSettings();
    const save = app.getByRole('button', { name: 'Save changes' });
    await expect(save).toBeDisabled();
    await app.getByLabel('Series name').fill('Margin Notes');
    await app.getByLabel('First day number').fill('200');
    await app.getByLabel('Remind me when I’m behind').check();
    // Switching reminders on fills in an evening time and the device's zone.
    await expect(app.getByLabel('Time', { exact: true })).toHaveValue('20:00');
    await expect(app.getByLabel('Timezone')).toHaveValue('Europe/Berlin');
    await app.getByLabel('Time', { exact: true }).fill('07:45');
    await app.getByLabel('Timezone').selectOption('Asia/Tokyo');
    await app.getByLabel('A ping in #daily-sketch').check();
    await expect(app.getByText('Unsaved changes', { exact: true })).toBeVisible();
    await save.click();
    await expect(app.getByText('Saved', { exact: true })).toBeVisible();
    await expect(save).toBeDisabled();

    // Only what changed was sent, under the server's names.
    expect(patches).toEqual([
      {
        name: 'Margin Notes',
        start_day: 200,
        reminder_enabled: true,
        reminder_time: '07:45',
        reminder_timezone: 'Asia/Tokyo',
        reminder_dm: false,
      },
    ]);
    const stored = await (await leaf.get('creator', `/series/${id}/settings`)).json();
    expect(stored).toMatchObject({
      name: 'Margin Notes',
      start_day: 200,
      reminder_enabled: true,
      reminder_time: '07:45',
      reminder_timezone: 'Asia/Tokyo',
      reminder_dm: false,
    });

    // leaf closed and opened again: the form shows what was stored.
    app = await openSettings();
    await expect(app.getByLabel('Series name')).toHaveValue('Margin Notes');
    await expect(app.getByLabel('First day number')).toHaveValue('200');
    await expect(app.getByLabel('Remind me when I’m behind')).toBeChecked();
    await expect(app.getByLabel('Time', { exact: true })).toHaveValue('07:45');
    await expect(app.getByLabel('Timezone')).toHaveValue('Asia/Tokyo');
    await expect(app.getByLabel('A ping in #daily-sketch')).toBeChecked();
    await expect(app.getByRole('button', { name: 'Save changes' })).toBeDisabled();
  });

  test('a first day number past the earliest archived day is refused under its field', async ({
    discord,
    leaf,
    guard,
  }) => {
    expectRefusal(guard, 400, 'PATCH', /\/api\/guilds\/\d+\/series\/\d+/);
    guard.allow(/^console error: leaf: saving the series settings failed/);
    const { activity: app } = await discord.launch('creator');
    await app.getByRole('button', { name: /^Evening Walks/ }).click();
    await app.getByRole('button', { name: 'Series settings' }).click();
    const firstDay = app.getByLabel('First day number');
    await expect(firstDay).toHaveValue('1');

    // Evening Walks has Day 1 archived, so the count cannot start at 3.
    await firstDay.fill('3');
    await app.getByRole('button', { name: 'Save changes' }).click();
    await expect(firstDay).toHaveAttribute('aria-invalid', 'true');
    await expect(firstDay).toHaveAccessibleDescription(
      /no higher than the earliest day already archived/,
    );
    await expect(app.getByText('Saved', { exact: true })).toHaveCount(0);
    const stored = await (
      await leaf.get('creator', `/series/${leaf.seed.series.reminder.id}/settings`)
    ).json();
    expect(stored).toMatchObject({ name: 'Evening Walks', start_day: 1 });
  });

  test('a reminder Discord would not deliver is reported on the list and in the settings', async ({
    discord,
  }) => {
    const { activity: app } = await discord.launch('creator');
    await app.getByRole('button', { name: 'Manage my series' }).click();
    const card = app.getByRole('button', { name: /^Evening Walks/ });
    await expect(card).toContainText('reminders can’t reach you');
    // No other series has a reminder that failed.
    await expect(app.getByText('reminders can’t reach you')).toHaveCount(1);

    await card.click();
    await expect(app.getByText('leaf couldn’t deliver your last reminder')).toBeVisible();
    // The reason is the server's (`dm_closed`), and so is the day it was tried.
    const callout = app.getByText('Discord wouldn’t let leaf send you a DM.');
    await expect(callout).toBeVisible();
    await expect(callout).toContainText('It was tried on May 18.');
    await expect(app.getByLabel('Remind me when I’m behind')).toBeChecked();
    await expect(app.getByLabel('Time', { exact: true })).toHaveValue('18:30');
    await expect(app.getByLabel('A direct message')).toBeChecked();
  });
});
