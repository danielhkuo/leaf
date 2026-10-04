// The `custom_id` leaf puts on an activity link and reads back from the
// session to open the gallery on a series or on one of its days:
//
//   s<seriesId>          the series home
//   s<seriesId>d<day>    that day in the viewer
//
// Discord allows 64 characters. Anything else (another app's id, a link
// someone edited) reads as "no target", never as an error.

import type { LaunchIntent } from '../types/api';

/** Fifteen digits: comfortably inside what a JS number holds exactly. */
const MAX_SERIES_ID = 999_999_999_999_999;
/** Highest day number leaf archives (leaf-core `parser::MAX_DAY`). */
const MAX_DAY = 999_999;

const FORMAT = /^s([1-9]\d{0,14})(?:d([1-9]\d{0,5}))?$/;

function inRange(n: number, max: number): boolean {
  return Number.isInteger(n) && n >= 1 && n <= max;
}

/**
 * The series (and day) a launch link asked for, or `null`. The series may
 * not exist or be visible here: check it against the loaded list.
 */
export function parseCustomId(customId: string | null | undefined): LaunchIntent | null {
  const match = customId ? FORMAT.exec(customId) : null;
  if (!match?.[1]) return null;
  return { seriesId: Number(match[1]), day: match[2] ? Number(match[2]) : null };
}

/**
 * The `custom_id` that opens `seriesId`, on `day` when a valid one is given.
 * `null` for a series id that is not a positive integer.
 */
export function formatCustomId(seriesId: number, day: number | null = null): string | null {
  if (!inRange(seriesId, MAX_SERIES_ID)) return null;
  return day !== null && inRange(day, MAX_DAY) ? `s${seriesId}d${day}` : `s${seriesId}`;
}
