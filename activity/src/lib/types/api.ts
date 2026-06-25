// Shared API types, inferred from the zod schemas so there is a single
// source of truth for response shapes (schemas in `lib/api/schemas.ts`).

import type { z } from 'zod';
import type {
  createdSeriesSchema,
  daySchema,
  daySummarySchema,
  eligibilitySchema,
  mediaSchema,
  mySeriesSchema,
  namedIdSchema,
  seriesOptionsSchema,
  seriesSchema,
  seriesSettingsSchema,
  statsSchema,
  violationSchema,
} from '../api/schemas';

export type Series = z.infer<typeof seriesSchema>;
export type Media = z.infer<typeof mediaSchema>;
export type Day = z.infer<typeof daySchema>;
export type DaySummary = z.infer<typeof daySummarySchema>;
export type Stats = z.infer<typeof statsSchema>;

// --- creator series management ---

export type Violation = z.infer<typeof violationSchema>;
export type Eligibility = z.infer<typeof eligibilitySchema>;
export type NamedId = z.infer<typeof namedIdSchema>;
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
  description?: string;
  emoji?: string;
  cadence?: string;
  privacy?: string;
  privacy_role_id?: string | null;
  channel_id?: string;
  detection_mode?: string;
  reminder_enabled?: boolean;
  reminder_time?: string;
  reminder_timezone?: string;
  reminder_dm?: boolean;
}
