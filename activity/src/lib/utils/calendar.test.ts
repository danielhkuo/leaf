import { describe, expect, it } from 'vitest';

import type { DaySummary } from '../types/api';
import { buildMonths, weekdayLabels } from './calendar';

/** A present day posted at noon local on the given date (noon avoids tz flips). */
function day(n: number, year: number, month: number, dom: number): DaySummary {
  return {
    day: n,
    posted_at: Math.floor(new Date(year, month, dom, 12).getTime() / 1000),
    thumb_url: `t${n}`,
  };
}

describe('buildMonths', () => {
  it('returns nothing for an empty index', () => {
    expect(buildMonths([])).toEqual([]);
  });

  it('places present days on their real date and leaves gaps empty', () => {
    const months = buildMonths([day(1, 2024, 5, 3), day(2, 2024, 5, 5)], 'en-US');
    expect(months).toHaveLength(1);
    const june = months[0]!;
    expect(june.label).toBe('June 2024');
    expect(june.month).toBe(5);
    expect(june.leading).toBe(6); // June 1 2024 is a Saturday
    expect(june.cells).toHaveLength(30);
    expect(june.cells[2]).toEqual({ date: 3, entry: { day: 1, thumbUrl: 't1' } });
    expect(june.cells[4]).toEqual({ date: 5, entry: { day: 2, thumbUrl: 't2' } });
    expect(june.cells[3]?.entry).toBeNull(); // June 4 is a gap
  });

  it('spans every month between first and last, including gap-only months', () => {
    const months = buildMonths([day(1, 2023, 11, 20), day(2, 2024, 1, 10)], 'en-US');
    expect(months.map((m) => m.label)).toEqual(['December 2023', 'January 2024', 'February 2024']);
    expect(months[1]?.cells.every((c) => c.entry === null)).toBe(true);
  });

  it('keeps the lowest day when two share a date', () => {
    const months = buildMonths([day(5, 2024, 2, 4), day(9, 2024, 2, 4)], 'en-US');
    expect(months[0]?.cells[3]?.entry?.day).toBe(5);
  });
});

describe('weekdayLabels', () => {
  it('is seven labels, Sunday-first', () => {
    const labels = weekdayLabels('en-US');
    expect(labels).toHaveLength(7);
    expect(labels[0]).toBe('Sun');
    expect(labels[6]).toBe('Sat');
  });
});
