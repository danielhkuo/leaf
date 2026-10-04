// The settings form's model: what it shows for a saved series, which fields
// the creator has changed, what is wrong with them and what to send. Pure, so
// the rules are tested without rendering the form.
//
// Only changed fields are sent. A save can then never be blocked by a stored
// value the creator did not touch (a channel dropped from the server's list),
// and a form left open does not write its stale copy over a change made
// elsewhere (an admin's privacy override, the creator's other device).

import type { SeriesSettings, UpdateSeriesInput } from '../../types/api';
import { isKnownTimezone } from '../../utils/timezones';
import {
  DAY_NUMBER_PROBLEM,
  descriptionProblem,
  isSingleEmoji,
  nameProblem,
  parseDayNumber,
  ruleCopy,
} from './formRules';

/** The form's fields as the controls hold them. */
export interface SettingsValues {
  name: string;
  description: string;
  emoji: string;
  cadence: string;
  privacy: string;
  /** `''` when no role is stored. Never defaulted. */
  privacyRoleId: string;
  /** `''` when no channel is stored. Never defaulted. */
  channelId: string;
  /** As typed; `''` when it is not known (see {@link canEditStartDay}). */
  startDay: string;
  reminderEnabled: boolean;
  /** `HH:MM`, or `''` for none. */
  reminderTime: string;
  /** An IANA zone, or `''` for the server's timezone. */
  reminderTz: string;
  reminderDm: boolean;
}

/** In the order the fields appear, so the first problem is the topmost. */
export const SETTINGS_FIELDS = [
  'name',
  'description',
  'emoji',
  'privacy',
  'role',
  'cadence',
  'channel',
  'startDay',
  'reminderEnabled',
  'reminderTime',
  'reminderTz',
  'reminderDm',
] as const;
export type SettingsField = (typeof SETTINGS_FIELDS)[number];

/** The time a reminder gets when it is first switched on: early evening. */
export const DEFAULT_REMINDER_TIME = '20:00';

/** `9:05` → `09:05`, so a native time input can show a value stored unpadded. */
function paddedTime(time: string | null): string {
  const parts = /^(\d{1,2}):(\d{2})$/.exec(time?.trim() ?? '');
  return parts ? `${(parts[1] ?? '').padStart(2, '0')}:${parts[2] ?? ''}` : '';
}

/** What the form shows for a saved series. Also the baseline edits are measured against. */
export function settingsValues(saved: SeriesSettings): SettingsValues {
  return {
    name: saved.name,
    description: saved.description,
    emoji: saved.emoji,
    cadence: saved.cadence,
    privacy: saved.privacy,
    privacyRoleId: saved.privacy_role_id ?? '',
    channelId: saved.channel_id ?? '',
    startDay: saved.start_day === undefined ? '' : String(saved.start_day),
    reminderEnabled: saved.reminder_enabled,
    reminderTime: paddedTime(saved.reminder_time),
    reminderTz: saved.reminder_timezone ?? '',
    reminderDm: saved.reminder_dm,
  };
}

/**
 * Whether the first day number can be edited: only when the form knows what
 * it is now, because a save sends changes and nothing else. The settings
 * response does not have to carry it; the settings view fills it in from the
 * gallery's list (which always does) before the form sees it.
 */
export function canEditStartDay(saved: SeriesSettings): boolean {
  return saved.start_day !== undefined;
}

/** Whether reminders are on as far as the server is concerned: never for freeform. */
export function remindersOn(values: Pick<SettingsValues, 'reminderEnabled' | 'cadence'>): boolean {
  return values.reminderEnabled && values.cadence !== 'freeform';
}

/** Text the server stores trimmed: surrounding whitespace is not a change. */
function sameText(a: string, b: string): boolean {
  return a === b || a.trim() === b.trim();
}

/** The fields whose value differs from what is saved. Empty means nothing to save. */
export function changedFields(saved: SeriesSettings, form: SettingsValues): SettingsField[] {
  const base = settingsValues(saved);
  const changed: SettingsField[] = [];

  if (!sameText(form.name, base.name)) changed.push('name');
  if (!sameText(form.description, base.description)) changed.push('description');
  if (!sameText(form.emoji, base.emoji)) changed.push('emoji');
  if (form.privacy !== base.privacy) changed.push('privacy');
  // The role only matters while the series is role-only, and goes along
  // whenever the series becomes role-only.
  const gated = form.privacy === 'role_gated' && form.privacyRoleId !== '';
  if (gated && (form.privacyRoleId !== base.privacyRoleId || form.privacy !== base.privacy)) {
    changed.push('role');
  }
  if (form.cadence !== base.cadence) changed.push('cadence');
  if (form.channelId !== '' && form.channelId !== base.channelId) changed.push('channel');
  const sameDay =
    form.startDay.trim() === base.startDay || parseDayNumber(form.startDay) === saved.start_day;
  if (canEditStartDay(saved) && !sameDay) changed.push('startDay');

  const on = remindersOn(form);
  if (on !== remindersOn(base)) changed.push('reminderEnabled');
  // The reminder's details only matter (and only show) while it is on.
  if (on) {
    if (form.reminderTime !== base.reminderTime) changed.push('reminderTime');
    if (form.reminderTz.trim().toLowerCase() !== base.reminderTz.toLowerCase()) {
      changed.push('reminderTz');
    }
    if (form.reminderDm !== base.reminderDm) changed.push('reminderDm');
  }
  return changed;
}

