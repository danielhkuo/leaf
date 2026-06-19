// Pure calendar math for the gallery overview. Each present day carries a real
// post date (`posted_at`, unix seconds), so we lay days onto real month grids
// and the overview reads like a calendar. Gaps are simply absent dates. Local
// time zone, matching the day viewer's date display (see utils/datetime.ts).

import type { DaySummary } from '../types/api';

const WEEK = 7;

/** An archived day as placed in a calendar cell. */
export interface CalendarEntry {
  /** Series day number. */
  day: number;
  /** Signed thumbnail URL, if the day has media. */
  thumbUrl: string | null;
}

/** One date within a month grid; `entry` is set only on archived dates. */
export interface MonthCell {
  /** Day-of-month, 1..31. */
  date: number;
  entry: CalendarEntry | null;
}

/** A single month laid out for rendering. */
export interface CalendarMonth {
  year: number;
  /** 0-11. */
  month: number;
  /** Localized "Month YYYY" heading. */
  label: string;
  /** Blank leading cells before date 1 (Sunday-start, 0..6). */
  leading: number;
  /** One cell per real date in the month, in order. */
  cells: MonthCell[];
}

function monthLabel(year: number, month: number, locale?: string): string {
  return new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric' }).format(
    new Date(year, month, 1),
  );
}

/** Weekday column labels, Sunday-first. (2023-01-01 was a Sunday.) */
export function weekdayLabels(
  locale?: string,
  weekday: 'narrow' | 'short' = 'short',
): string[] {
  const fmt = new Intl.DateTimeFormat(locale, { weekday });
  return Array.from({ length: WEEK }, (_, i) => fmt.format(new Date(2023, 0, 1 + i)));
}

function daysInMonth(year: number, month: number): number {
  return new Date(year, month + 1, 0).getDate();
}

/**
 * Real month grids spanning the earliest to latest archived day, oldest-first.
 * Present days are placed on their actual local date; every other date is an
 * empty cell. Months between the first and last are always included (so the
 * timeline is continuous even across gap-only months). Returns `[]` when empty.
 */
export function buildMonths(index: readonly DaySummary[], locale?: string): CalendarMonth[] {
  if (index.length === 0) return [];

  // Bucket present days by month-stamp (year*12+month), then by day-of-month.
  const byStamp = new Map<number, Map<number, CalendarEntry>>();
  let min = Number.POSITIVE_INFINITY;
  let max = Number.NEGATIVE_INFINITY;
  for (const d of index) {
    const date = new Date(d.posted_at * 1000);
    const stamp = date.getFullYear() * 12 + date.getMonth();
    const dom = date.getDate();
    let bucket = byStamp.get(stamp);
    if (!bucket) {
      bucket = new Map();
      byStamp.set(stamp, bucket);
    }
    // First (lowest day) wins if two days share a date — rare for daily series.
    if (!bucket.has(dom)) bucket.set(dom, { day: d.day, thumbUrl: d.thumb_url });
    if (stamp < min) min = stamp;
    if (stamp > max) max = stamp;
  }

  const months: CalendarMonth[] = [];
  for (let stamp = min; stamp <= max; stamp += 1) {
    const year = Math.floor(stamp / 12);
    const month = ((stamp % 12) + 12) % 12;
    const bucket = byStamp.get(stamp);
    const total = daysInMonth(year, month);
    const cells: MonthCell[] = [];
    for (let date = 1; date <= total; date += 1) {
      cells.push({ date, entry: bucket?.get(date) ?? null });
    }
    months.push({
      year,
      month,
      label: monthLabel(year, month, locale),
      leading: new Date(year, month, 1).getDay(),
      cells,
    });
  }
  return months;
}
