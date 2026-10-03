// Pure calendar math for the series Home. Each archived day is placed on its
// calendar date: the server's `local_date` (worked out in the server's
// timezone, so every viewer sees the same calendar and it agrees with
// /wrapped), else the device-local date of `posted_at` for an older server.
// Several days can share a date (catch-up posts, freeform series): a cell
// keeps all of them. Runs of months with nothing archived collapse into one
// line, and a post time that cannot be real goes to an "Undated" group
// rather than stretching the calendar back to 1970.

import type { DaySummary } from '../types/api';

const WEEK = 7;
const DAY_MS = 86_400_000;
/** Discord launched in 2015, so no message (and no archived post) is older. */
const EARLIEST_POST_SECS = Date.UTC(2015, 0, 1) / 1000;
/** Clock skew allowed before a post time counts as being in the future. */
const FUTURE_SLACK_MS = 2 * DAY_MS;

/** An archived day as placed in a calendar cell. */
export interface CalendarEntry {
  /** Series day number. */
  day: number;
  /** Signed thumbnail URL; `null` when the day has no stored picture. */
  thumbUrl: string | null;
}

/**
 * - `archived`: one or more days were archived on this date.
 * - `empty`: nothing archived, between the first post and today.
 * - `outside`: before the first post or after today, so nothing was missed.
 */
export type CellKind = 'archived' | 'empty' | 'outside';

/** One date within a month grid. */
export interface MonthCell {
  /** Day of the month, 1..31; `null` for an undated entry. */
  date: number | null;
  /** Every day archived on this date, lowest day number first. */
  entries: CalendarEntry[];
  kind: CellKind;
  /** Whether this is today's date (in the server's timezone when known). */
  today: boolean;
  /** Full date for screen readers ("Tuesday, March 4, 2024"); archived cells only. */
  dateLabel: string;
}

/** A single month laid out for rendering. */
export interface CalendarMonth {
  kind: 'month';
  /** `YYYY-MM`, stable across renders (keyed lists, jump targets). */
  key: string;
  year: number;
  /** 0-11. */
  month: number;
  /** Localized "Month YYYY" heading. */
  label: string;
  /** Blank cells before date 1, counted from the week's first day (0..6). */
  leading: number;
  /** Week rows the grid needs. */
  rows: number;
  /** One cell per real date in the month, in order. */
  cells: MonthCell[];
}

/** A run of months with nothing archived, shown as one line. */
export interface CalendarGap {
  kind: 'gap';
  key: string;
  /** "No days archived from May to September 2024". */
  label: string;
}

export type CalendarItem = CalendarMonth | CalendarGap;

export interface CalendarLayout {
  /** Oldest first. The view reverses it to show the newest month on top. */
  items: CalendarItem[];
  /** Days whose post time is impossible (bad import), lowest day first. */
  undated: CalendarEntry[];
}

export interface CalendarOptions {
  locale?: string | undefined;
  /** First day of the week, 0 = Sunday .. 6 = Saturday (see {@link weekStartFor}). */
  weekStart?: number | undefined;
  /** IANA zone the server computed `local_date` in; decides "today". */
  timeZone?: string | undefined;
  /** The current time, epoch milliseconds. */
  now?: number | undefined;
}

interface Ymd {
  y: number;
  /** 0-11. */
  m: number;
  d: number;
}

function daysInMonth(year: number, month: number): number {
  return new Date(year, month + 1, 0).getDate();
}

function stampOf(at: Ymd): number {
  return at.y * 12 + at.m;
}

/** A sortable number for a date: later dates are larger. */
function ordinal(at: Ymd): number {
  return stampOf(at) * 32 + at.d;
}

const ISO_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

function parseLocalDate(text: string): Ymd | null {
  const match = ISO_DATE.exec(text);
  if (!match) return null;
  const y = Number(match[1]);
  const m = Number(match[2]) - 1;
  const d = Number(match[3]);
  if (m < 0 || m > 11 || d < 1 || d > daysInMonth(y, m)) return null;
  return { y, m, d };
}

function deviceDate(ms: number): Ymd {
  const at = new Date(ms);
  return { y: at.getFullYear(), m: at.getMonth(), d: at.getDate() };
}

