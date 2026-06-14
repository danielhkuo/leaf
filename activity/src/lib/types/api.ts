// Shared API types, inferred from the zod schemas so there is a single
// source of truth for response shapes (schemas in `lib/api/schemas.ts`).

import type { z } from 'zod';
import type {
  daySchema,
  daySummarySchema,
  mediaSchema,
  seriesSchema,
  statsSchema,
} from '../api/schemas';

export type Series = z.infer<typeof seriesSchema>;
export type Media = z.infer<typeof mediaSchema>;
export type Day = z.infer<typeof daySchema>;
export type DaySummary = z.infer<typeof daySummarySchema>;
export type Stats = z.infer<typeof statsSchema>;
