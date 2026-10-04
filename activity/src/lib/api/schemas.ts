// Zod schemas mirroring leaf-server's `api::dto` structs. Every API response
// is parsed through these, so a contract drift surfaces as a runtime error at
// the boundary instead of `undefined` deep in a component.
//
// Fields added after the first release go through `opt()`: an older server
// omits them and a newer one may send `null`, and the Activity has to work
// against both. Zod drops keys a schema doesn't name, so a field the views
// should see must be listed here.

import { z } from 'zod';

/**
 * An optional field; absent and `null` both read as `undefined`. So does a
 * value of the wrong shape: every view already copes without these fields,
 * so one that drifted costs a detail on screen, not the whole response.
 */
function opt<T extends z.ZodTypeAny>(schema: T) {
  return schema
    .nullish()
    .catch(({ error }) => {
      console.warn('leaf: ignored a field the server sent in an unexpected shape', error.issues);
      return undefined;
    })
    .transform((v: z.output<T> | null | undefined) => v ?? undefined);
}

/** A sprout's progress towards showing in the gallery. */
export const sproutSchema = z.object({
  archived: z.number(),
  threshold: z.number(),
});

export const seriesSchema = z.object({
  id: z.number(),
  name: z.string(),
  description: z.string(),
  creator_id: z.string(),
  cadence: z.string(),
  emoji: z.string(),
  start_day: z.number(),
  max_day: z.number().nullable(),
  /** `sprout` / `active` / `revoked`. The owner's revoked series are listed. */
  state: opt(z.string()),
  /** `public` / `role_gated` / `creator_only`. */
  privacy: opt(z.string()),
  is_owner: opt(z.boolean()),
  /** Channels the series archives from. */
  channel_ids: opt(z.array(z.string())),
  /** IANA zone the days' `local_date` values are computed in. */
  timezone: opt(z.string()),
  /** Newest post time, unix seconds. */
  last_posted_at: opt(z.number()),
  total_days: opt(z.number()),
  /** Set only while the series is a sprout. */
  sprout: opt(sproutSchema),
});
export const seriesListSchema = z.array(seriesSchema);

export const mediaSchema = z.object({
  url: z.string(),
  thumb_url: z.string(),
  content_type: z.string(),
  missing: z.boolean(),
});

export const daySchema = z.object({
  day: z.number(),
  caption: z.string(),
  posted_at: z.number(),
  jump_url: z.string(),
  media: z.array(mediaSchema),
});

export const daySummarySchema = z.object({
  day: z.number(),
  posted_at: z.number(),
  thumb_url: z.string().nullable(),
  /** `YYYY-MM-DD` in the server's timezone: the calendar cell for this day. */
  local_date: opt(z.string()),
  /** Number of media files on the day. */
  count: opt(z.number()),
  /** True when no file was captured for the day (imported placeholder). */
  missing: opt(z.boolean()),
});
export const daySummaryListSchema = z.array(daySummarySchema);

export const statsSchema = z.object({
  total: z.number(),
  current_streak: z.number(),
  longest_streak: z.number(),
  missed: z.number(),
  max_day: z.number().nullable(),
});

export const exchangeSchema = z.object({
  token: z.string(),
  access_token: z.string(),
  expires_in: z.number(),
});

/** `POST /api/token/refresh`: a fresh leaf token and its lifetime in seconds. */
export const refreshSchema = z.object({
  token: z.string(),
  expires_in: z.number(),
});

/** `GET .../launch-intent`: where a button in chat asked the gallery to open. */
export const launchIntentSchema = z
  .object({
    series_id: z.number(),
    day: opt(z.number()),
  })
  .nullable();

// --- creator series management ---

/** The specifics behind a violation; which keys are set depends on the code. */
export const violationParamsSchema = z.object({
  limit: opt(z.number()),
  current: opt(z.number()),
  days: opt(z.number()),
  /** Unix seconds at which an age rule stops applying. */
  eligible_at: opt(z.number()),
  role_name: opt(z.string()),
});

export const violationSchema = z.object({
  code: z.string(),
  message: z.string().default(''),
  params: opt(violationParamsSchema),
});

export const eligibilitySchema = z.object({
  can_create: z.boolean(),
  violations: z.array(violationSchema),
  /** Whether the viewer has created any series here, revoked ones included. */
  owns_any: opt(z.boolean()),
});

export const namedIdSchema = z.object({
  id: z.string(),
  name: z.string(),
});

/** A channel a series may use; `name` is null when Discord couldn't supply it. */
export const channelOptionSchema = z.object({
  id: z.string(),
  name: z.string().nullable(),
});

/** A role a series may be gated on; `held` marks roles the viewer has. */
export const roleOptionSchema = namedIdSchema.extend({
  held: opt(z.boolean()),
});

export const seriesOptionsSchema = z.object({
  channels: z.array(channelOptionSchema),
  roles: z.array(roleOptionSchema),
  cadences: z.array(z.string()),
  privacy_modes: z.array(z.string()),
  guild_timezone: z.string(),
  sprout_enabled: z.boolean(),
  sprout_threshold: z.number(),
  /** True when the role list could not be loaded (`roles` is then empty). */
  roles_unavailable: opt(z.boolean()),
});

// `POST /series`: the full series from a current server; an older one sends
// only id, name, state and emoji.
export const createdSeriesSchema = seriesSchema.partial().extend({
  id: z.number(),
  name: z.string(),
  emoji: z.string(),
});

export const mySeriesSchema = z.object({
  id: z.number(),
  name: z.string(),
  emoji: z.string(),
  state: z.string(),
  cadence: z.string(),
  channel_id: z.string().nullable(),
  /** Without `#`. Not there when leaf cannot see the channel (deleted, or hidden from it). */
  channel_name: opt(z.string()),
  /**
   * True when the series' channel is gone or hidden from leaf. A server that
   * does not send it says the same with no name; an older one may still send
   * the name it last knew. Read both through `channelOf` (utils/channel.ts).
   */
  channel_missing: opt(z.boolean()),
  archived_days: z.number(),
  reminder_enabled: z.boolean(),
  /** Why the last reminder could not be delivered, if it could not. */
  reminder_error: opt(z.string()),
});
export const mySeriesListSchema = z.array(mySeriesSchema);

export const seriesSettingsSchema = z.object({
  id: z.number(),
  name: z.string(),
  description: z.string(),
  emoji: z.string(),
  cadence: z.string(),
  privacy: z.string(),
  privacy_role_id: z.string().nullable(),
  channel_id: z.string().nullable(),
  detection_mode: z.string(),
  state: z.string(),
  reminder_enabled: z.boolean(),
  reminder_time: z.string().nullable(),
  reminder_timezone: z.string().nullable(),
  reminder_dm: z.boolean(),
  start_day: opt(z.number()),
  /** `dm_closed` / `channel_missing` / `no_permission`, see `reminderErrorMessage`. */
  reminder_error: opt(z.string()),
  /** When that failure was recorded, unix seconds. */
  reminder_error_at: opt(z.number()),
});
