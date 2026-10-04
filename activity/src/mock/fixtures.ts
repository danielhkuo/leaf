// Fixture data for the mock screen viewer (src/mock). Lets every screen render
// with realistic content and no Discord SDK, network, or auth — purely for
// design review. Not part of the production bundle (mock.html is dev-only).
//
// The gallery fixtures are served through the real API client (see api.ts),
// so they have to be what leaf-server would send: every optional field a
// current server sets is set here too.

import type { AdminApi } from '../lib/admin/client';
import type {
  AdminGuildDetail,
  AdminOptions,
  AdminSeries,
  AdminSettings,
  SeriesPatch,
  SettingsPatch,
} from '../lib/admin/schemas';
import { BootError } from '../lib/sdk/bootError';
import type { Session } from '../lib/sdk/handshake';
import type { Platform } from '../lib/sdk/types';
import type { SessionState } from '../lib/stores/session.svelte';
import type {
  Day,
  DaySummary,
  Eligibility,
  Media,
  MySeries,
  Series,
  SeriesOptions,
  SeriesSettings,
  Stats,
} from '../lib/types/api';
// Two seconds of a slow green gradient, 480 x 270, no sound, in both of the
// formats a browser may play. Made with ffmpeg from its `gradients` source
// (`s=480x270:d=2:r=12:speed=0.02`): `-c:v libvpx-vp9 -crf 50 -b:v 0` for the
// WebM, `-c:v libx264 -profile:v baseline -crf 34 -movflags +faststart` for
// the MP4. Nothing depends on the bytes beyond "small, valid, this shape".
import clipMp4 from './media/clip.mp4?url';
import clipWebm from './media/clip.webm?url';
import type { ScreenId } from './screens';

const NOW = Math.floor(Date.now() / 1000);
const HOUR = 3_600;
const DAY = 86_400;
/**
 * The newest post time in the fixtures: the latest noon UTC that has passed.
 * Days are posted whole days before it, which is early morning in the
 * server's timezone, so no fixture time sits near a midnight or a clock
 * change there and each lands on the date it is meant to.
 */
const LATEST_POST = Math.floor((NOW - 12 * HOUR) / DAY) * DAY + 12 * HOUR;

export const GUILD_ID = '900000000000000009';
/** The signed-in viewer; owned series use this id so the owner controls show. */
export const USER_ID = '100000000000000001';
/** The server's timezone: the calendar's dates are worked out in it. */
export const TIMEZONE = 'America/Chicago';

const CHANNEL = {
  sketch: '200000000000000001',
  share: '200000000000000002',
  general: '200000000000000003',
  hidden: '200000000000000004',
  /** Deleted since a series was started in it: no longer a series channel. */
  deleted: '200000000000000005',
  log: '200000000000000009',
} as const;

const ROLE = {
  artist: '300000000000000001',
  patron: '300000000000000002',
  member: '300000000000000003',
} as const;

/** A session as the handshake would produce it, launched from #daily-sketch. */
export function mockSession(platform: Platform): Session {
  return {
    user: { id: USER_ID, username: 'mika', global_name: 'Mika' },
    guildId: GUILD_ID,
    channelId: CHANNEL.sketch,
    platform,
    appName: 'leaf',
    customId: null,
    token: 'mock-token',
    expiresAt: Date.now() + 6 * HOUR * 1000,
  };
}

/**
 * A colorful inline-SVG placeholder so the viewer needs no real media:
 * `size` square, or `size` wide and `height` tall.
 */