/** Today's date in `timeZone`, or on the device when it is missing or unknown. */
function todayIn(nowMs: number, timeZone: string | undefined): Ymd {
  if (timeZone) {
    try {
      const parts = new Intl.DateTimeFormat('en-US', {
        timeZone,
        year: 'numeric',
        month: 'numeric',
        day: 'numeric',
      }).formatToParts(new Date(nowMs));
      const part = (type: string): number =>
        Number(parts.find((p) => p.type === type)?.value ?? Number.NaN);
      const y = part('year');
      const m = part('month') - 1;
      const d = part('day');
      if (Number.isInteger(y) && Number.isInteger(m) && Number.isInteger(d)) return { y, m, d };
    } catch {
      // An unknown zone name: fall back to the device's own date.
    }
  }
  return deviceDate(nowMs);
}

function plausiblePostTime(postedAt: number, nowMs: number): boolean {
  return (
    Number.isFinite(postedAt) &&
    postedAt >= EARLIEST_POST_SECS &&
    postedAt * 1000 <= nowMs + FUTURE_SLACK_MS
  );
}

function monthLabel(year: number, month: number, locale?: string): string {
  return new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric' }).format(
    new Date(year, month, 1),
  );
}

function monthKey(year: number, month: number): string {
  return `${year}-${String(month + 1).padStart(2, '0')}`;
}

function gapLabel(from: number, to: number, locale?: string): string {
  const fromYear = Math.floor(from / 12);
  const toYear = Math.floor(to / 12);
  const toLabel = monthLabel(toYear, to % 12, locale);
  if (from === to) return `No days archived in ${toLabel}`;
  const fromLabel =
    fromYear === toYear
      ? new Intl.DateTimeFormat(locale, { month: 'long' }).format(new Date(fromYear, from % 12, 1))
      : monthLabel(fromYear, from % 12, locale);
  return `No days archived from ${fromLabel} to ${toLabel}`;
}

/**
 * The first day of the week for `locale` (the device's formatting locale by
 * default), 0 = Sunday .. 6 = Saturday. Webviews without `Intl.Locale` week
 * data get Sunday.
 */
export function weekStartFor(locale?: string): number {
  interface WeekInfo {
    firstDay?: number;
  }
  try {
    const tag = locale ?? new Intl.DateTimeFormat().resolvedOptions().locale;
    // `getWeekInfo()` is the current API; older engines expose a getter.
    const loc = new Intl.Locale(tag) as Intl.Locale & {
      getWeekInfo?: () => WeekInfo;
      weekInfo?: WeekInfo;
    };
    const first = (loc.getWeekInfo?.() ?? loc.weekInfo)?.firstDay;
    // Intl numbers the days 1 (Monday) .. 7 (Sunday).
    if (typeof first === 'number' && first >= 1 && first <= 7) return first % WEEK;
  } catch {
    /* no Intl.Locale, or an unusable tag */
  }
  return 0;
}

/** Weekday column labels, starting from `weekStart` (0 = Sunday). */
export function weekdayLabels(
  locale?: string,
  weekday: 'narrow' | 'short' = 'short',
  weekStart = 0,
): string[] {
  const fmt = new Intl.DateTimeFormat(locale, { weekday });
  // 2023-01-01 was a Sunday.
  return Array.from({ length: WEEK }, (_, i) => fmt.format(new Date(2023, 0, 1 + weekStart + i)));
}

/**
 * Lays an index onto month grids. Months with something archived become
 * grids; the months between them become one "nothing archived" line each
 * run. The calendar runs up to the current month: a stale series ends in
 * such a line, and a series last posted to last month gets this month's
 * grid so today is visible. Entries with an impossible post time are
 * returned as `undated`. The index may be in any order.
 */
