// Field rules shared by the create form and the settings form. They mirror
// leaf-core's `series_ops`, so a creator hears about a problem next to the
// field instead of after a round trip. The server checks everything again and
// stays the judge; these only have to agree with it.

import type { ChannelOption } from '../../types/api';
import { seriesErrorMessage } from '../../utils/labels';

export const NAME_MIN = 2;
export const NAME_MAX = 40;
export const DESCRIPTION_MAX = 200;
/** The highest day number leaf accepts (leaf-core `parser::MAX_DAY`). */
export const MAX_DAY = 999_999;
/** leaf-core `EMOJI_MAX_SCALARS`. */
const EMOJI_MAX_SCALARS = 10;

/** The sentence labels.ts holds for a server code, for a rule checked here. */
export function ruleCopy(code: string): string {
  return seriesErrorMessage(code, 'That value isn’t one leaf can use.');
}

/**
 * Length as the server counts it: Unicode code points. (`String.length` and
 * the `maxlength` attribute count UTF-16 units, so an emoji would count twice.)
 */
export function charCount(text: string): number {
  return Array.from(text).length;
}

/** What is wrong with a series name, as a sentence, or `null` when it is fine. */
export function nameProblem(raw: string): string | null {
  const name = raw.trim();
  if (name === '') return 'Give the series a name.';
  const length = charCount(name);
  if (length < NAME_MIN || length > NAME_MAX) {
    return `A series name needs ${NAME_MIN} to ${NAME_MAX} characters.`;
  }
  if (/\p{Cc}/u.test(name)) return 'A series name has to fit on one line.';
  const folded = name.toLowerCase();
  if (['@everyone', '@here', '<@', '<#'].some((ping) => folded.includes(ping))) {
    return 'A series name can’t contain @everyone, @here or a mention.';
  }
  return null;
}

/** What is wrong with a description, or `null`. It is sent trimmed. */
export function descriptionProblem(raw: string): string | null {
  return charCount(raw.trim()) > DESCRIPTION_MAX ? ruleCopy('invalid_description') : null;
}

export const DAY_NUMBER_PROBLEM = `Enter a whole number from 1 to ${MAX_DAY}.`;

/**
 * A typed day number, or `null` when it is not a whole number in
 * `1..=MAX_DAY`. Full-width digits (a Japanese keyboard) count as digits.
 */
export function parseDayNumber(raw: string): number | null {
  const text = raw.normalize('NFKC').trim();
  if (!/^[0-9]{1,7}$/.test(text)) return null;
  const day = Number(text);
  return day >= 1 && day <= MAX_DAY ? day : null;
}

const ZERO_WIDTH_JOINER = 0x200d;
/** Variation selector 16: "show the previous character as an emoji". */
const EMOJI_PRESENTATION = 0xfe0f;
const COMBINING_KEYCAP = 0x20e3;
/** 🏴, the base of the subdivision flags (England, Scotland, Wales). */
const BLACK_FLAG = 0x1f3f4;
const CANCEL_TAG = 0xe007f;

function between(c: number, low: number, high: number): boolean {
  return c >= low && c <= high;
}

function isRegionalIndicator(c: number): boolean {
  return between(c, 0x1f1e6, 0x1f1ff);
}

function isSkinTone(c: number): boolean {
  return between(c, 0x1f3fb, 0x1f3ff);
}

function isTag(c: number): boolean {
  return between(c, 0xe0030, 0xe0039) || between(c, 0xe0061, 0xe007a);
}

/** `0`-`9`, `#` and `*`: the characters a keycap emoji starts with. */
function isKeycapBase(c: number): boolean {
  return between(c, 0x30, 0x39) || c === 0x23 || c === 0x2a;
}

