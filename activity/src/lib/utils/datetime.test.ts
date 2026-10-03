import { describe, expect, it } from 'vitest';

import { formatPostedAt, isoInstant } from './datetime';

describe('formatPostedAt', () => {
  it('formats a unix-seconds timestamp into a short date', () => {
    // 2023-11-14T22:13:20Z — a midday-ish time, so the date does not shift
    // across a year boundary in any timezone the test might run in.
    const out = formatPostedAt(1_700_000_000, 'en-US');
    expect(out).toMatch(/Nov 1[45], 2023/);
  });
});

describe('formatPostedAt with a zone', () => {
  it('uses the given zone, not the device’s', () => {
    expect(formatPostedAt(1_700_000_000, 'en-US', 'Asia/Tokyo')).toBe('Nov 15, 2023');
    expect(formatPostedAt(1_700_000_000, 'en-US', 'America/Chicago')).toBe('Nov 14, 2023');
  });

  it('falls back to the device’s zone for a name it does not know', () => {
    expect(formatPostedAt(1_700_000_000, 'en-US', 'Not/AZone')).toMatch(/Nov 1[45], 2023/);
  });

  it('gives nothing for a time that is not one', () => {
    expect(formatPostedAt(Number.NaN, 'en-US')).toBe('');
  });
});

describe('isoInstant', () => {
  it('is the UTC instant, or nothing for a bad time', () => {
    expect(isoInstant(1_700_000_000)).toBe('2023-11-14T22:13:20.000Z');
    expect(isoInstant(Number.POSITIVE_INFINITY)).toBeUndefined();
  });
});
