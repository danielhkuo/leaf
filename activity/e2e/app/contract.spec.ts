// The client's schemas and the server's answers, key by key. Every response
// the gallery and the admin panel parse is asked of the real API here, with
// no browser, and held to the schema the client reads it with: each key the
// schema names has to be in what the server sent, and each key the server
// sent has to be one the schema names.
//
// This is what the console guard cannot see. A field read through `opt()`
// may be absent, so a server that renames `total_days` still gets a quiet
// console and a gallery that loads, with one detail gone from the screen.
//
// A response the client starts to parse needs its sample here: a schema
// exported from either module that no answer was read with fails the test.

import type { APIRequestContext } from '@playwright/test';
import { z } from 'zod';

import * as admin from '../../src/lib/admin/schemas';
import * as gallery from '../../src/lib/api/schemas';
import { expect, test, type LeafServer, type PersonaKey } from '../support/app';
import { Contract } from '../support/contract';

/**
 * Keys the gallery API sends only when it has something to say (serde's
 * `skip_serializing_if`, in leaf-server's api/mod.rs). A violation carries
 * the numbers or the name of its own rule and no others, and
 * `guild_not_setup` has none to give. Any other key that comes and goes
 * between answers of one shape is drift.
 */
const VIOLATION_SPECIFICS = [
  'eligibility.violations[].params',
  'eligibility.violations[].params.current',
  'eligibility.violations[].params.days',
  'eligibility.violations[].params.eligible_at',
  'eligibility.violations[].params.limit',
  'eligibility.violations[].params.role_name',
];

/** The one such key of the admin API: only the answer to a save counts the sprouts it published. */
const SAVE_ONLY = ['guild.settings.sprouts_published'];

/**
 * Exported schemas no response is read with. `namedIdSchema` is only what
 * `roleOptionSchema` is built from.
 */
const NOT_A_RESPONSE = ['namedIdSchema'];

/** The exported schemas of a module that no answer was read with. */
function unused(schemas: Record<string, unknown>, contract: Contract): string[] {
  return Object.entries(schemas)
    .filter(([, schema]) => schema instanceof z.ZodType && !contract.used(schema))
    .map(([name]) => name);
}

type Send = (method: 'GET' | 'POST' | 'PATCH', path: string, data?: unknown) => Promise<unknown>;

/** Calls under `base` with `headers`, each of which has to succeed, answering the JSON. */
function caller(request: APIRequestContext, base: string, headers: Record<string, string>): Send {
  return async (method, path, data) => {
    const answer = await request.fetch(`${base}${path}`, { method, headers, data });
    expect(answer.ok(), `${method} ${base}${path} answered ${answer.status()}`).toBe(true);
    return (await answer.json()) as unknown;
  };
}

/** The gallery API as `persona`. */
async function gallerySession(
  leaf: LeafServer,
  request: APIRequestContext,
  persona: PersonaKey,
): Promise<Send> {
  return caller(request, '/api', await leaf.signIn(persona));
}

/** The admin API as the seeded admin. */
async function adminSession(leaf: LeafServer, request: APIRequestContext): Promise<Send> {
  const token = await leaf.mintAdminToken('admin');
  return caller(request, '/api/admin', { authorization: `Bearer ${token}` });
}

