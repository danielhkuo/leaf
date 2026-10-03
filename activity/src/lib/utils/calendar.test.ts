import { describe, expect, it } from 'vitest';

import type { DaySummary } from '../types/api';
import {
  buildMonths,
  cellLabel,
  lastPostLabel,
  nearestDay,
  weekStartFor,
  weekdayLabels,
  type CalendarGap,
  type CalendarMonth,
  type MonthCell,
} from './calendar';

/** Noon local on the given date (noon avoids zone flips), unix seconds. */
function noon(year: number, month: number, dom: number): number {
  return Math.floor(new Date(year, month, dom, 12).getTime() / 1000);
}

/** A day posted at noon local on the given date, as an older server sends it. */
function day(n: number, year: number, month: number, dom: number): DaySummary {
  return { day: n, posted_at: noon(year, month, dom), thumb_url: `t${n}` };
}

/** "Now" in the middle of June 2024, so June is the current month. */
const JUNE_20 = new Date(2024, 5, 20, 12).getTime();

function months(items: ReturnType<typeof buildMonths>['items']): CalendarMonth[] {
  return items.filter((i): i is CalendarMonth => i.kind === 'month');
}

function cell(month: CalendarMonth | undefined, date: number): MonthCell | undefined {
  return month?.cells[date - 1];
}

describe('buildMonths', () => {
  it('returns nothing for an empty index', () => {
    expect(buildMonths([], { now: JUNE_20 })).toEqual({ items: [], undated: [] });
  });

  it('places days on their real date and marks gaps, outside dates and today', () => {
    const { items } = buildMonths([day(1, 2024, 5, 3), day(2, 2024, 5, 5)], {
      locale: 'en-US',
      now: JUNE_20,
    });
    expect(items).toHaveLength(1);
    const june = months(items)[0];
    expect(june?.label).toBe('June 2024');
    expect(june?.key).toBe('2024-06');
    expect(june?.leading).toBe(6); // June 1 2024 is a Saturday
    expect(june?.rows).toBe(6);
    expect(june?.cells).toHaveLength(30);
    expect(cell(june, 3)).toMatchObject({
      kind: 'archived',
      entries: [{ day: 1, thumbUrl: 't1' }],
      dateLabel: 'Monday, June 3, 2024',
    });
    expect(cell(june, 4)?.kind).toBe('empty'); // a missed date
    expect(cell(june, 2)?.kind).toBe('outside'); // before the first post
    expect(cell(june, 21)?.kind).toBe('outside'); // after today
    expect(cell(june, 20)?.today).toBe(true);
    expect(cell(june, 19)?.today).toBe(false);
  });

  it('keeps every day that shares a date, lowest first', () => {
    const { items } = buildMonths([day(9, 2024, 5, 4), day(5, 2024, 5, 4)], { now: JUNE_20 });
    expect(cell(months(items)[0], 4)?.entries.map((e) => e.day)).toEqual([5, 9]);
  });

  it('groups by the server’s local date rather than the device clock', () => {
    // Posted at 03:00 UTC on 5 June, which was 4 June in Chicago.
    const postedAt = Date.UTC(2024, 5, 5, 3) / 1000;
    const { items } = buildMonths(
      [{ day: 7, posted_at: postedAt, thumb_url: null, local_date: '2024-06-04' }],
      { now: JUNE_20 },
    );
    expect(cell(months(items)[0], 4)?.entries).toEqual([{ day: 7, thumbUrl: null }]);
  });

  it('falls back to the device date when local_date is malformed', () => {
    const { items } = buildMonths([{ ...day(3, 2024, 5, 8), local_date: '2024-02-31' }], {
      now: JUNE_20,
    });
    expect(cell(months(items)[0], 8)?.kind).toBe('archived');
  });

  it('drops the thumbnail of a day whose media is missing', () => {
    const { items } = buildMonths([{ ...day(4, 2024, 5, 8), missing: true }], { now: JUNE_20 });
    expect(cell(months(items)[0], 8)?.entries).toEqual([{ day: 4, thumbUrl: null }]);
  });

  it('collapses runs of empty months into one line', () => {
    const { items } = buildMonths([day(1, 2023, 11, 20), day(2, 2024, 4, 10)], {
      locale: 'en-US',
      now: new Date(2024, 4, 25).getTime(),
    });
    expect(items.map((i) => i.kind)).toEqual(['month', 'gap', 'month']);
    expect((items[1] as CalendarGap).label).toBe('No days archived from January to April 2024');
  });

  it('names a one-month gap and a gap across years', () => {
    const one = buildMonths([day(1, 2024, 0, 5), day(2, 2024, 2, 5)], {
      locale: 'en-US',
      now: new Date(2024, 2, 9).getTime(),
    });
    expect((one.items[1] as CalendarGap).label).toBe('No days archived in February 2024');
    const across = buildMonths([day(1, 2023, 9, 5), day(2, 2024, 2, 5)], {
      locale: 'en-US',
      now: new Date(2024, 2, 9).getTime(),
    });
    expect((across.items[1] as CalendarGap).label).toBe(
      'No days archived from November 2023 to February 2024',
    );
  });

  it('runs up to the current month: a grid for next month, a line after that', () => {
    const recent = buildMonths([day(1, 2024, 4, 30)], { now: JUNE_20 });
    expect(months(recent.items).map((m) => m.key)).toEqual(['2024-05', '2024-06']);
    expect(cell(months(recent.items)[1], 20)?.today).toBe(true);

    const stale = buildMonths([day(1, 2024, 1, 3)], { locale: 'en-US', now: JUNE_20 });
    expect(stale.items.map((i) => i.kind)).toEqual(['month', 'gap']);
    expect((stale.items[1] as CalendarGap).label).toBe('No days archived from March to June 2024');
  });

  it('puts impossible post times in an undated group instead of stretching the calendar', () => {
    const { items, undated } = buildMonths(
      [
        day(3, 2024, 5, 3),
        { day: 2, posted_at: 0, thumb_url: 't2' },
        { day: 1, posted_at: noon(2024, 5, 3) * 1000, thumb_url: null },
      ],
      { now: JUNE_20 },
    );
    expect(months(items).map((m) => m.key)).toEqual(['2024-06']);
    expect(undated.map((e) => e.day)).toEqual([1, 2]);
  });

  it('offsets the first row by the week start', () => {
    // June 1 2024 is a Saturday: six blanks from Sunday, five from Monday.
    const { items } = buildMonths([day(1, 2024, 5, 3)], { weekStart: 1, now: JUNE_20 });
    expect(months(items)[0]?.leading).toBe(5);
  });

  it('takes today from the server timezone when one is given', () => {
    // 20 June 23:30 UTC is already 21 June in Auckland.
    const now = Date.UTC(2024, 5, 20, 23, 30);
    const index = [
      { day: 1, posted_at: noon(2024, 5, 3), thumb_url: null, local_date: '2024-06-03' },
    ];
    const auckland = months(buildMonths(index, { now, timeZone: 'Pacific/Auckland' }).items)[0];
    expect(cell(auckland, 21)?.today).toBe(true);
    const chicago = months(buildMonths(index, { now, timeZone: 'America/Chicago' }).items)[0];
    expect(cell(chicago, 20)?.today).toBe(true);
    // An unknown zone falls back to the device date instead of throwing.
    expect(() => buildMonths(index, { now, timeZone: 'Not/AZone' })).not.toThrow();
  });
});

