// The gallery API as leaf-server serializes it, parsed by the schemas the
// client uses. The samples are written from the server's response structs
// (crates/leaf-server/src/api/dto.rs and mod.rs): every field, with `null`
// wherever the Rust type is an `Option` that is serialized when empty. The
// mock API (src/mock) is built from the client's own types, so it cannot
// show the two sides drifting apart; this can, for the client's half. A
// change to a server struct needs the matching sample changed here.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  daySchema,
  daySummaryListSchema,
  eligibilitySchema,
  launchIntentSchema,
  mySeriesListSchema,
  refreshSchema,
  seriesListSchema,
  seriesOptionsSchema,
  seriesSettingsSchema,
  statsSchema,
  createdSeriesSchema,
} from './schemas';

const SERIES_DTO = {
  id: 7,
  name: 'Daily Sketch',
  description: '',
  creator_id: '100000000000000001',
  cadence: 'daily',
  emoji: '✏️',
  start_day: 1,
  max_day: 128,
  state: 'active',
  privacy: 'public',
  is_owner: true,
  channel_ids: ['900000000000000101'],
  timezone: 'America/Chicago',
  last_posted_at: 1_790_000_000,
  total_days: 124,
  sprout: null,
};

const EMPTY_SPROUT_DTO = {
  ...SERIES_DTO,
  id: 8,
  max_day: null,
  state: 'sprout',
  privacy: 'creator_only',
  last_posted_at: null,
  total_days: 0,
  sprout: { archived: 0, threshold: 3 },
};

const SETTINGS_DTO = {
  id: 7,
  name: 'Daily Sketch',
  description: '',
  emoji: '✏️',
  cadence: 'daily',
  privacy: 'public',
  privacy_role_id: null,
  channel_id: '900000000000000101',
  detection_mode: 'context_menu',
  state: 'active',
  reminder_enabled: true,
  reminder_time: '20:00',
  reminder_timezone: null,
  reminder_dm: true,
  start_day: 1,
  reminder_error: 'dm_closed',
  reminder_error_at: 1_790_000_000,
};

let warn: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  // The schemas read a wrong-shaped optional field as absent and warn. For
  // the server's own shapes that would be drift, so no warning is allowed.
  warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
});
afterEach(() => {
  expect(warn).not.toHaveBeenCalled();
  warn.mockRestore();
});

