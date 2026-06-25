// Fixture data for the mock screen viewer (src/mock). Lets every screen render
// with realistic content and no Discord SDK, network, or auth — purely for
// design review. Not part of the production bundle (mock.html is dev-only).

import type { AdminApi } from '../lib/admin/client';
import type { AdminGuildDetail, SeriesPatch, SettingsPatch } from '../lib/admin/schemas';
import type {
  Day,
  DaySummary,
  Eligibility,
  MySeries,
  Series,
  SeriesOptions,
  SeriesSettings,
  Stats,
} from '../lib/types/api';

const NOW = Math.floor(Date.now() / 1000);
const DAY = 86_400;

/** The signed-in viewer; owned series use this id so the owner controls show. */
export const USER_ID = '100000000000000001';

/** A colorful inline-SVG placeholder so the viewer needs no real media. */
export function placeholder(label: string, hue: number, size = 600): string {
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">` +
    `<stop offset="0" stop-color="hsl(${hue},70%,72%)"/>` +
    `<stop offset="1" stop-color="hsl(${(hue + 40) % 360},65%,56%)"/>` +
    `</linearGradient></defs>` +
    `<rect width="${size}" height="${size}" fill="url(#g)"/>` +
    `<text x="50%" y="53%" font-family="Georgia,serif" font-size="${size / 6}" ` +
    `fill="rgba(32,32,32,0.5)" text-anchor="middle" dominant-baseline="middle">${label}</text>` +
    `</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

// --- gallery: series list (ids span 1..7 so every per-series accent shows) ---

export const series: Series[] = [
  { id: 7, name: 'Daily Sketch', description: 'One drawing a day, rain or shine.', creator_id: USER_ID, cadence: 'daily', emoji: '✏️', start_day: 1, max_day: 128 },
  { id: 1, name: 'Morning Coffee', description: 'My cup, every single morning.', creator_id: USER_ID, cadence: 'daily', emoji: '☕', start_day: 1, max_day: 64 },
  { id: 2, name: 'Trail Runs', description: 'Where my feet took me this week.', creator_id: '222', cadence: 'weekly', emoji: '🏃', start_day: 1, max_day: 22 },
  { id: 3, name: 'Sourdough Log', description: 'Loaf by loaf, crumb by crumb.', creator_id: '333', cadence: 'freeform', emoji: '🍞', start_day: 1, max_day: 41 },
  { id: 4, name: 'City Windows', description: 'Light through glass.', creator_id: '444', cadence: 'daily', emoji: '🌆', start_day: 1, max_day: 90 },
  { id: 5, name: 'Tiny Plants', description: 'Watching things grow.', creator_id: '555', cadence: 'weekly', emoji: '🪴', start_day: 1, max_day: 15 },
  { id: 6, name: 'Night Skies', description: 'Whatever the dark shows me.', creator_id: '666', cadence: 'freeform', emoji: '🌙', start_day: 1, max_day: 33 },
];

/** The series shown on the Home screen. */
export const homeSeries: Series = series[0] as Series;

export const stats: Stats = {
  total: 124,
  current_streak: 12,
  longest_streak: 31,
  missed: 4,
  max_day: 128,
};

/** A present-day index for the calendar: ~46 days back from today, some gaps. */
export const dayIndex: DaySummary[] = Array.from({ length: 46 }, (_, i) => {
  const day = 128 - i;
  const hasThumb = i % 5 !== 0; // every 5th day is a gap/missing thumbnail
  return {
    day,
    posted_at: NOW - i * DAY,
    thumb_url: hasThumb ? placeholder(String(day), (day * 23) % 360, 160) : null,
  };
}).filter((_, i) => i % 7 !== 3); // drop one weekday each week, so gaps read

// --- viewer: a few full days with media carousels ---

function makeDay(day: number, hue: number, attachments: number, caption: string): Day {
  return {
    day,
    caption,
    posted_at: NOW - (130 - day) * DAY,
    jump_url: 'https://discord.com/channels/0/0/0',
    media: Array.from({ length: attachments }, (_, i) => ({
      url: placeholder(`${day}.${i + 1}`, (hue + i * 30) % 360, 1200),
      thumb_url: placeholder(`${day}.${i + 1}`, (hue + i * 30) % 360, 160),
      content_type: 'image/png',
      missing: false,
    })),
  };
}

export const viewerDays: Day[] = [
  makeDay(126, 210, 1, 'Quiet morning, soft light.'),
  makeDay(127, 30, 3, 'Three quick studies before the coffee went cold.'),
  makeDay(128, 140, 2, "Golden hour over the ridge — almost didn't catch it."),
];

// --- creator ---

export const options: SeriesOptions = {
  channels: [
    { id: 'c1', name: 'daily-sketch' },
    { id: 'c2', name: 'art-share' },
    { id: 'c3', name: 'general' },
  ],
  roles: [
    { id: 'r1', name: 'Patron' },
    { id: 'r2', name: 'Member' },
  ],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: true,
  sprout_threshold: 3,
};

export const mySeries: MySeries[] = [
  { id: 7, name: 'Daily Sketch', emoji: '✏️', state: 'active', cadence: 'daily', channel_id: 'c1', channel_name: 'daily-sketch', archived_days: 124, reminder_enabled: true },
  { id: 1, name: 'Morning Coffee', emoji: '☕', state: 'sprout', cadence: 'daily', channel_id: 'c3', channel_name: 'general', archived_days: 2, reminder_enabled: false },
  { id: 9, name: 'Old Polaroids', emoji: '📸', state: 'revoked', cadence: 'freeform', channel_id: null, channel_name: null, archived_days: 58, reminder_enabled: false },
];

export const seriesSettings: SeriesSettings = {
  id: 7,
  name: 'Daily Sketch',
  description: 'One drawing a day, rain or shine.',
  emoji: '✏️',
  cadence: 'daily',
  privacy: 'public',
  privacy_role_id: null,
  channel_id: 'c1',
  detection_mode: 'context_menu',
  state: 'active',
  reminder_enabled: true,
  reminder_time: '21:00',
  reminder_timezone: 'America/Chicago',
  reminder_dm: true,
};

export const eligibilityOk: Eligibility = { can_create: true, violations: [] };

export const eligibilityBlocked: Eligibility = {
  can_create: false,
  violations: [
    { code: 'account_age', message: 'Your Discord account must be at least 30 days old.' },
    { code: 'max_series', message: "You've reached the limit of 3 active series." },
  ],
};

// --- admin ---

export const guildDetail: AdminGuildDetail = {
  guild_id: '900000000000000009',
  settings: {
    timezone: 'America/Chicago',
    creator_role_id: 'r1',
    log_channel_id: 'c9',
    max_series_per_user: 3,
    min_account_age_days: 30,
    min_membership_age_days: 7,
    sprout_enabled: true,
    sprout_threshold: 3,
  },
  series: [
    { id: 7, name: 'Daily Sketch', creator_id: '100…001', privacy: 'public', privacy_role_id: null, state: 'active' },
    { id: 2, name: 'Trail Runs', creator_id: '222…222', privacy: 'role_gated', privacy_role_id: 'r1', state: 'active' },
    { id: 9, name: 'Old Polaroids', creator_id: '333…333', privacy: 'creator_only', privacy_role_id: null, state: 'revoked' },
  ],
};

/** A stand-in for the real AdminApi: returns fixtures, persists nothing. */
export const mockAdminApi = {
  listGuilds: async () => [
    { guild_id: guildDetail.guild_id, series_count: guildDetail.series.length },
  ],
  guild: async () => structuredClone(guildDetail),
  patchSettings: async (_gid: string, patch: SettingsPatch) => ({
    ...guildDetail.settings,
    ...patch,
  }),
  patchSeries: async (_gid: string, seriesId: number, patch: SeriesPatch) => {
    const found = guildDetail.series.find((s) => s.id === seriesId) ?? guildDetail.series[0];
    return { ...found, ...patch };
  },
} as unknown as AdminApi;
