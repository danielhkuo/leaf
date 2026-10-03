// Human-friendly labels for the domain enum strings and error codes the API
// speaks. Keeping them here means the picker, the create form, the settings
// form and every error state phrase a rule the same way.

import type { Violation, ViolationParams } from '../types/api';

export function cadenceLabel(cadence: string): string {
  switch (cadence) {
    case 'daily':
      return 'Daily';
    case 'weekdays':
      return 'Weekdays';
    case 'weekly':
      return 'Weekly';
    case 'freeform':
      return 'Freeform (no schedule)';
    default:
      return cadence;
  }
}

export function privacyLabel(privacy: string): string {
  switch (privacy) {
    case 'public':
      return 'Everyone in the server';
    case 'role_gated':
      return 'Only members with a role';
    case 'creator_only':
      return 'Only me';
    default:
      return privacy;
  }
}

/** "1 day" / "30 days", for the age rules. */
function dayCount(n: number): string {
  return n === 1 ? '1 day' : `${n} days`;
}

/**
 * "later today" / "on 14 Oct" for a unix time still ahead of `nowMs`, in the
 * device's locale and zone. `null` once the time has passed, so the caller
 * falls back to a sentence without a date.
 */
function whenEligible(unix: number, nowMs: number): string | null {
  const at = new Date(unix * 1000);
  const now = new Date(nowMs);
  if (Number.isNaN(at.getTime()) || at.getTime() <= nowMs) return null;
  if (at.toDateString() === now.toDateString()) return 'later today';
  const sameYear = at.getFullYear() === now.getFullYear();
  const date = at.toLocaleDateString(undefined, {
    day: 'numeric',
    month: 'short',
    ...(sameYear ? {} : { year: 'numeric' }),
  });
  return `on ${date}`;
}

type Copy = string | ((p: ViolationParams, nowMs: number) => string);

// One sentence per server code (leaf-server `policy_code`, `validation_code`
// and the coded errors in section 5.5 of docs/ux-audit.md). Each says what is
// in the way and what to do next. The same entry serves the eligibility
// check, which carries params, and a failed submit, which does not.
const COPY: Record<string, Copy> = {
  max_series: ({ limit, current }) => {
    const ask = 'Ask a server admin if you need another.';
    if (limit === undefined)
      return `You’ve reached this server’s limit of series per member. ${ask}`;
    if (current === undefined) {
      return `You’ve reached this server’s limit of ${limit} series per member. ${ask}`;
    }
    return `You have ${current} of ${limit} series here, which is this server’s limit. ${ask}`;
  },
  account_too_new: ({ eligible_at, days }, nowMs) => {
    const when = eligible_at === undefined ? null : whenEligible(eligible_at, nowMs);
    if (when)
      return `Your Discord account is too new to start a series here. You can start one ${when}.`;
    if (days === undefined) return 'Your Discord account is too new to start a series here yet.';
    return `Your Discord account needs to be at least ${dayCount(days)} old to start a series here.`;
  },
  membership_too_new: ({ eligible_at, days }, nowMs) => {
    const when = eligible_at === undefined ? null : whenEligible(eligible_at, nowMs);
    if (when)
      return `You joined this server too recently to start a series. You can start one ${when}.`;
    if (days === undefined)
      return 'You haven’t been in this server long enough to start a series yet.';
    return `You need to have been in this server for at least ${dayCount(days)} to start a series.`;
  },
  missing_creator_role: ({ role_name }) =>
    role_name
      ? `Starting a series here needs the @${role_name} role. Ask a server admin for it.`
      : 'Starting a series here needs a role you don’t have. Ask a server admin for it.',
  guild_not_setup:
    'leaf isn’t set up in this server yet. A server admin needs to run /setup in chat first.',
  invalid_channel:
    'That channel isn’t one this server allows for series. Pick another, or ask a server admin to add it with /setup.',
  unknown_channel: 'leaf can’t find that channel. Pick another one.',
  unknown_role: 'leaf can’t find that role. Pick another one.',
  invalid_name:
    'Give the series a name of 2 to 40 characters on one line, without @everyone, @here or a mention.',
  invalid_description: 'Keep the description to 200 characters or fewer.',
  invalid_emoji: 'Use a single standard emoji for the reaction, such as 🍃.',
  invalid_start_day:
    'The first day number must be between 1 and 999999, and no higher than the earliest day already archived.',
  missing_privacy_role: 'Choose the role that can view this series.',
  role_required: 'Choose the role that can view this series.',
  invalid_reminder_time: 'Enter the reminder time as 24-hour HH:MM, for example 17:30.',
  invalid_timezone: 'leaf doesn’t know that timezone. Pick one from the list.',
  reminder_time_required: 'Choose a reminder time before turning reminders on.',
  reminder_on_freeform:
    'Freeform series have no schedule to remind against. Choose a daily, weekdays or weekly cadence first.',
  name_taken:
    'That name is already used in this server, possibly by a private or revoked series you can’t see. Choose a different name.',
  revoked:
    'A server admin revoked this series, so it can’t be changed. Ask an admin to restore it.',
  discord_unavailable: 'leaf can’t reach Discord right now. Try again in a moment.',
  // Sign-in: Discord refused the one-time code, and a session can't ask for
  // another, so there is nothing to retry.
  code_rejected: 'Discord didn’t accept the sign-in. Close leaf and open it again.',
};

/**
 * The sentence for a server error code, or `fallback` when the code is
 * missing or not one leaf has copy for. `params` fills in the specifics the
 * eligibility check provides (limit, role name, date).
 */
export function seriesErrorMessage(
  code: string | undefined,
  fallback: string,
  params: ViolationParams = {},
  nowMs: number = Date.now(),
): string {
  // Own keys only, so a code like "constructor" can't reach the prototype.
  // (`Object.hasOwn` is missing from the iOS 15.0-15.3 webview.)
  const known = code !== undefined && Object.prototype.hasOwnProperty.call(COPY, code);
  const copy = known ? COPY[code] : undefined;
  if (copy === undefined) return fallback;
  return typeof copy === 'string' ? copy : copy(params, nowMs);
}

/**
 * Why the viewer can't start a series, as one actionable sentence. Codes this
 * build doesn't know fall back to the server's own sentence.
 */
export function violationMessage(v: Violation, nowMs: number = Date.now()): string {
  const fallback = v.message || 'You can’t start a series here yet. Ask a server admin why.';
  return seriesErrorMessage(v.code, fallback, v.params ?? {}, nowMs);
}

/**
 * What to do about a reminder leaf could not deliver. `reason` is the
 * settings DTO's `reminder_error` (`dm_closed`, `channel_missing`,
 * `no_permission`); anything else gets a general line.
 */
export function reminderErrorMessage(reason: string): string {
  switch (reason) {
    case 'dm_closed':
      return 'Discord wouldn’t let leaf send you a DM. Allow direct messages from this server in its Privacy Settings, or switch the reminder to a channel ping.';
    case 'channel_missing':
      return 'The channel this series reminds in is gone or was never set. Pick a channel for the series, or switch the reminder to a DM.';
    case 'no_permission':
      return 'leaf isn’t allowed to post in the channel this series reminds in. Ask a server admin to let leaf send messages there, or switch the reminder to a DM.';
    default:
      return 'leaf couldn’t deliver the last reminder. Check the reminder settings and save them again.';
  }
}