/**
 * One sentence per field the server would refuse. Only what is being sent is
 * checked, plus the two rules the server applies to every save: a role-only
 * series needs its role, and a reminder that is on needs its time.
 */
export function settingsProblems(
  saved: SeriesSettings,
  form: SettingsValues,
): Partial<Record<SettingsField, string>> {
  const changed = new Set(changedFields(saved, form));
  const problems: Partial<Record<SettingsField, string>> = {};

  if (changed.has('name')) {
    const name = nameProblem(form.name);
    if (name) problems.name = name;
  }
  if (changed.has('description')) {
    const description = descriptionProblem(form.description);
    if (description) problems.description = description;
  }
  if (changed.has('emoji') && !isSingleEmoji(form.emoji))
    problems.emoji = ruleCopy('invalid_emoji');
  if (form.privacy === 'role_gated' && form.privacyRoleId === '') {
    problems.role = ruleCopy('missing_privacy_role');
  }
  if (changed.has('startDay') && parseDayNumber(form.startDay) === null) {
    problems.startDay = DAY_NUMBER_PROBLEM;
  }
  if (remindersOn(form)) {
    if (form.reminderTime === '') problems.reminderTime = ruleCopy('reminder_time_required');
    else if (paddedTime(form.reminderTime) === '') {
      problems.reminderTime = ruleCopy('invalid_reminder_time');
    }
    const zone = form.reminderTz.trim();
    if (changed.has('reminderTz') && zone !== '' && !isKnownTimezone(zone)) {
      problems.reminderTz = ruleCopy('invalid_timezone');
    }
  }
  return problems;
}

/** The PATCH body: the changed fields only. Call it once {@link settingsProblems} is empty. */
export function settingsPatch(saved: SeriesSettings, form: SettingsValues): UpdateSeriesInput {
  const patch: UpdateSeriesInput = {};
  for (const field of changedFields(saved, form)) {
    switch (field) {
      case 'name':
        patch.name = form.name.trim();
        break;
      case 'description':
        patch.description = form.description.trim();
        break;
      case 'emoji':
        patch.emoji = form.emoji.trim();
        break;
      case 'privacy':
        patch.privacy = form.privacy;
        break;
      case 'role':
        patch.privacy_role_id = form.privacyRoleId;
        break;
      case 'cadence':
        patch.cadence = form.cadence;
        break;
      case 'channel':
        patch.channel_id = form.channelId;
        break;
      case 'startDay': {
        const day = parseDayNumber(form.startDay);
        if (day !== null) patch.start_day = day;
        break;
      }
      case 'reminderEnabled':
        patch.reminder_enabled = remindersOn(form);
        break;
      case 'reminderTime':
        patch.reminder_time = paddedTime(form.reminderTime);
        break;
      case 'reminderTz':
        // `""` clears the override, so the server's timezone applies.
        patch.reminder_timezone = form.reminderTz.trim();
        break;
      case 'reminderDm':
        patch.reminder_dm = form.reminderDm;
        break;
    }
  }
  return patch;
}

/** A save's failure as the settings view holds it: a sentence, and the code that places it. */
export interface SaveFailure {
  code: string;
  message: string;
}

/**
 * What to say when the server answered a save without applying the name or
 * the first day number it was sent: `after` (its answer) still holds what
 * `before` did. A server from before those two could be changed ignores them
 * and answers 200, and "Saved" over a rename that did not happen would be a
 * lie. `null` when everything sent shows in the answer.
 *
 * The codes are this form's own (`fieldForCode` knows them), so the sentence
 * lands under the field like any other refusal.
 */
export function unappliedFailure(
  before: SeriesSettings,
  patch: UpdateSeriesInput,
  after: SeriesSettings,
): SaveFailure | null {
  const name = patch.name !== undefined && patch.name !== before.name && after.name === before.name;
  const startDay =
    patch.start_day !== undefined &&
    patch.start_day !== before.start_day &&
    after.start_day === before.start_day;
  if (!name && !startDay) return null;

  let what = 'The first day number wasn’t changed';
  let cant = 'move it';
  if (name) {
    what = startDay ? 'The name and first day number weren’t changed' : 'The name wasn’t changed';
    cant = startDay ? 'change them' : 'rename a series';
  }
  const others =
    Object.keys(patch).length > Number(name) + Number(startDay)
      ? ' Your other changes were saved.'
      : '';
  return {
    code: name ? 'name_not_saved' : 'start_day_not_saved',
    message: `${what}. This server runs an older version of leaf that can’t ${cant}; ask whoever runs it to update leaf.${others}`,
  };
}

/**
 * The values to fill in when the reminder is switched on for the first time
 * (no time chosen yet): early evening, in the device's timezone. Discord does
 * not tell leaf a person's timezone, and the server's own defaults to UTC, so
 * without this "20:00" would mean 20:00 somewhere else. A reminder that
 * already has a time keeps its time and zone.
 */
export function reminderDefaults(
  form: Pick<SettingsValues, 'reminderTime' | 'reminderTz'>,
  deviceZone: string | null,
): Pick<SettingsValues, 'reminderTime' | 'reminderTz'> {
  if (form.reminderTime !== '')
    return { reminderTime: form.reminderTime, reminderTz: form.reminderTz };
  return {
    reminderTime: DEFAULT_REMINDER_TIME,
    reminderTz: form.reminderTz === '' ? (deviceZone ?? '') : form.reminderTz,
  };
}