export function buildMonths(
  index: readonly DaySummary[],
  opts: CalendarOptions = {},
): CalendarLayout {
  const { locale, timeZone } = opts;
  const nowMs = opts.now ?? Date.now();
  const weekStart = (((opts.weekStart ?? 0) % WEEK) + WEEK) % WEEK;

  const byStamp = new Map<number, Map<number, CalendarEntry[]>>();
  const undated: CalendarEntry[] = [];
  let first: Ymd | null = null;
  for (const d of index) {
    const entry: CalendarEntry = { day: d.day, thumbUrl: d.missing ? null : d.thumb_url };
    if (!plausiblePostTime(d.posted_at, nowMs)) {
      undated.push(entry);
      continue;
    }
    const at =
      (d.local_date === undefined ? null : parseLocalDate(d.local_date)) ??
      deviceDate(d.posted_at * 1000);
    let bucket = byStamp.get(stampOf(at));
    if (!bucket) {
      bucket = new Map();
      byStamp.set(stampOf(at), bucket);
    }
    const onDate = bucket.get(at.d);
    if (onDate) onDate.push(entry);
    else bucket.set(at.d, [entry]);
    if (!first || ordinal(at) < ordinal(first)) first = at;
  }
  const byDay = (a: CalendarEntry, b: CalendarEntry): number => a.day - b.day;
  undated.sort(byDay);
  if (!first) return { items: [], undated };

  const today = todayIn(nowMs, timeZone);
  const firstOrdinal = ordinal(first);
  const todayOrdinal = ordinal(today);
  const fullDate = new Intl.DateTimeFormat(locale, {
    weekday: 'long',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });

  const grid = (stamp: number): CalendarMonth => {
    const year = Math.floor(stamp / 12);
    const month = stamp % 12;
    const bucket = byStamp.get(stamp);
    const total = daysInMonth(year, month);
    const cells: MonthCell[] = [];
    for (let date = 1; date <= total; date += 1) {
      const entries = (bucket?.get(date) ?? []).sort(byDay);
      const at = ordinal({ y: year, m: month, d: date });
      let kind: CellKind = 'empty';
      if (entries.length > 0) kind = 'archived';
      else if (at < firstOrdinal || at > todayOrdinal) kind = 'outside';
      cells.push({
        date,
        entries,
        kind,
        today: at === todayOrdinal,
        dateLabel: kind === 'archived' ? fullDate.format(new Date(year, month, date)) : '',
      });
    }
    const leading = (new Date(year, month, 1).getDay() - weekStart + WEEK) % WEEK;
    return {
      kind: 'month',
      key: monthKey(year, month),
      year,
      month,
      label: monthLabel(year, month, locale),
      leading,
      rows: Math.ceil((leading + total) / WEEK),
      cells,
    };
  };
  const gap = (from: number, to: number): CalendarGap => ({
    kind: 'gap',
    key: `gap-${from}-${to}`,
    label: gapLabel(from, to, locale),
  });

  // Only months with something in them are visited, so a gap of any length
  // costs one item: there is no loop over the empty months themselves.
  const stamps = [...byStamp.keys()].sort((a, b) => a - b);
  const items: CalendarItem[] = [];
  let previous: number | null = null;
  for (const stamp of stamps) {
    if (previous !== null && stamp - previous > 1) items.push(gap(previous + 1, stamp - 1));
    items.push(grid(stamp));
    previous = stamp;
  }
  const todayStamp = stampOf(today);
  if (previous !== null && todayStamp === previous + 1) items.push(grid(todayStamp));
  else if (previous !== null && todayStamp > previous + 1)
    items.push(gap(previous + 1, todayStamp));
  return { items, undated };
}

/**
 * The accessible name of an archived cell: "Day 57, Tuesday, March 4, 2024",
 * or "Day 41 and 2 more, …" when several days share the date. Says "no
 * preview" when the cell shows no picture: none was stored, or (with
 * `previewFailed`) the stored one did not load.
 */
export function cellLabel(cell: MonthCell, previewFailed = false): string {
  const [first, ...rest] = cell.entries;
  if (!first) return cell.dateLabel;
  const parts = [
    rest.length === 0 ? `Day ${first.day}` : `Day ${first.day} and ${rest.length} more`,
  ];
  parts.push(cell.dateLabel || 'date unknown');
  if (cell.today) parts.push('today');
  if (first.thumbUrl === null || previewFailed) parts.push('no preview');
  return parts.join(', ');
}

/**
 * The archived day closest to `target`: the day itself when archived, else
 * the nearer neighbour (the later one on a tie). `null` for an empty list.
 */
export function nearestDay(days: readonly number[], target: number): number | null {
  let best: number | null = null;
  for (const d of days) {
    if (best === null) {
      best = d;
      continue;
    }
    const gapNow = Math.abs(d - target);
    const gapBest = Math.abs(best - target);
    if (gapNow < gapBest || (gapNow === gapBest && d > best)) best = d;
  }
  return best;
}

/**
 * When the last post was, for the stats card: "today", "yesterday",
 * "3 days ago", then a date ("on 14 Mar", with the year once it differs).
 * Counted in calendar days on the device.
 */
export function lastPostLabel(postedAt: number, nowMs: number, locale?: string): string {
  const then = deviceDate(postedAt * 1000);
  const now = deviceDate(nowMs);
  const days = Math.round(
    (Date.UTC(now.y, now.m, now.d) - Date.UTC(then.y, then.m, then.d)) / DAY_MS,
  );
  if (days <= 0) return 'today';
  if (days === 1) return 'yesterday';
  if (days < WEEK) return `${days} days ago`;
  const date = new Date(postedAt * 1000).toLocaleDateString(locale, {
    day: 'numeric',
    month: 'short',
    ...(then.y === now.y ? {} : { year: 'numeric' }),
  });
  return `on ${date}`;
}