/** A code point that can be an emoji on its own (the server's block list). */
function isPictograph(c: number): boolean {
  if (isRegionalIndicator(c) || isSkinTone(c)) return false;
  return (
    [0xa9, 0xae, 0x203c, 0x2049, 0x2122, 0x2139, 0x24c2, 0x2934, 0x2935].includes(c) ||
    [0x3030, 0x303d, 0x3297, 0x3299].includes(c) ||
    between(c, 0x2194, 0x21aa) ||
    between(c, 0x2300, 0x23ff) ||
    between(c, 0x25a0, 0x25ff) ||
    between(c, 0x2600, 0x27bf) ||
    between(c, 0x2b00, 0x2b55) ||
    between(c, 0x1f000, 0x1faff)
  );
}

/**
 * Whether `raw` is shaped like exactly one standard emoji: a pictograph (with
 * an optional skin tone, or joined to others), a keycap, a flag or a
 * subdivision flag. The same shape check as leaf-core's `validate_emoji`,
 * because the bot reacts with the whole value; text, two emoji and custom
 * server emoji (`<:name:id>`) are refused.
 */
export function isSingleEmoji(raw: string): boolean {
  const points = Array.from(raw.trim(), (ch) => ch.codePointAt(0) ?? 0);
  const first = points[0];
  if (first === undefined || points.length > EMOJI_MAX_SCALARS) return false;
  let at = 1;
  /** Steps past the next code point when `fits` accepts it. */
  const take = (fits: (c: number) => boolean): boolean => {
    const c = points[at];
    if (c === undefined || !fits(c)) return false;
    at += 1;
    return true;
  };
  const done = (): boolean => at === points.length;

  if (isKeycapBase(first)) {
    take((c) => c === EMOJI_PRESENTATION);
    return take((c) => c === COMBINING_KEYCAP) && done();
  }
  if (isRegionalIndicator(first)) return take(isRegionalIndicator) && done();
  if (!isPictograph(first)) return false;
  if (first === BLACK_FLAG && take(isTag)) {
    while (take(isTag)) {
      // The rest of the tag letters that spell the subdivision.
    }
    return take((c) => c === CANCEL_TAG) && done();
  }
  for (;;) {
    take(isSkinTone);
    take((c) => c === EMOJI_PRESENTATION);
    if (done()) return true;
    if (!take((c) => c === ZERO_WIDTH_JOINER) || !take(isPictograph)) return false;
  }
}

/**
 * A channel as the forms name it. Discord may not have told leaf the name
 * (a channel leaf can no longer see); the id's tail tells two of those apart.
 */
export function channelLabel(channel: ChannelOption): string {
  return channel.name ? `#${channel.name}` : `A channel leaf can’t see (…${channel.id.slice(-4)})`;
}

/** "1 day is" / "3 days are", for the sprout threshold. */
export function daysAre(n: number): string {
  return n === 1 ? '1 day is' : `${n} days are`;
}

/** The fields a server error code can belong to, across both forms. */
export type CodedField =
  | 'name'
  | 'description'
  | 'emoji'
  | 'role'
  | 'channel'
  | 'startDay'
  | 'reminderTime'
  | 'reminderTz';

const CODE_FIELD: Record<string, CodedField> = {
  name_taken: 'name',
  invalid_name: 'name',
  invalid_description: 'description',
  invalid_emoji: 'emoji',
  missing_privacy_role: 'role',
  role_required: 'role',
  unknown_role: 'role',
  invalid_channel: 'channel',
  unknown_channel: 'channel',
  invalid_start_day: 'startDay',
  invalid_reminder_time: 'reminderTime',
  reminder_time_required: 'reminderTime',
  invalid_timezone: 'reminderTz',
  // Not the server's: the settings form's own codes for a save the server
  // answered without applying (settingsForm.ts `unappliedFailure`).
  name_not_saved: 'name',
  start_day_not_saved: 'startDay',
};

/**
 * The field a failed submit's server code is about, or `null` for a failure
 * that belongs to the form as a whole (policy, network, unknown codes).
 */
export function fieldForCode(code: string | null | undefined): CodedField | null {
  if (!code || !Object.prototype.hasOwnProperty.call(CODE_FIELD, code)) return null;
  return CODE_FIELD[code] ?? null;
}
