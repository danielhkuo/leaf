// The setup page's storage choice: Cloudflare R2, or a folder on this
// machine. The page is leaf's real one and so is the route it submits to
// (the e2e server's `POST /__e2e/setup` opens setup mode); only what Discord
// and R2 would say is scripted. A folder is checked on this machine's disk
// by the real check, in a temp dir of the server's own.
//
// What is tested here is what the page's script does and no Rust test can
// see: which fields show, what a submit sends, where an error lands and
// where focus goes, what a reload keeps, and the layout at 320px.

import type { APIRequestContext, Locator, Page, Request } from '@playwright/test';

import { expect, test } from '../support/app';
import { axeViolations } from '../support/axe';
import { auditLayout } from '../support/layout';
import { viewport } from '../support/viewports';

/** `POST /__e2e/setup`: what Discord and R2 say to a submit. */
interface Script {
  discord?: 'ok' | 'refuse_token';
  r2?: 'ok' | 'refuse_bucket';
}

/** `GET /__e2e/setup`: the data directory as it stands. */
interface DataDir {
  saved: {
    public_url: string;
    r2: { endpoint: string; bucket: string; access_key_id: string; secret_access_key: string };
  } | null;
  raw: string | null;
  entries: string[];
}

const R2_FIELDS = ['r2_endpoint', 'r2_bucket', 'r2_access_key_id', 'r2_secret_access_key'] as const;

/** The sentences the page and the server share (setup.rs). */
const FOLDER_NOT_ABSOLUTE =
  "Enter the folder's full path, starting with /, such as /data/media. Short forms such as " +
  "~/media, ./media or a path with .. in it don't work.";

/** Opens setup mode on the server and returns its code and data directory. */
async function openSetup(
  request: APIRequestContext,
  script: Script = {},
): Promise<{ code: string; dataDir: string }> {
  const response = await request.post('/__e2e/setup', { data: script });
  expect(response.status(), await response.text()).toBe(200);
  const opened = (await response.json()) as { code: string; data_dir: string };
  return { code: opened.code, dataDir: opened.data_dir };
}

async function dataDir(request: APIRequestContext): Promise<DataDir> {
  const response = await request.get('/__e2e/setup');
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()) as DataDir;
}

/** Opens the page and types the setup code, as an operator does. */
async function unlock(page: Page, code: string): Promise<void> {
  await page.goto('/setup');
  await page.getByRole('textbox', { name: 'Character 1' }).focus();
  await page.keyboard.type(code.replaceAll('-', ''));
  await expect(page.getByText('Code verified')).toBeVisible();
}

/** Fills the Discord and public URL fields with values of the right shape. */
async function fillCommon(page: Page): Promise<void> {
  await page.getByLabel('Bot token').fill('aaa.bbb.ccc');
  await page.getByLabel('Application (client) ID').fill('800000000000000001');
  await page.getByLabel('OAuth client secret').fill('a-client-secret');
  await page.getByLabel('Public URL').fill('https://leaf.example.com');
}

const r2Choice = (page: Page): Locator =>
  page.getByRole('radio', { name: 'Cloudflare R2 (or another S3-compatible store)' });
const folderChoice = (page: Page): Locator =>
  page.getByRole('radio', { name: 'A folder on this machine' });
const folderField = (page: Page): Locator => page.getByLabel('Folder path');
const save = (page: Page): Locator => page.getByRole('button', { name: 'Validate & save' });

/** Whether the fieldset is switched off: its fields are then neither sent nor reachable. */
const switchedOff = (group: Locator): Promise<boolean> =>
  group.evaluate((el) => (el as HTMLFieldSetElement).disabled);

/** Selects the submit button and returns the request the page sends. */
async function submit(page: Page): Promise<Request> {
  const [sent] = await Promise.all([
    page.waitForRequest(
      (request) => request.method() === 'POST' && request.url().endsWith('/setup/api/submit'),
    ),
    save(page).click(),
  ]);
  return sent;
}