test('the gallery API sends every field the client reads, and the client reads every field it sends', async ({
  leaf,
  request,
}) => {
  const { seed } = leaf;
  const { main, unset } = seed.guilds;
  const { long } = seed.series;
  const guild = `/guilds/${main.id}`;
  const contract = new Contract();

  // Signing in, and renewing the session it gives.
  const anonymous = caller(request, '/api', {});
  const session = await anonymous('POST', '/token', { code: seed.personas.viewer.code });
  contract.check('token', gallery.exchangeSchema, session);
  const { token } = session as { token: string };
  const holder = caller(request, '/api', { authorization: `Bearer ${token}` });
  contract.check('refresh', gallery.refreshSchema, await holder('POST', '/token/refresh'));

  const viewer = await gallerySession(leaf, request, 'viewer');
  const creator = await gallerySession(leaf, request, 'creator');
  const newcomer = await gallerySession(leaf, request, 'newcomer');

  // The creator's list has every kind of series, a sprout and a revoked one included.
  contract.check('series', gallery.seriesListSchema, await creator('GET', `${guild}/series`));
  contract.check('series', gallery.seriesListSchema, await viewer('GET', `${guild}/series`));
  const days = await viewer('GET', `${guild}/series/${long.id}/days`);
  contract.check('days', gallery.daySummaryListSchema, days);
  // A day of photos, a video, and a day whose file was never saved.
  const { three_images_day, mp4_day, missing_media_day } = seed.long_series;
  for (const day of [three_images_day, mp4_day, missing_media_day]) {
    const answer = await viewer('GET', `${guild}/series/${long.id}/days/${day}`);
    contract.check('day', gallery.daySchema, answer);
  }
  const stats = await viewer('GET', `${guild}/series/${long.id}/stats`);
  contract.check('stats', gallery.statsSchema, stats);

  // Nothing pressed, a press on a series, and a press on one of its days.
  contract.check(
    'launch intent',
    gallery.launchIntentSchema,
    await viewer('GET', `${guild}/launch-intent`),
  );
  for (const day of [undefined, 3]) {
    await leaf.pressOpenGallery('viewer', 'long', day);
    const pressed = await viewer('GET', `${guild}/launch-intent`);
    expect(pressed).not.toBeNull();
    contract.check('launch intent', gallery.launchIntentSchema, pressed);
  }

  // The creator's side: what they own, the form's choices, a series'
  // settings (one of them with a reminder that failed), and the answers to
  // a create and to a save.
  contract.check('mine', gallery.mySeriesListSchema, await creator('GET', `${guild}/series/mine`));
  const options = await creator('GET', `${guild}/series/options`);
  contract.check('options', gallery.seriesOptionsSchema, options);
  for (const series of [long, seed.series.reminder]) {
    const settings = await creator('GET', `${guild}/series/${series.id}/settings`);
    contract.check('settings', gallery.seriesSettingsSchema, settings);
  }
  const created = await creator('POST', `${guild}/series`, {
    name: 'Contract Check',
    channel_id: main.channels.daily_sketch.id,
    cadence: 'daily',
    privacy: 'public',
  });
  contract.check('created series', gallery.createdSeriesSchema, created);
  const { id } = created as { id: number };
  const saved = await creator('PATCH', `${guild}/series/${id}`, { description: 'Read back.' });
  contract.check('settings', gallery.seriesSettingsSchema, saved);

  // May start a series; may not, for each reason that comes with a name or
  // with numbers (the role, the join date, and, once an admin lowers it, the
  // limit); and in a server where /setup never ran.
  const eligibility = `${guild}/series/eligibility`;
  contract.check('eligibility', gallery.eligibilitySchema, await creator('GET', eligibility));
  contract.check('eligibility', gallery.eligibilitySchema, await newcomer('GET', eligibility));
  const asAdmin = await adminSession(leaf, request);
  await asAdmin('PATCH', `/guilds/${main.id}/settings`, { max_series_per_user: 1 });
  contract.check('eligibility', gallery.eligibilitySchema, await creator('GET', eligibility));
  const notSetUp = await creator('GET', `/guilds/${unset.id}/series/eligibility`);
  contract.check('eligibility', gallery.eligibilitySchema, notSetUp);

  expect(contract.unreadable(), 'answers the client could not read whole').toEqual([]);
  // `mine[].channel_missing` among them: the client reads a name that is not
  // there as a channel that is gone unless the flag says otherwise
  // (src/lib/utils/channel.ts), so this server has to send it every time.
  expect(contract.neverSent(), 'fields the client reads that no answer had').toEqual([]);
  expect(contract.sometimesSent(), 'fields only some answers had').toEqual(VIOLATION_SPECIFICS);
  expect(contract.unread(), 'fields the server sends that the client drops').toEqual([]);
  expect(unused(gallery, contract), 'schemas with no answer to read').toEqual(NOT_A_RESPONSE);
});

