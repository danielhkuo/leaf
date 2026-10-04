// The server settings form's model: what it shows for the saved settings,
// which fields the admin has changed, what is wrong with them and what to
// send. Pure, so the rules are tested without rendering the form.
//
// Only changed fields are checked and sent. A save can then never be blocked
// by a stored value the admin did not touch (a role Discord no longer lists),
// and a panel left open does not write its stale copy over a change made in
// chat with /setup.

import { z } from 'zod';

import { isKnownTimezone } from '../utils/timezones';
import type { AdminSettings, SettingsPatch } from './schemas';

/** The form's fields as the controls hold them. Numbers stay text until sent. */
export interface SettingsValues {
  timezone: string;
  /** `''` lets anyone start a series. */
  creatorRoleId: string;
  /** `''` turns the log off. */
  logChannelId: string;
  maxSeries: string;
  minAccountAge: string;
  minMembershipAge: string;
  sproutEnabled: boolean;
  sproutThreshold: string;
}

/** In the order the fields appear, so the first problem is the topmost. */
export const SETTINGS_FIELDS = [
  'timezone',
  'creatorRoleId',
  'logChannelId',
  'maxSeries',
  'minAccountAge',
  'minMembershipAge',
  'sproutEnabled',
  'sproutThreshold',
] as const;
export type SettingsField = (typeof SETTINGS_FIELDS)[number];

/** The changed fields of a form, as typed: what is kept across a sign-in. */
export type SettingsDraft = Partial<SettingsValues>;

const draftSchema = z
  .object({
    timezone: z.string(),
    creatorRoleId: z.string(),
    logChannelId: z.string(),
    maxSeries: z.string(),
    minAccountAge: z.string(),
    minMembershipAge: z.string(),
    sproutEnabled: z.boolean(),
    sproutThreshold: z.string(),
  })
  .partial();

/** The largest count or number of days the form accepts. */
export const MAX_NUMBER = 999_999;

/** What the form shows for saved settings. Also the baseline edits are measured against. */
export function settingsValues(saved: AdminSettings): SettingsValues {
  return {
    timezone: saved.timezone,
    creatorRoleId: saved.creator_role_id ?? '',
    logChannelId: saved.log_channel_id ?? '',
    maxSeries: String(saved.max_series_per_user),
    minAccountAge: String(saved.min_account_age_days),
    minMembershipAge: String(saved.min_membership_age_days),
    sproutEnabled: saved.sprout_enabled,
    sproutThreshold: String(saved.sprout_threshold),
  };
}

/**
 * A typed whole number, or `null` when it is not one. Full-width digits (a
 * Japanese keyboard) count as digits. The range is not checked here.
 */
export function parseWhole(raw: string): number | null {
  const text = raw.normalize('NFKC').trim();
  if (!/^[0-9]{1,15}$/.test(text)) return null;
  return Number(text);
}

/** What is wrong with a count or a number of days that must be at least `min`. */
function numberProblem(raw: string, min: number, unit: string): string | null {
  const n = parseWhole(raw);
  if (n === null || n < min) return `Enter a whole number${unit}, ${min} or more.`;
  if (n > MAX_NUMBER) return `That’s more than leaf accepts. Enter ${MAX_NUMBER} or less.`;
  return null;
}

/** Discord ids are 17 to 20 digits today; the range leaves room either side. */
export function isSnowflake(id: string): boolean {
  return /^\d{15,22}$/.test(id);
}

/** The ids the pickers list. A typed id outside them has to look like an id. */
export interface KnownIds {
  roles?: readonly string[] | undefined;
  channels?: readonly string[] | undefined;
}

/** The fields whose value differs from what is saved. Empty means nothing to save. */
export function changedFields(saved: AdminSettings, form: SettingsValues): SettingsField[] {
  const changed: SettingsField[] = [];
  // The server stores the canonical spelling, so case alone is not a change.
  if (form.timezone.trim().toLowerCase() !== saved.timezone.toLowerCase()) changed.push('timezone');
  if (form.creatorRoleId.trim() !== (saved.creator_role_id ?? '')) changed.push('creatorRoleId');
  if (form.logChannelId.trim() !== (saved.log_channel_id ?? '')) changed.push('logChannelId');
  if (parseWhole(form.maxSeries) !== saved.max_series_per_user) changed.push('maxSeries');
  if (parseWhole(form.minAccountAge) !== saved.min_account_age_days) changed.push('minAccountAge');
  if (parseWhole(form.minMembershipAge) !== saved.min_membership_age_days) {
    changed.push('minMembershipAge');
  }
  if (form.sproutEnabled !== saved.sprout_enabled) changed.push('sproutEnabled');
  // The threshold only matters (and is only editable) while the stage is on.
  if (form.sproutEnabled && parseWhole(form.sproutThreshold) !== saved.sprout_threshold) {
    changed.push('sproutThreshold');
  }
  return changed;
}