test.describe('the storage choice', () => {
  test('starts on R2, and choosing the folder leaves one field and what a folder means', async ({
    page,
    request,
  }) => {
    const { code, dataDir: dir } = await openSetup(request);
    await unlock(page, code);

    await expect(r2Choice(page)).toBeChecked();
    await expect(page.locator('#group-r2')).toBeVisible();
    await expect(page.locator('#group-folder')).toBeHidden();
    expect(await switchedOff(page.locator('#group-folder'))).toBe(true);
    expect(await switchedOff(page.locator('#group-r2'))).toBe(false);

    // By keyboard: the two choices are one radio group, moved through with
    // the arrow keys.
    await r2Choice(page).focus();
    await page.keyboard.press('ArrowDown');
    await expect(folderChoice(page)).toBeChecked();
    await expect(folderChoice(page)).toBeFocused();

    await expect(page.locator('#group-r2')).toBeHidden();
    expect(await switchedOff(page.locator('#group-r2'))).toBe(true);
    expect(await switchedOff(page.locator('#group-folder'))).toBe(false);
    for (const name of R2_FIELDS) await expect(page.locator(`#${name}`)).toBeHidden();
    // One field, filled with the folder leaf suggests inside its data directory.
    await expect(page.locator('#group-folder input')).toHaveCount(1);
    await expect(folderField(page)).toBeVisible();
    await expect(folderField(page)).toHaveValue(`${dir}/media`);
    await expect(page.locator('#folder-suggested')).toHaveText(`${dir}/media`);

    const means = page.getByRole('note', { name: 'What a folder means' });
    await expect(means).toBeVisible();
    await expect(means.getByRole('listitem')).toHaveText([
      /^The files live only on this machine\./,
      /^There is no redundancy: /,
      /^In Docker, the folder must be inside the mounted data volume, for example \/data\/media\./,
    ]);

    // And back: the bucket and keys return, the folder goes.
    await page.keyboard.press('ArrowUp');
    await expect(r2Choice(page)).toBeChecked();
    await expect(page.locator('#group-r2')).toBeVisible();
    await expect(page.locator('#group-folder')).toBeHidden();
    expect(await switchedOff(page.locator('#group-folder'))).toBe(true);
  });

  test('a typed folder path is not replaced by the suggestion', async ({ page, request }) => {
    const { code } = await openSetup(request);
    await unlock(page, code);
    await folderChoice(page).check();
    await folderField(page).fill('/srv/leaf-media');
    await r2Choice(page).check();
    await folderChoice(page).check();
    await expect(folderField(page)).toHaveValue('/srv/leaf-media');
  });
});

test.describe('saving', () => {
  test('a folder is sent without bucket or keys, and saved as the folder alone', async ({
    page,
    request,
  }) => {
    const { code, dataDir: dir } = await openSetup(request);
    await unlock(page, code);
    await fillCommon(page);
    // Something typed for R2 before the folder was chosen stays in the page.
    await page.getByLabel('Bucket').fill('typed-then-left');
    await folderChoice(page).check();

    const sent = await submit(page);
    const body = sent.postDataJSON() as Record<string, unknown>;
    expect(Object.keys(body).sort()).toEqual([
      'client_id',
      'client_secret',
      'discord_token',
      'public_url',
      'setup_code',
      'storage',
      'storage_folder',
    ]);
    expect(body).toMatchObject({ storage: 'folder', storage_folder: `${dir}/media` });

    await expect(page.getByRole('heading', { name: 'leaf is set up' })).toBeFocused();
    const after = await dataDir(request);
    expect(after.saved?.r2).toEqual({
      endpoint: `file://${dir}/media`,
      bucket: '',
      access_key_id: '',
      secret_access_key: '',
    });
    expect(after.raw).not.toContain('bucket');
    expect(after.raw).not.toContain('access_key');
    // The check created the folder and left nothing in it.
    expect(after.entries).toEqual(['leaf.conf', 'media']);
  });

  test('R2 is sent with its four fields and no folder', async ({ page, request }) => {
    const { code } = await openSetup(request);
    await unlock(page, code);
    await fillCommon(page);
    // A folder typed and then left for R2 is not sent either.
    await folderChoice(page).check();
    await r2Choice(page).check();
    await page.getByLabel('S3 endpoint').fill('https://acc.r2.cloudflarestorage.com');
    await page.getByLabel('Bucket').fill('leaf-media');
    await page.getByLabel('Access key ID').fill('an-access-key-id');
    await page.getByLabel('Secret access key', { exact: true }).fill('a-secret-access-key');

    const sent = await submit(page);
    const body = sent.postDataJSON() as Record<string, unknown>;
    expect(Object.keys(body).sort()).toEqual(
      [
        'client_id',
        'client_secret',
        'discord_token',
        'public_url',
        ...R2_FIELDS,
        'setup_code',
        'storage',
      ].sort(),
    );
    expect(body).toMatchObject({ storage: 'r2', r2_bucket: 'leaf-media' });

    await expect(page.getByRole('heading', { name: 'leaf is set up' })).toBeVisible();
    const after = await dataDir(request);
    expect(after.saved?.r2).toEqual({
      endpoint: 'https://acc.r2.cloudflarestorage.com',
      bucket: 'leaf-media',
      access_key_id: 'an-access-key-id',
      secret_access_key: 'a-secret-access-key',
    });
    expect(after.entries).toEqual(['leaf.conf']);
  });
});