describe('server responses parse without loss', () => {
  it('series list: SeriesDto, with and without a sprout', () => {
    const [active, sprout] = seriesListSchema.parse([SERIES_DTO, EMPTY_SPROUT_DTO]);
    expect(active).toMatchObject({
      id: 7,
      max_day: 128,
      state: 'active',
      privacy: 'public',
      is_owner: true,
      channel_ids: ['900000000000000101'],
      timezone: 'America/Chicago',
      last_posted_at: 1_790_000_000,
      total_days: 124,
    });
    expect(active?.sprout).toBeUndefined();
    expect(sprout?.sprout).toEqual({ archived: 0, threshold: 3 });
    expect(sprout?.last_posted_at).toBeUndefined();
    expect(sprout?.max_day ?? null).toBeNull();
  });

  it('created series: POST answers the full SeriesDto', () => {
    expect(createdSeriesSchema.parse(EMPTY_SPROUT_DTO)).toMatchObject({
      id: 8,
      name: 'Daily Sketch',
      emoji: '✏️',
      state: 'sprout',
    });
  });

  it('day index: DaySummaryDto, including a day with no stored file', () => {
    const rows = daySummaryListSchema.parse([
      {
        day: 1,
        posted_at: 1_780_000_000,
        thumb_url: '/api/media/a1?thumb=1&exp=1&sig=s',
        local_date: '2026-05-28',
        count: 2,
        missing: false,
      },
      {
        day: 2,
        posted_at: 1_780_086_400,
        thumb_url: null,
        local_date: '2026-05-29',
        count: 0,
        missing: true,
      },
    ]);
    expect(rows[0]).toMatchObject({ local_date: '2026-05-28', count: 2, missing: false });
    expect(rows[1]).toMatchObject({ thumb_url: null, count: 0, missing: true });
  });

  it('day: DayDto with MediaDto', () => {
    const day = daySchema.parse({
      day: 1,
      caption: 'first',
      posted_at: 1_780_000_000,
      jump_url: 'https://discord.com/channels/1/2/3',
      media: [
        {
          url: '/api/media/a1?exp=1&sig=s',
          thumb_url: '/api/media/a1?thumb=1&exp=1&sig=s',
          content_type: 'image/png',
          missing: false,
        },
      ],
    });
    expect(day.media).toHaveLength(1);
  });

  it('stats: StatsDto for a series with and without days', () => {
    const base = { total: 0, current_streak: 0, longest_streak: 0, missed: 0 };
    expect(statsSchema.parse({ ...base, max_day: null }).max_day).toBeNull();
    expect(statsSchema.parse({ ...base, total: 3, max_day: 3 }).max_day).toBe(3);
  });

  it('launch intent: null, a series, a series and day', () => {
    expect(launchIntentSchema.parse(null)).toBeNull();
    expect(launchIntentSchema.parse({ series_id: 7, day: null })).toMatchObject({ series_id: 7 });
    expect(launchIntentSchema.parse({ series_id: 7, day: 12 })).toMatchObject({ day: 12 });
  });

  it('token refresh: RefreshResponse', () => {
    expect(refreshSchema.parse({ token: 't', expires_in: 3600 })).toEqual({
      token: 't',
      expires_in: 3600,
    });
  });

  it('eligibility: EligibilityDto, params always sent for a policy violation', () => {
    const parsed = eligibilitySchema.parse({
      can_create: false,
      owns_any: true,
      violations: [
        {
          code: 'max_series',
          message: 'You’ve reached the limit.',
          params: { limit: 2, current: 2 },
        },
        // No specifics to give: the server still sends the key, as `{}`.
        { code: 'missing_creator_role', message: 'Needs the creator role.', params: {} },
        {
          code: 'account_too_new',
          message: 'Too new.',
          params: { days: 7, eligible_at: 1_790_000_000 },
        },
      ],
    });
    expect(parsed.owns_any).toBe(true);
    expect(parsed.violations[0]?.params).toEqual({ limit: 2, current: 2 });
    const empty = parsed.violations[1]?.params;
    expect(empty).toBeDefined();
    expect(Object.values(empty ?? {}).filter((v) => v !== undefined)).toEqual([]);
    expect(parsed.violations[2]?.params?.eligible_at).toBe(1_790_000_000);
  });

  it('eligibility: a server that is not set up, the one violation without params', () => {
    const parsed = eligibilitySchema.parse({
      can_create: false,
      owns_any: false,
      violations: [
        {
          code: 'guild_not_setup',
          message: 'leaf isn’t set up in this server yet. A server admin needs to run /setup.',
        },
      ],
    });
    expect(parsed.violations[0]?.code).toBe('guild_not_setup');
    expect(parsed.violations[0]?.params).toBeUndefined();
  });

  it('options: OptionsDto, with a channel leaf cannot name', () => {
    const parsed = seriesOptionsSchema.parse({
      channels: [
        { id: '900000000000000101', name: 'daily-sketch' },
        { id: '900000000000000102', name: null },
      ],
      roles: [{ id: '900000000000000201', name: 'Artist', held: true }],
      cadences: ['daily', 'weekdays', 'weekly', 'freeform'],
      privacy_modes: ['public', 'role_gated', 'creator_only'],
      guild_timezone: 'America/Chicago',
      sprout_enabled: true,
      sprout_threshold: 3,
      roles_unavailable: false,
    });
    expect(parsed.channels[1]?.name).toBeNull();
    expect(parsed.roles[0]?.held).toBe(true);
    expect(parsed.roles_unavailable).toBe(false);
  });

  it('my series: MySeriesDto rows', () => {
    const rows = mySeriesListSchema.parse([
      {
        id: 7,
        name: 'Daily Sketch',
        emoji: '✏️',
        state: 'active',
        cadence: 'daily',
        channel_id: '900000000000000101',
        channel_name: 'daily-sketch',
        archived_days: 124,
        reminder_enabled: true,
        reminder_error: 'dm_closed',
      },
      {
        id: 9,
        name: 'Old Polaroids',
        emoji: '🍃',
        state: 'revoked',
        cadence: 'freeform',
        channel_id: null,
        channel_name: null,
        archived_days: 0,
        reminder_enabled: false,
        reminder_error: null,
      },
    ]);
    expect(rows[0]?.reminder_error).toBe('dm_closed');
    expect(rows[1]?.reminder_error).toBeUndefined();
  });

  it('settings: SeriesSettingsDto, with and without a reminder failure', () => {
    const failing = seriesSettingsSchema.parse(SETTINGS_DTO);
    expect(failing).toMatchObject({
      start_day: 1,
      reminder_error: 'dm_closed',
      reminder_error_at: 1_790_000_000,
    });
    const fine = seriesSettingsSchema.parse({
      ...SETTINGS_DTO,
      reminder_enabled: false,
      reminder_time: null,
      reminder_error: null,
      reminder_error_at: null,
    });
    expect(fine.reminder_error).toBeUndefined();
    expect(fine.reminder_error_at).toBeUndefined();
  });
});
