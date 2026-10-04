// The guard every app test runs under, tried on the faults it exists to
// catch. An app test fails for a console warning or error and for a response
// the server refused, unless the test said beforehand that it expects one.
// Each test here produces such a fault on purpose, checks that the guard has
// it, and then allows it so that the test itself can pass. (The guard's own
// mechanics are tested against the mock screens, in e2e/mock/checks.spec.ts;
// a request that leaves this machine is isolation.spec.ts's, and a field
// that goes missing without a word is contract.spec.ts's.)

import type { Page, Route } from '@playwright/test';

import { expect, test } from '../support/app';

type Row = Record<string, unknown>;

/** Lets the real series list through with `change` applied to every row. */
async function driftSeriesList(page: Page, change: (row: Row) => void): Promise<void> {
  await page.route(/\/api\/guilds\/\d+\/series$/, async (route: Route) => {
    const response = await route.fetch();
    const rows = (await response.json()) as Row[];
    rows.forEach(change);
    await route.fulfill({ response, json: rows });
  });
}

test('an optional field that changed shape still loads, and is reported', async ({
  discord,
  guard,
  page,
}) => {
  // As if the server had started sending the day count as text.
  await driftSeriesList(page, (row) => (row.total_days = String(row.total_days)));
  const { activity: app } = await discord.launch('viewer');
  // The client drops the field and carries on: nothing on screen gives it away.
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();

  const reported = guard.unexpected();
  expect(reported).toHaveLength(2);
  for (const line of reported) {
    expect(line).toMatch(
      /^console warning: leaf: ignored a field the server sent in an unexpected shape/,
    );
  }
  guard.allow(/^console warning: leaf: ignored a field the server sent in an unexpected shape/);
});

test('a required field that changed shape stops the gallery, and is reported', async ({
  discord,
  guard,
  page,
}) => {
  await driftSeriesList(page, (row) => (row.id = String(row.id)));
  const { activity: app } = await discord.launch('viewer');
  await expect(
    app.getByRole('heading', { level: 1, name: 'Couldn’t load the gallery' }),
  ).toBeVisible();
  await expect(app.getByText('leaf sent something this version can’t read.')).toBeVisible();

  expect(guard.unexpected()).toEqual([
    expect.stringMatching(
      /^console error: leaf: loading the gallery failed .*unexpected response shape/,
    ),
  ]);
  guard.allow(/^console error: leaf: loading the gallery failed/);
});

test('a response the server refused is reported, with the browser’s line about it', async ({
  discord,
  leaf,
  guard,
}) => {
  // Signed in, but leaf cannot ask Discord whether this person is a member.
  await leaf.discordApi({ mode: 'down', only: ['guild_member'] });
  const { activity: app } = await discord.launch('viewer');
  await expect(
    app.getByRole('heading', { level: 1, name: 'Couldn’t load the gallery' }),
  ).toBeVisible();
  await expect(
    app.getByText('leaf can’t reach Discord right now. Try again in a moment.'),
  ).toBeVisible();

  const reported = guard.unexpected();
  expect(reported).toContainEqual(
    expect.stringMatching(/^HTTP 503: GET http:\/\/127\.0\.0\.1:\d+\/api\/guilds\/\d+\/series$/),
  );
  expect(reported).toContainEqual(
    expect.stringMatching(
      /^console error: Failed to load resource: the server responded with a status of 503/,
    ),
  );
  expect(reported).toContainEqual(
    expect.stringMatching(/^console error: leaf: loading the gallery failed/),
  );
  guard.allow(
    /^HTTP 503: GET /,
    /^console error: (Failed to load resource|leaf: loading the gallery failed)/,
  );

  // Discord is back: Try again loads the gallery without a new sign-in.
  await leaf.discordApi({ mode: 'ok' });
  await app.getByRole('button', { name: 'Try again' }).click();
  await expect(app.getByRole('button', { name: /^Daily Sketch/ })).toBeVisible();
});