test.describe('errors', () => {
  test('a path that is not a full one is caught on the folder field before anything is sent', async ({
    page,
    request,
  }) => {
    const { code } = await openSetup(request);
    await unlock(page, code);
    await fillCommon(page);
    await folderChoice(page).check();
    await folderField(page).fill('media');

    const posts: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'POST') posts.push(r.url());
    });
    await save(page).click();

    await expect(page.locator('#err-storage_folder')).toHaveText(FOLDER_NOT_ABSOLUTE);
    await expect(folderField(page)).toHaveAttribute('aria-invalid', 'true');
    await expect(folderField(page)).toBeFocused();
    // The error is the field's own description, so a screen reader reads it there.
    await expect(folderField(page)).toHaveAccessibleDescription(
      new RegExp(`^${FOLDER_NOT_ABSOLUTE.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`),
    );
    // No bucket or key is asked for, and nothing left the page.
    for (const name of R2_FIELDS) await expect(page.locator(`#err-${name}`)).toBeEmpty();
    await expect(page.locator('#err-form')).toBeEmpty();
    expect(posts).toEqual([]);

    // An empty one says what to enter.
    await folderField(page).fill('');
    await save(page).click();
    await expect(page.locator('#err-storage_folder')).toHaveText(
      "Enter the folder's full path, such as /data/media.",
    );
    expect(posts).toEqual([]);
  });

  test('what the server refuses about the folder lands on the folder field', async ({
    page,
    request,
    guard,
  }) => {
    guard.allow(
      /^HTTP 422: POST .*\/setup\/api\/submit$/,
      /^console error: Failed to load resource: .*422/,
    );
    const { code, dataDir: dir } = await openSetup(request);
    await unlock(page, code);
    await fillCommon(page);
    await folderChoice(page).check();
    // A full path as far as the page can tell; the server refuses the `..`.
    await folderField(page).fill(`${dir}/../media`);

    const sent = await submit(page);
    expect((await sent.response())?.status()).toBe(422);
    await expect(page.locator('#err-storage_folder')).toHaveText(FOLDER_NOT_ABSOLUTE);
    await expect(folderField(page)).toHaveAttribute('aria-invalid', 'true');
    await expect(folderField(page)).toBeFocused();
    await expect(page.locator('#err-form')).toBeEmpty();
    await expect(save(page)).toBeEnabled();

    // Choosing R2 takes the folder's error away with the folder.
    await r2Choice(page).check();
    await folderChoice(page).check();
    await expect(page.locator('#err-storage_folder')).toBeEmpty();
    await expect(folderField(page)).not.toHaveAttribute('aria-invalid');
    expect((await dataDir(request)).saved).toBeNull();
  });

  test('a refused token is shown on the token, and the folder that was tried is not left behind', async ({
    page,
    request,
    guard,
  }) => {
    guard.allow(
      /^HTTP 422: POST .*\/setup\/api\/submit$/,
      /^console error: Failed to load resource: .*422/,
    );
    const { code } = await openSetup(request, { discord: 'refuse_token' });
    await unlock(page, code);
    await fillCommon(page);
    await folderChoice(page).check();

    await submit(page);
    await expect(page.locator('#err-discord_token')).toHaveText(
      'The stand-in for Discord refuses this bot token.',
    );
    await expect(page.getByLabel('Bot token')).toBeFocused();
    await expect(page.locator('#err-storage_folder')).toBeEmpty();
    await expect(folderField(page)).not.toHaveAttribute('aria-invalid');
    expect(await dataDir(request)).toEqual({ saved: null, raw: null, entries: [] });
  });

  test('what R2 refuses lands on its own field', async ({ page, request, guard }) => {
    guard.allow(
      /^HTTP 422: POST .*\/setup\/api\/submit$/,
      /^console error: Failed to load resource: .*422/,
    );
    const { code } = await openSetup(request, { r2: 'refuse_bucket' });
    await unlock(page, code);
    await fillCommon(page);
    await page.getByLabel('S3 endpoint').fill('https://acc.r2.cloudflarestorage.com');
    await page.getByLabel('Bucket').fill('leaf-media');
    await page.getByLabel('Access key ID').fill('an-access-key-id');
    await page.getByLabel('Secret access key', { exact: true }).fill('a-secret-access-key');

    await submit(page);
    await expect(page.locator('#err-r2_bucket')).toHaveText(
      'The stand-in for R2 has no such bucket.',
    );
    await expect(page.getByLabel('Bucket')).toBeFocused();
    await expect(page.getByLabel('Bucket')).toHaveAttribute('aria-invalid', 'true');
  });
});