export function placeholder(label: string, hue: number, size = 600, height = size): string {
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${height}">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">` +
    `<stop offset="0" stop-color="hsl(${hue},70%,72%)"/>` +
    `<stop offset="1" stop-color="hsl(${(hue + 40) % 360},65%,56%)"/>` +
    `</linearGradient></defs>` +
    `<rect width="${size}" height="${height}" fill="url(#g)"/>` +
    `<text x="50%" y="53%" font-family="Georgia,serif" font-size="${height / 6}" ` +
    `fill="rgba(32,32,32,0.5)" text-anchor="middle" dominant-baseline="middle">${label}</text>` +
    `</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

// --- worst-case text ---

/**
 * Text as long as each typed field can be, with nowhere to wrap: what a
 * layout has to survive. Series names and descriptions are at leaf's limits
 * (40 and 200) and display names at Discord's (32). Channel and role names
 * can run to 100; 60 and 40 already break a layout that does not wrap.
 */
const WORST_CASE = new Map([
  ['name', 'W'.repeat(40)],
  ['description', 'W'.repeat(200)],
  ['caption', `${'W'.repeat(120)} https://example.com/${'w'.repeat(120)}`],
  ['channel_name', 'w'.repeat(60)],
  ['creator_name', 'W'.repeat(32)],
  ['privacy_role_name', 'W'.repeat(40)],
  ['role_name', 'W'.repeat(40)],
]);

/**
 * A copy of an API answer with every name, description and caption in it
 * replaced by its worst case (see the screen viewer's "Long text" switch).
 */
export function worstCase<T>(value: T): T {
  if (Array.isArray(value)) return value.map((item: unknown) => worstCase(item)) as T;
  if (value === null || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.entries(value).map(([key, field]: [string, unknown]) => {
      const worst = typeof field === 'string' ? WORST_CASE.get(key) : undefined;
      return [key, worst ?? worstCase(field)];
    }),
  ) as T;
}

// --- gallery: series list (ids 1..7 so every per-series accent shows) ---

/** Fields every fixture series shares; each entry overrides what differs. */
function listed(
  s: Pick<Series, 'id' | 'name' | 'description' | 'emoji' | 'cadence' | 'max_day'> &
    Partial<Series>,
): Series {
  return {
    creator_id: USER_ID,
    start_day: 1,
    state: 'active',
    privacy: 'public',
    is_owner: (s.creator_id ?? USER_ID) === USER_ID,
    channel_ids: [CHANNEL.share],
    timezone: TIMEZONE,
    total_days: s.max_day ?? 0,
    ...(s.max_day === null ? {} : { last_posted_at: LATEST_POST }),
    ...s,
  };
}

export const series: Series[] = [
  listed({
    id: 7,
    name: 'Daily Sketch',
    description: 'One drawing a day, rain or shine.',
    cadence: 'daily',
    emoji: '✏️',
    max_day: 128,
    channel_ids: [CHANNEL.sketch],
    total_days: 124,
  }),
  // The viewer's own sprout: hidden from everyone else until 3 days are in.
  listed({
    id: 1,
    name: 'Morning Coffee',
    description: 'My cup, every single morning.',
    cadence: 'daily',
    emoji: '☕',
    max_day: 2,
    state: 'sprout',
    channel_ids: [CHANNEL.general],
    sprout: { archived: 2, threshold: 3 },
  }),
  listed({
    id: 2,
    name: 'Trail Runs',
    description: 'Where my feet took me this week.',
    creator_id: '100000000000000002',
    cadence: 'weekly',
    emoji: '🏃',
    max_day: 22,
    privacy: 'role_gated',
  }),
  listed({
    id: 3,
    name: 'Sourdough Log',
    description: 'Loaf by loaf, crumb by crumb.',
    creator_id: '100000000000000003',
    cadence: 'freeform',
    emoji: '🍞',
    max_day: 41,
  }),
  listed({
    id: 4,
    name: 'City Windows',
    description: 'Light through glass.',
    creator_id: '100000000000000004',
    cadence: 'daily',
    emoji: '🌆',
    max_day: 90,
    channel_ids: [CHANNEL.sketch],
  }),
  // The longest name leaf allows (40 characters), to show how it wraps.
  listed({
    id: 5,
    name: 'Tiny Plants on a North-Facing Windowsill',
    description: 'Watching things grow, slowly, with whatever light there is.',
    creator_id: '100000000000000005',
    cadence: 'weekly',
    emoji: '🪴',
    max_day: 15,
  }),
  listed({
    id: 6,
    name: 'Night Skies',
    description: 'Whatever the dark shows me.',
    creator_id: '100000000000000006',
    cadence: 'freeform',
    emoji: '🌙',
    max_day: 33,
  }),
  // Just started and private: the owner's "archive your first post" state.
  listed({
    id: 8,
    name: 'Pressed Flowers',
    description: '',
    cadence: 'weekly',
    emoji: '🌸',
    max_day: null,
    state: 'sprout',
    privacy: 'creator_only',
    channel_ids: [CHANNEL.general],
    sprout: { archived: 0, threshold: 3 },
  }),
  // Revoked by an admin: only its owner still sees it, and it cannot be opened.
  listed({
    id: 9,
    name: 'Old Polaroids',
    description: 'A shoebox, one print at a time.',
    cadence: 'freeform',
    emoji: '📸',
    max_day: 58,
    state: 'revoked',
  }),
];

/** Looks a fixture series up by id. */
function fixtureSeries(id: number): Series {
  const found = series.find((s) => s.id === id);
  if (!found) throw new Error(`no fixture series with id ${id}`);
  return found;
}

/** The series the Home, viewer and settings screens open. */
export const homeSeries: Series = fixtureSeries(7);

/**
 * The same list after Daily Sketch's channel was deleted. The series keeps
 * the channel's id, and its owner's list ({@link mineOf}) says it is gone.
 */
export const seriesChannelGone: Series[] = series.map((s) =>
  s.id === homeSeries.id ? { ...s, channel_ids: [CHANNEL.deleted] } : s,
);

// --- gallery: archived days ---

const dateParts = new Intl.DateTimeFormat('en-US', {
  timeZone: TIMEZONE,
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
});

/** `YYYY-MM-DD` in the server's timezone, as the days index carries it. */
function localDate(unix: number): string {
  const parts = dateParts.formatToParts(new Date(unix * 1000));
  const part = (type: string): string => parts.find((p) => p.type === type)?.value ?? '';
  return `${part('year')}-${part('month')}-${part('day')}`;
}

/** Day numbers Daily Sketch never used: its "skipped day numbers". */
const SKIPPED = new Set([31, 62, 93, 124]);
/** Days brought in by /import with no file: the hatched "no picture" tile. */
const isPlaceholderDay = (day: number): boolean => day % 17 === 0;
const attachmentCount = (day: number): number => (day % 5 === 0 ? 3 : 1);

/**
 * When Daily Sketch's days were posted: one a day, with a two-month pause
 * after Day 82 (the calendar collapses the empty month into one line) and
 * Day 121 posted ten minutes after Day 120 (a catch-up, so that date's cell
 * holds two days).
 */
function sketchPostedAt(day: number): number {
  if (day === 121) return sketchPostedAt(120) + 600;
  const pause = day <= 82 ? 60 : 0;
  return LATEST_POST - (128 - day + pause) * DAY;
}

function summary(seriesId: number, day: number, postedAt: number): DaySummary {
  const missing = isPlaceholderDay(day);
  return {
    day,
    posted_at: postedAt,
    thumb_url: missing ? null : placeholder(String(day), (day * 23 + seriesId * 40) % 360, 160),
    local_date: localDate(postedAt),
    count: attachmentCount(day),
    missing,
  };
}

/** Daily Sketch's whole index: 124 archived days, oldest first. */
export const dayIndex: DaySummary[] = Array.from({ length: 128 }, (_, i) => i + 1)
  .filter((day) => !SKIPPED.has(day))
  .map((day) => summary(7, day, sketchPostedAt(day)));

/** Consistent with {@link dayIndex}: runs of 30, 30, 30, 30 and 4 days. */
export const stats: Stats = {
  total: 124,
  current_streak: 4,
  longest_streak: 30,
  missed: 4,
  max_day: 128,
};

const indexes = new Map<number, DaySummary[]>([[7, dayIndex]]);

/** Days between posts for the series that have no hand-made index. */
function spacing(cadence: string): number {
  if (cadence === 'weekly') return 7;
  return cadence === 'freeform' ? 3 : 1;
}

/** The archived-day index of any series: every day up to its newest. */
export function indexOf(s: Series): DaySummary[] {
  const known = indexes.get(s.id);
  if (known) return known;
  const last = s.max_day ?? 0;
  const step = spacing(s.cadence) * DAY;
  const built = Array.from({ length: last }, (_, i) => i + 1).map((day) =>
    summary(s.id, day, LATEST_POST - (last - day) * step),
  );
  indexes.set(s.id, built);
  return built;
}

export function statsOf(s: Series): Stats {
  if (s.id === 7) return stats;
  const total = indexOf(s).length;
  return {
    total,
    current_streak: total,
    longest_streak: total,
    missed: 0,
    max_day: s.max_day,
  };
}

const CAPTIONS = [
  'Quiet morning, soft light.',
  'Three quick studies before the coffee went cold.',
  'Golden hour over the ridge. Almost didn’t catch it.',
  '',
];
/** A long caption with a line break and a link that cannot wrap at a space. */
const LONG_CAPTION =
  'Tried the new brush pens on the train: the paper buckles a little, but the greys layer well ' +
  'and the ink dries before the next stop. Second and third pictures are the same view on the ' +
  'way back.\nReference photos: https://example.com/albums/2024/commute-sketches/reference-set-03';

/** The day the viewer screens open: three pictures and the long caption. */
export const VIEWER_DAY = 125;

/** The day of Daily Sketch whose one file is a video (480 x 270, two seconds). */
export const VIDEO_DAY = 118;

/**
 * The mock's clip in a format this browser plays. WebM first: a Chromium
 * built without H.264 (Playwright's, on some systems) plays nothing else,
 * and the MP4 is for a browser with no WebM.
 */
function clip(): Pick<Media, 'url' | 'content_type'> {
  const webm =
    typeof document !== 'undefined' &&
    document.createElement('video').canPlayType('video/webm; codecs="vp9"') !== '';
  return webm
    ? { url: clipWebm, content_type: 'video/webm' }
    : { url: clipMp4, content_type: 'video/mp4' };
}

/** One archived day with its media, or `null` when the series has no such day. */
export function dayOf(s: Series, day: number): Day | null {
  const row = indexOf(s).find((r) => r.day === day);
  if (!row) return null;
  const hue = (day * 23 + s.id * 40) % 360;
  const video = s.id === homeSeries.id && day === VIDEO_DAY;
  return {
    day,
    caption: day % 25 === 0 ? LONG_CAPTION : (CAPTIONS[day % CAPTIONS.length] ?? ''),
    posted_at: row.posted_at,
    jump_url: `https://discord.com/channels/${GUILD_ID}/${s.channel_ids?.[0] ?? CHANNEL.share}/40000000000000${String(day).padStart(4, '0')}`,
    media: video
      ? // Its poster is a stored thumbnail's size: the clip's shape, 256px long.
        [{ ...clip(), thumb_url: placeholder(String(day), hue, 256, 144), missing: false }]
      : Array.from({ length: row.count ?? 1 }, (_, i) => ({
          url: row.missing ? '' : placeholder(`${day}.${i + 1}`, (hue + i * 30) % 360, 1200),
          thumb_url: row.missing ? '' : placeholder(`${day}.${i + 1}`, (hue + i * 30) % 360, 160),
          content_type: 'image/png',
          missing: row.missing ?? false,
        })),
  };
}

/**
 * A day number Daily Sketch skipped, inside its range: the server has no
 * such day, so a viewer opened on it shows its "couldn't load" state.
 */
export const MISSING_DAY = 124;

/** {@link VIEWER_DAY} of Daily Sketch, for component tests. */
export const viewerDay: Day = (() => {
  const day = dayOf(homeSeries, VIEWER_DAY);
  if (!day) throw new Error(`Daily Sketch has no Day ${VIEWER_DAY}`);
  return day;
})();

// --- creator ---

export const options: SeriesOptions = {
  channels: [
    { id: CHANNEL.sketch, name: 'daily-sketch' },
    { id: CHANNEL.share, name: 'art-share' },
    { id: CHANNEL.general, name: 'general' },
    // Watched, but leaf has lost sight of it: shown by the tail of its id.
    { id: CHANNEL.hidden, name: null },
  ],
  roles: [
    { id: ROLE.artist, name: 'Artist', held: true },
    { id: ROLE.patron, name: 'Patron', held: false },
    { id: ROLE.member, name: 'Member', held: true },
  ],
  cadences: ['daily', 'weekdays', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: TIMEZONE,
  sprout_enabled: true,
  sprout_threshold: 3,
};

/**
 * The form's choices in a server where no admin has run /setup since
 * {@link seriesChannelGone}'s channel was deleted: it is still the one series
 * channel on the server's list, and there is no name for it.
 */
export const optionsChannelUnseen: SeriesOptions = {
  ...options,
  channels: [{ id: CHANNEL.deleted, name: null }],
};

/**
 * What only the owner's own endpoints know about a series: its reminders and
 * the role a role-gated one is limited to.
 */
export type OwnerSettings = Partial<
  Pick<
    SeriesSettings,
    | 'privacy_role_id'
    | 'reminder_enabled'
    | 'reminder_time'
    | 'reminder_timezone'
    | 'reminder_dm'
    | 'reminder_error'
    | 'reminder_error_at'
  >
>;

export const ownerSettings: Readonly<Record<number, OwnerSettings>> = {
  7: { reminder_enabled: true, reminder_time: '21:00' },
  // A reminder Discord refused: the settings screen says why and what to do.
  1: {
    reminder_enabled: true,
    reminder_time: '08:30',
    reminder_error: 'dm_closed',
    reminder_error_at: NOW - 2 * DAY,
  },
};

/** `GET .../series/{id}/settings` for an owned series. */
export function settingsOf(s: Series, own: OwnerSettings = {}): SeriesSettings {
  return {
    id: s.id,
    name: s.name,
    description: s.description,
    emoji: s.emoji,
    cadence: s.cadence,
    privacy: s.privacy ?? 'public',
    privacy_role_id: null,
    channel_id: s.channel_ids?.[0] ?? null,
    detection_mode: 'context_menu',
    state: s.state ?? 'active',
    reminder_enabled: false,
    reminder_time: null,
    reminder_timezone: null,
    reminder_dm: true,
    start_day: s.start_day,
    ...own,
  };
}

/**
 * A row of `GET .../series/mine`. A channel leaf cannot name (deleted, so
 * not among the server's series channels, or listed there with no name) is
 * sent with no name and flagged as missing. A series with no channel has
 * none to miss.
 */
export function mineOf(
  s: Series,
  own: OwnerSettings = {},
  channels: SeriesOptions['channels'] = options.channels,
): MySeries {
  const channelId = s.channel_ids?.[0] ?? null;
  const channelName = channels.find((c) => c.id === channelId)?.name;
  return {
    id: s.id,
    name: s.name,
    emoji: s.emoji,
    state: s.state ?? 'active',
    cadence: s.cadence,
    channel_id: channelId,
    ...(channelName ? { channel_name: channelName } : {}),
    channel_missing: channelId !== null && !channelName,
    archived_days: s.total_days ?? 0,
    reminder_enabled: own.reminder_enabled ?? false,
  };
}

/** Daily Sketch's settings, for component tests. */
export const seriesSettings: SeriesSettings = settingsOf(homeSeries, ownerSettings[7]);

export const eligibilityOk: Eligibility = { can_create: true, violations: [], owns_any: true };

/** A newcomer without the creator role: two rules are in the way. */
export const eligibilityBlocked: Eligibility = {
  can_create: false,
  violations: [
    {
      code: 'missing_creator_role',
      message: 'Starting a series here needs the creator role. Ask a server admin for it.',
      params: { role_name: 'Artist' },
    },
    {
      code: 'membership_too_new',
      message:
        'You need to have been a member of this server for at least 7 days to start a series.',
      params: { days: 7, eligible_at: NOW + 4 * DAY },
    },
  ],
  owns_any: false,
};

// --- boot ---

/** What the shell shows before the gallery: a boot in progress, or one that failed. */
export type BootState = Exclude<SessionState, { status: 'authed' }>;

/** The boot screens. Each is a state the boot can end in, with the detail it logs. */
export const bootScreens: ReadonlyMap<string, BootState> = new Map([
  ['loading', { status: 'loading', step: 'ready', slow: false }],
  // The permission sheet is up, or was dismissed: the one wait with no time limit.
  ['loading-consent', { status: 'loading', step: 'authorize', slow: true }],
  [
    'error',
    { status: 'error', error: new BootError('ready_timeout', 'ready: no answer from Discord') },
  ],
  [
    'error-retry',
    { status: 'error', error: new BootError('network', 'POST /token → no response') },
  ],
  // Minimised (see the tile screens in Screen.svelte): the first two again.
  ['tile-boot', { status: 'loading', step: 'ready', slow: false }],
  [
    'tile-error',
    { status: 'error', error: new BootError('ready_timeout', 'ready: no answer from Discord') },
  ],
] satisfies [ScreenId, BootState][]);

// --- admin ---

export const guildDetail: AdminGuildDetail = {
  guild_id: GUILD_ID,
  name: 'Sketchbook Club',
  setup_complete: true,
  settings: {
    timezone: TIMEZONE,
    creator_role_id: ROLE.artist,
    log_channel_id: CHANNEL.log,
    max_series_per_user: 5,
    min_account_age_days: 30,
    min_membership_age_days: 7,
    sprout_enabled: true,
    sprout_threshold: 3,
  },
  series: [
    {
      id: 7,
      name: 'Daily Sketch',
      creator_id: USER_ID,
      creator_name: 'Mika',
      privacy: 'public',
      privacy_role_id: null,
      state: 'active',
      archived_days: 124,
    },
    {
      id: 2,
      name: 'Trail Runs',
      creator_id: '100000000000000002',
      creator_name: 'Noor',
      privacy: 'role_gated',
      privacy_role_id: ROLE.patron,
      privacy_role_name: 'Patron',
      state: 'active',
      archived_days: 22,
    },
    {
      id: 1,
      name: 'Morning Coffee',
      creator_id: USER_ID,
      creator_name: 'Mika',
      privacy: 'public',
      privacy_role_id: null,
      state: 'sprout',
      archived_days: 2,
    },
    // Its creator has left the server, so there is no name to show.
    {
      id: 9,
      name: 'Old Polaroids',
      creator_id: '100000000000000007',
      privacy: 'creator_only',
      privacy_role_id: null,
      state: 'revoked',
      archived_days: 58,
    },
  ],
};

export const adminOptions: AdminOptions = {
  roles: [
    { id: ROLE.artist, name: 'Artist' },
    { id: ROLE.patron, name: 'Patron' },
    { id: ROLE.member, name: 'Member' },
  ],
  channels: [
    { id: CHANNEL.general, name: 'general' },
    { id: CHANNEL.sketch, name: 'daily-sketch' },
    { id: CHANNEL.log, name: 'leaf-log' },
  ],
};

/** The stored settings after `patch`: `""` clears a role or channel. */
function patchedSettings(stored: AdminSettings, patch: SettingsPatch): AdminSettings {
  const { creator_role_id, log_channel_id, ...rest } = patch;
  return {
    ...stored,
    ...rest,
    ...(creator_role_id === undefined ? {} : { creator_role_id: creator_role_id || null }),
    ...(log_channel_id === undefined ? {} : { log_channel_id: log_channel_id || null }),
  };
}

/**
 * A stand-in for the real AdminApi, answering from the fixtures (with
 * worst-case names when `longText` is set). Saved changes are kept until the
 * page reloads, so a save reads back as stored.
 */
export function createMockAdminApi(longText = false): AdminApi {
  const stored = longText ? worstCase(guildDetail) : structuredClone(guildDetail);
  const choices = longText ? worstCase(adminOptions) : adminOptions;
  const api: Pick<AdminApi, 'listGuilds' | 'guild' | 'options' | 'patchSettings' | 'patchSeries'> =
    {
      listGuilds: () =>
        Promise.resolve([
          {
            guild_id: stored.guild_id,
            name: stored.name,
            icon_url: stored.icon_url,
            series_count: stored.series.length,
          },
        ]),
      guild: () => Promise.resolve(structuredClone(stored)),
      options: () => Promise.resolve(structuredClone(choices)),
      patchSettings: (_gid: string, patch: SettingsPatch) => {
        stored.settings = patchedSettings(stored.settings, patch);
        return Promise.resolve({ ...stored.settings, sprouts_published: 0 });
      },
      patchSeries: (_gid: string, seriesId: number, patch: SeriesPatch) => {
        const at = stored.series.findIndex((s) => s.id === seriesId);
        const found = stored.series[at];
        if (!found) return Promise.reject(new Error(`no fixture series with id ${seriesId}`));
        const privacy = patch.privacy ?? found.privacy;
        // Only a role-gated series keeps a role.
        const roleId =
          privacy === 'role_gated' ? (patch.privacy_role_id ?? found.privacy_role_id) : null;
        const updated: AdminSeries = {
          ...found,
          privacy,
          privacy_role_id: roleId,
          privacy_role_name: choices.roles?.find((role) => role.id === roleId)?.name,
          state: patch.state ?? found.state,
        };
        stored.series[at] = updated;
        return Promise.resolve({ ...updated });
      },
    };
  return api as AdminApi;
}
