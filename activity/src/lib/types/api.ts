// Shared API types, inferred from the zod schemas so there is a single
// source of truth for response shapes (schemas in `lib/api/schemas.ts`).
//
// These are the parsed (output) shapes: a field the server may omit or send
// as `null` is `T | undefined` here, never `null`.

import type { z } from 'zod';
import type {
  channelOptionSchema,
  createdSeriesSchema,
  daySchema,
  daySummarySchema,
  eligibilitySchema,
  mediaSchema,
  mySeriesSchema,
  namedIdSchema,
  roleOptionSchema,
  seriesOptionsSchema,
  seriesSchema,
  seriesSettingsSchema,
  sproutSchema,
  statsSchema,
  violationParamsSchema,
  violationSchema,
} from '../api/schemas';

export type Series = z.infer<typeof seriesSchema>;
export type SproutProgress = z.infer<typeof sproutSchema>;
export type Media = z.infer<typeof mediaSchema>;
export type Day = z.infer<typeof daySchema>;
export type DaySummary = z.infer<typeof daySummarySchema>;
export type Stats = z.infer<typeof statsSchema>;

/** Where a button in chat asked the gallery to open (already validated shape). */
export interface LaunchIntent {
  seriesId: number;
  /** `null` opens the series home rather than a day. */
  day: number | null;
}

// --- creator series management ---

export type ViolationParams = z.infer<typeof violationParamsSchema>;
export type Violation = z.infer<typeof violationSchema>;
export type Eligibility = z.infer<typeof eligibilitySchema>;
export type NamedId = z.infer<typeof namedIdSchema>;
export type ChannelOption = z.infer<typeof channelOptionSchema>;
export type RoleOption = z.infer<typeof roleOptionSchema>;
export type SeriesOptions = z.infer<typeof seriesOptionsSchema>;
export type CreatedSeries = z.infer<typeof createdSeriesSchema>;
export type MySeries = z.infer<typeof mySeriesSchema>;
export type SeriesSettings = z.infer<typeof seriesSettingsSchema>;

/** Body for `POST /series` — mirrors the server's `CreateSeriesRequest`. */
export interface CreateSeriesInput {
  name: string;
  description?: string;
  channel_id: string;
  cadence: string;
  privacy: string;
  privacy_role_id?: string | null;
  start_day?: number;
}

/** Body for `PATCH /series/{id}` — every field optional (partial update). */
export interface UpdateSeriesInput {
  name?: string;
  description?: string;
  emoji?: string;
  cadence?: string;
  privacy?: string;
  privacy_role_id?: string | null;
  channel_id?: string;
  /** Ignored by current servers (passive capture was removed); do not send. */
  detection_mode?: string;
  start_day?: number;
  reminder_enabled?: boolean;
  reminder_time?: string;
  /** An IANA zone; `""` clears the override so the server timezone applies. */
  reminder_timezone?: string;
  reminder_dm?: boolean;
}