test('a reload keeps the folder choice and the path typed', async ({ page, request }) => {
  const { code } = await openSetup(request);
  await unlock(page, code);
  await folderChoice(page).check();
  await folderField(page).fill('/srv/leaf-media');

  await page.reload();
  await expect(page.getByText('Code verified')).toBeVisible();
  await expect(folderChoice(page)).toBeChecked();
  await expect(folderField(page)).toHaveValue('/srv/leaf-media');
  await expect(page.locator('#group-r2')).toBeHidden();
  expect(await switchedOff(page.locator('#group-r2'))).toBe(true);
});

/**
 * What axe cannot judge on this page, storage or no storage: text straight
 * on the page's gradient background (it can name no one colour behind it)
 * and the tick of "Code verified" (a symbol, not text). Nothing in the
 * storage card is among them, so anything else unjudged is a failure.
 */
const UNJUDGED_ON_THIS_PAGE = ['h1', '.sub', '.check', 'footer', 'footer > code'];

/** The elements axe listed under `finding`, without what it measured. */
function elementsOf(finding: string): string[] {
  return finding
    .split('\n')
    .slice(1)
    .map((line) => line.trim().replace(/ \(.*$/, ''));
}

test.describe('at 320px', () => {
  test.use(viewport(320).use);

  for (const choice of ['r2', 'folder'] as const) {
    test(`nothing is wider than the screen with ${choice} chosen, and axe finds nothing`, async ({
      page,
      request,
    }) => {
      const { code } = await openSetup(request);
      await unlock(page, code);
      if (choice === 'folder') await folderChoice(page).check();
      await expect(choice === 'folder' ? folderField(page) : r2Choice(page)).toBeVisible();
      // The form fades in once the code verifies; colours are measured after.
      await page.evaluate(() => Promise.all(document.getAnimations().map((a) => a.finished)));

      const layout = await auditLayout(page);
      expect(layout.pageOverflow).toBe(0);
      expect(layout.outside).toEqual([]);
      expect(layout.smallFonts).toEqual([]);
      expect(layout.smallTargets).toEqual([]);
      expect(layout.fields).toBeGreaterThan(0);

      const findings = await axeViolations(page);
      const unjudged = findings.filter((f) => f.startsWith('color-contrast, not judged:'));
      expect(findings.filter((f) => !unjudged.includes(f))).toEqual([]);
      expect(unjudged.flatMap(elementsOf).sort()).toEqual([...UNJUDGED_ON_THIS_PAGE].sort());
    });
  }
});