/** One sentence per changed field the server would refuse. */
export function settingsProblems(
  saved: AdminSettings,
  form: SettingsValues,
  known: KnownIds = {},
): Partial<Record<SettingsField, string>> {
  const problems: Partial<Record<SettingsField, string>> = {};
  const put = (field: SettingsField, problem: string | null): void => {
    if (problem !== null) problems[field] = problem;
  };
  for (const field of changedFields(saved, form)) {
    switch (field) {
      case 'timezone': {
        const zone = form.timezone.trim();
        if (zone === '') put(field, 'Choose a timezone.');
        else if (!isKnownTimezone(zone)) {
          put(field, 'leaf doesn’t know that timezone. Use a name such as Europe/Berlin.');
        }
        break;
      }
      case 'creatorRoleId': {
        const id = form.creatorRoleId.trim();
        if (id !== '' && !known.roles?.includes(id) && !isSnowflake(id)) {
          put(field, 'A role ID is a number 17 to 20 digits long. Copy it from Discord again.');
        }
        break;
      }
      case 'logChannelId': {
        const id = form.logChannelId.trim();
        if (id !== '' && !known.channels?.includes(id) && !isSnowflake(id)) {
          put(field, 'A channel ID is a number 17 to 20 digits long. Copy it from Discord again.');
        }
        break;
      }
      case 'maxSeries':
        put(field, numberProblem(form.maxSeries, 1, ''));
        break;
      case 'minAccountAge':
        put(field, numberProblem(form.minAccountAge, 0, ' of days'));
        break;
      case 'minMembershipAge':
        put(field, numberProblem(form.minMembershipAge, 0, ' of days'));
        break;
      case 'sproutThreshold':
        put(field, numberProblem(form.sproutThreshold, 1, ' of days'));
        break;
      case 'sproutEnabled':
        break;
    }
  }
  return problems;
}

/** The PATCH body: the changed fields only. Call it once {@link settingsProblems} is empty. */
export function settingsPatch(saved: AdminSettings, form: SettingsValues): SettingsPatch {
  const patch: SettingsPatch = {};
  const whole = (raw: string): number => parseWhole(raw) ?? 0;
  for (const field of changedFields(saved, form)) {
    switch (field) {
      case 'timezone':
        patch.timezone = form.timezone.trim();
        break;
      case 'creatorRoleId':
        // `""` clears the role: anyone may start a series.
        patch.creator_role_id = form.creatorRoleId.trim();
        break;
      case 'logChannelId':
        patch.log_channel_id = form.logChannelId.trim();
        break;
      case 'maxSeries':
        patch.max_series_per_user = whole(form.maxSeries);
        break;
      case 'minAccountAge':
        patch.min_account_age_days = whole(form.minAccountAge);
        break;
      case 'minMembershipAge':
        patch.min_membership_age_days = whole(form.minMembershipAge);
        break;
      case 'sproutEnabled':
        patch.sprout_enabled = form.sproutEnabled;
        break;
      case 'sproutThreshold':
        patch.sprout_threshold = whole(form.sproutThreshold);
        break;
    }
  }
  return patch;
}

/** The changed fields as typed (valid or not), or `null` when nothing is unsaved. */
export function draftOf(saved: AdminSettings, form: SettingsValues): SettingsDraft | null {
  const changed = changedFields(saved, form);
  if (changed.length === 0) return null;
  const draft: Record<string, string | boolean> = {};
  for (const field of changed) draft[field] = form[field];
  return draft as SettingsDraft;
}

/** Saved settings with a kept draft laid over them: the form after signing in again. */
export function withDraft(saved: AdminSettings, draft: SettingsDraft | null): SettingsValues {
  return { ...settingsValues(saved), ...(draft ?? {}) };
}

/** Checks the shape of a draft read back from storage; `null` when it is unusable or empty. */
export function readDraft(raw: unknown): SettingsDraft | null {
  const parsed = draftSchema.safeParse(raw);
  if (!parsed.success) return null;
  // Drop absent keys so the draft can be spread over saved values.
  const draft = Object.fromEntries(
    Object.entries(parsed.data).filter(([, value]) => value !== undefined),
  ) as SettingsDraft;
  return Object.keys(draft).length > 0 ? draft : null;
}

const CODE_FIELD: Record<string, SettingsField> = {
  invalid_timezone: 'timezone',
  unknown_role: 'creatorRoleId',
  unknown_channel: 'logChannelId',
};

/**
 * The field a refused save's server code is about, or `null` for one that
 * belongs to the form as a whole (`invalid_limit` does not say which number,
 * so its sentence goes next to Save).
 */
export function fieldForCode(code: string | undefined): SettingsField | null {
  if (code === undefined || !Object.prototype.hasOwnProperty.call(CODE_FIELD, code)) return null;
  return CODE_FIELD[code] ?? null;
}

/** "Saved", plus what the save set in motion. */
export function savedText(updated: AdminSettings): string {
  const published = updated.sprouts_published ?? 0;
  if (published === 0) return 'Saved';
  return published === 1
    ? 'Saved. 1 sprout was published.'
    : `Saved. ${published} sprouts were published.`;
}
