// Zod schemas mirroring leaf-server's `api::admin` DTOs. The admin panel
// validates every response here, same as the gallery client does.
//
// Fields added after the first release go through `opt()`: an older server
// omits them and a newer one may send `null`, and the panel has to work
// against both (it falls back to ids and text boxes).

import { z } from 'zod';

/**
 * An optional field; absent and `null` both read as `undefined`. So does a
 * value of the wrong shape: the panel already copes without these fields, so
 * one that drifted costs a name on screen, not the whole response.
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

export const adminGuildSchema = z.object({
  guild_id: z.string(),
  series_count: z.number(),
  /** The server's name in Discord. */
  name: opt(z.string()),
  icon_url: opt(z.string()),
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
  /** On a PATCH answer: how many sprouts that save published. */
  sprouts_published: opt(z.number()),
});

/**
 * What a settings PATCH answers with: the stored settings, with
 * `sprouts_published` beside the other fields. A server that wraps them as
 * `{settings, sprouts_published}` is read the same way, so a save that went
 * through is never reported as unreadable.
 */
export const adminSettingsAnswerSchema = z.union([
  adminSettingsSchema,
  z
    .object({ settings: adminSettingsSchema, sprouts_published: opt(z.number()) })
    .transform(({ settings, sprouts_published }) => ({
      ...settings,
      sprouts_published: sprouts_published ?? settings.sprouts_published,
    })),
]);

export const adminSeriesSchema = z.object({
  id: z.number(),
  name: z.string(),
  creator_id: z.string(),
  privacy: z.string(),
  privacy_role_id: z.string().nullable(),
  state: z.string(),
  /** The creator's display name in this server; missing once they have left. */
  creator_name: opt(z.string()),
  /** How many days are archived, for a sprout's progress. */
  archived_days: opt(z.number()),
  /** The name of `privacy_role_id`, when Discord still lists that role. */
  privacy_role_name: opt(z.string()),
});

export const adminGuildDetailSchema = z.object({
  guild_id: z.string(),
  settings: adminSettingsSchema,
  series: z.array(adminSeriesSchema),
  name: opt(z.string()),
  icon_url: opt(z.string()),
  /** False until someone has run `/setup` in the server. */
  setup_complete: opt(z.boolean()),
});

const roleChoiceSchema = z.object({ id: z.string(), name: z.string() });
/** `name` is missing for a channel leaf can no longer see. */
const channelChoiceSchema = z.object({ id: z.string(), name: opt(z.string()) });

/**
 * What the pickers offer (`GET /guilds/{gid}/options`). A list the server
 * could not get from Discord is absent or flagged `*_unavailable`; the panel
 * then shows a text box for the id instead.
 */
export const adminOptionsSchema = z.object({
  roles: opt(z.array(roleChoiceSchema)),
  channels: opt(z.array(channelChoiceSchema)),
  roles_unavailable: opt(z.boolean()),
  channels_unavailable: opt(z.boolean()),
});

export type AdminGuild = z.infer<typeof adminGuildSchema>;
export type AdminSettings = z.infer<typeof adminSettingsSchema>;
export type AdminSeries = z.infer<typeof adminSeriesSchema>;
export type AdminGuildDetail = z.infer<typeof adminGuildDetailSchema>;
export type AdminOptions = z.infer<typeof adminOptionsSchema>;
export type RoleChoice = z.infer<typeof roleChoiceSchema>;
export type ChannelChoice = z.infer<typeof channelChoiceSchema>;

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