test('the admin API sends every field the panel reads, and the panel reads every field it sends', async ({
  leaf,
  request,
}) => {
  const { main, unset } = leaf.seed.guilds;
  const asAdmin = await adminSession(leaf, request);
  const contract = new Contract();

  contract.check('guilds', admin.adminGuildListSchema, await asAdmin('GET', '/guilds'));
  // A server with series in every state, and one where /setup never ran.
  for (const guild of [main, unset]) {
    const detail = await asAdmin('GET', `/guilds/${guild.id}`);
    contract.check('guild', admin.adminGuildDetailSchema, detail);
  }
  const options = await asAdmin('GET', `/guilds/${main.id}/options`);
  contract.check('options', admin.adminOptionsSchema, options);

  // The answers to a save are a guild's settings and one of its series
  // again, so they are counted with those. The panel reads the settings
  // answer with a union (it also takes an older, wrapped form); its keys
  // are the plain settings'.
  const settings = await asAdmin('PATCH', `/guilds/${main.id}/settings`, { sprout_threshold: 2 });
  contract.check(
    'guild.settings',
    admin.adminSettingsAnswerSchema,
    settings,
    admin.adminSettingsSchema,
  );
  const { reminder } = leaf.seed.series;
  const series = await asAdmin('PATCH', `/guilds/${main.id}/series/${reminder.id}`, {
    privacy: 'creator_only',
  });
  contract.check('guild.series[]', admin.adminSeriesSchema, series);

  expect(contract.unreadable(), 'answers the panel could not read whole').toEqual([]);
  expect(contract.neverSent(), 'fields the panel reads that no answer had').toEqual([]);
  expect(contract.sometimesSent(), 'fields only some answers had').toEqual(SAVE_ONLY);
  expect(contract.unread(), 'fields the server sends that the panel drops').toEqual([]);
  expect(unused(admin, contract), 'schemas with no answer to read').toEqual([]);
});

test.describe('the comparison itself, on an answer that drifted', () => {
  type Row = Record<string, unknown>;

  /** The creator's series list as the server sends it: six rows, a sprout among them. */
  async function seriesList(leaf: LeafServer): Promise<Row[]> {
    return (await (await leaf.get('creator', '/series')).json()) as Row[];
  }

  interface Findings {
    unreadable: string[];
    neverSent: string[];
    sometimesSent: string[];
    unread: string[];
  }

  /** Everything a comparison of `rows` with the series list's schema finds. */
  function findings(rows: Row[]): Findings {
    const contract = new Contract();
    contract.check('series', gallery.seriesListSchema, rows);
    return {
      unreadable: contract.unreadable(),
      neverSent: contract.neverSent(),
      sometimesSent: contract.sometimesSent(),
      unread: contract.unread(),
    };
  }

  const NOTHING: Findings = { unreadable: [], neverSent: [], sometimesSent: [], unread: [] };

  test('a renamed optional field parses in silence, and is found from both sides', async ({
    leaf,
  }) => {
    const sent = await seriesList(leaf);
    expect(findings(sent)).toEqual(NOTHING);

    const renamed = sent.map(({ total_days, ...row }) => ({ ...row, days_total: total_days }));
    // What makes this check necessary: the client reads the answer without complaint.
    expect(gallery.seriesListSchema.safeParse(renamed).success).toBe(true);
    expect(findings(renamed)).toEqual({
      ...NOTHING,
      neverSent: ['series[].total_days'],
      unread: ['series[].days_total'],
    });
  });

  test('a field missing from some rows only is told apart from one never sent', async ({
    leaf,
  }) => {
    const [first = {}, ...rest] = await seriesList(leaf);
    const partly = { ...first };
    delete partly.last_posted_at;
    expect(findings([partly, ...rest])).toEqual({
      ...NOTHING,
      sometimesSent: ['series[].last_posted_at'],
    });
  });

  test('a field in another shape is unreadable, whether the client drops it or stops', async ({
    leaf,
  }) => {
    const sent = await seriesList(leaf);
    const dropped = findings(sent.map((row) => ({ ...row, total_days: String(row.total_days) })));
    expect(dropped.unreadable).toHaveLength(sent.length);
    expect(dropped.unreadable[0]).toMatch(
      /^series: leaf: ignored a field the server sent in an unexpected shape/,
    );

    const stopped = findings(sent.map((row) => ({ ...row, id: String(row.id) })));
    expect(stopped.unreadable).toEqual([
      expect.stringMatching(/^series: 0\.id: Expected number, received string/),
    ]);
  });

  test('a shape no answer held is reported as never sent', () => {
    // An empty list reads fine and proves nothing about its rows.
    expect(findings([]).neverSent).toContain('series[].id');
    expect(findings([]).unreadable).toEqual([]);
  });
});
