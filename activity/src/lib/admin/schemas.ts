// Zod schemas mirroring leaf-server's `api::admin` DTOs. The admin panel
// validates every response here, same as the gallery client does.

import { z } from 'zod';

export const adminGuildSchema = z.object({
  guild_id: z.string(),
  series_count: z.number(),
});
export const adminGuildListSchema = z.array(adminGuildSchema);

export const adminSettingsSchema = z.object({
  timezone: z.string(),
  creator_role_id: z.string().nullable(),
  log_channel_id: z.string().nullable(),
  max_series_per_user: z.number(),
  min_account_age_days: z.number(),
  min_membership_age_days: z.number(),
  sprout_enabled: z.boolean(),
  sprout_threshold: z.number(),
});

export const adminSeriesSchema = z.object({
  id: z.number(),
  name: z.string(),
  creator_id: z.string(),
  privacy: z.string(),
  privacy_role_id: z.string().nullable(),
  state: z.string(),
});

export const adminGuildDetailSchema = z.object({
  guild_id: z.string(),
  settings: adminSettingsSchema,
  series: z.array(adminSeriesSchema),
});

export type AdminGuild = z.infer<typeof adminGuildSchema>;
export type AdminSettings = z.infer<typeof adminSettingsSchema>;
export type AdminSeries = z.infer<typeof adminSeriesSchema>;
export type AdminGuildDetail = z.infer<typeof adminGuildDetailSchema>;

/** Partial settings update; only present keys change. Empty string clears a
 *  nullable field. */
export type SettingsPatch = Partial<{
  timezone: string;
  creator_role_id: string;
  log_channel_id: string;
  max_series_per_user: number;
  min_account_age_days: number;
  min_membership_age_days: number;
  sprout_enabled: boolean;
  sprout_threshold: number;
}>;

export type SeriesPatch = Partial<{
  privacy: string;
  privacy_role_id: string;
  state: string;
}>;
