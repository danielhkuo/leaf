// Zod schemas mirroring leaf-server's `api::dto` structs. Every API response
// is parsed through these, so a contract drift surfaces as a runtime error at
// the boundary instead of `undefined` deep in a component.

import { z } from 'zod';

export const seriesSchema = z.object({
  id: z.number(),
  name: z.string(),
  description: z.string(),
  creator_id: z.string(),
  cadence: z.string(),
  emoji: z.string(),
  start_day: z.number(),
  max_day: z.number().nullable(),
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

// --- creator series management ---

export const violationSchema = z.object({
  code: z.string(),
  message: z.string(),
});

export const eligibilitySchema = z.object({
  can_create: z.boolean(),
  violations: z.array(violationSchema),
});

export const namedIdSchema = z.object({
  id: z.string(),
  name: z.string(),
});

export const seriesOptionsSchema = z.object({
  channels: z.array(namedIdSchema),
  roles: z.array(namedIdSchema),
  cadences: z.array(z.string()),
  privacy_modes: z.array(z.string()),
  guild_timezone: z.string(),
  sprout_enabled: z.boolean(),
  sprout_threshold: z.number(),
});

export const createdSeriesSchema = z.object({
  id: z.number(),
  name: z.string(),
  state: z.string(),
  emoji: z.string(),
});

export const mySeriesSchema = z.object({
  id: z.number(),
  name: z.string(),
  emoji: z.string(),
  state: z.string(),
  cadence: z.string(),
  channel_id: z.string().nullable(),
  channel_name: z.string().nullable(),
  archived_days: z.number(),
  reminder_enabled: z.boolean(),
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
});
