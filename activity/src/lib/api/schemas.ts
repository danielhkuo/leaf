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
