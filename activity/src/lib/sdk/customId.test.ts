import { describe, expect, it } from 'vitest';

import { formatCustomId, parseCustomId } from './customId';

describe('parseCustomId', () => {
  it('reads a series, with or without a day', () => {
    expect(parseCustomId('s12')).toEqual({ seriesId: 12, day: null });
    expect(parseCustomId('s12d412')).toEqual({ seriesId: 12, day: 412 });
  });

  it('treats anything else as no target', () => {
    for (const id of [null, undefined, '', 'hello', 's', 's0', 's12d0', 's12d', 's-1', 'S12']) {
      expect(parseCustomId(id)).toBeNull();
    }
    expect(parseCustomId('s12d1000000')).toBeNull();
    expect(parseCustomId(' s12')).toBeNull();
    expect(parseCustomId('s12d4x')).toBeNull();
    expect(parseCustomId('s1234567890123456')).toBeNull();
  });
});

describe('formatCustomId', () => {
  it('round-trips through parseCustomId', () => {
    expect(formatCustomId(12)).toBe('s12');
    expect(formatCustomId(12, 412)).toBe('s12d412');
    expect(parseCustomId(formatCustomId(7, 999_999))).toEqual({ seriesId: 7, day: 999_999 });
  });

  it('links to the series alone when the day is not one leaf archives', () => {
    expect(formatCustomId(12, 0)).toBe('s12');
    expect(formatCustomId(12, 1.5)).toBe('s12');
    expect(formatCustomId(12, 1_000_000)).toBe('s12');
  });

  it('is null for a series id that cannot be one', () => {
    expect(formatCustomId(0)).toBeNull();
    expect(formatCustomId(-3)).toBeNull();
    expect(formatCustomId(Number.NaN)).toBeNull();
  });
});