describe('cellLabel', () => {
  const base: MonthCell = {
    date: 4,
    entries: [{ day: 41, thumbUrl: 'x' }],
    kind: 'archived',
    today: false,
    dateLabel: 'Monday, March 4, 2024',
  };

  it('names the day and the full date', () => {
    expect(cellLabel(base)).toBe('Day 41, Monday, March 4, 2024');
  });

  it('counts the other days on the date, and says today and no preview', () => {
    expect(
      cellLabel({
        ...base,
        today: true,
        entries: [
          { day: 41, thumbUrl: null },
          { day: 42, thumbUrl: 'y' },
          { day: 43, thumbUrl: 'z' },
        ],
      }),
    ).toBe('Day 41 and 2 more, Monday, March 4, 2024, today, no preview');
  });

  it('says no preview when the stored picture did not load', () => {
    expect(cellLabel(base, true)).toBe('Day 41, Monday, March 4, 2024, no preview');
  });

  it('says when the date is unknown', () => {
    expect(cellLabel({ ...base, date: null, dateLabel: '' })).toBe('Day 41, date unknown');
  });
});

describe('nearestDay', () => {
  it('prefers the day itself, then the nearer neighbour, then the later one', () => {
    const days = [1, 2, 5, 9];
    expect(nearestDay(days, 5)).toBe(5);
    expect(nearestDay(days, 4)).toBe(5);
    expect(nearestDay(days, 7)).toBe(9);
    expect(nearestDay(days, 3)).toBe(2);
    expect(nearestDay(days, 500)).toBe(9);
    expect(nearestDay(days, 0)).toBe(1);
    expect(nearestDay([], 3)).toBeNull();
  });
});

describe('lastPostLabel', () => {
  const now = new Date(2024, 5, 20, 9).getTime();

  it('is relative for the last week', () => {
    expect(lastPostLabel(noon(2024, 5, 20), now)).toBe('today');
    expect(lastPostLabel(noon(2024, 5, 19), now)).toBe('yesterday');
    expect(lastPostLabel(noon(2024, 5, 15), now)).toBe('5 days ago');
  });

  it('is a date after that, with the year once it differs', () => {
    expect(lastPostLabel(noon(2024, 2, 14), now, 'en-GB')).toBe('on 14 Mar');
    expect(lastPostLabel(noon(2023, 2, 14), now, 'en-GB')).toBe('on 14 Mar 2023');
  });
});

describe('week start', () => {
  it('reads the locale’s first weekday', () => {
    expect(weekStartFor('en-US')).toBe(0);
    expect(weekStartFor('en-GB')).toBe(1);
    expect(weekStartFor('not a locale')).toBe(0);
  });

  it('labels the columns from that day', () => {
    expect(weekdayLabels('en-US')).toEqual(['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']);
    expect(weekdayLabels('en-GB', 'short', 1)).toEqual([
      'Mon',
      'Tue',
      'Wed',
      'Thu',
      'Fri',
      'Sat',
      'Sun',
    ]);
  });
});
