import { describe, expect, it } from 'vitest';

import { clampIndex, nextDay, prevDay, randomDay } from './navigation';

const DAYS = [1, 2, 5, 9];

describe('nextDay / prevDay', () => {
  it('steps to the adjacent archived day, skipping gaps', () => {
    expect(nextDay(DAYS, 2)).toBe(5);
    expect(nextDay(DAYS, 5)).toBe(9);
    expect(prevDay(DAYS, 9)).toBe(5);
    expect(prevDay(DAYS, 5)).toBe(2);
  });

  it('returns null at the ends', () => {
    expect(nextDay(DAYS, 9)).toBeNull();
    expect(prevDay(DAYS, 1)).toBeNull();
  });

  it('works from a day that is not itself archived', () => {
    expect(nextDay(DAYS, 6)).toBe(9);
    expect(prevDay(DAYS, 6)).toBe(5);
  });
});

describe('randomDay', () => {
  it('never returns the current day', () => {
    const r = randomDay(DAYS, 5, () => 0.99);
    expect(r).not.toBe(5);
    expect(DAYS).toContain(r);
  });

  it('is deterministic given an rng', () => {
    expect(randomDay(DAYS, 1, () => 0)).toBe(2); // first of [2,5,9]
  });

  it('returns null when there is no other day', () => {
    expect(randomDay([7], 7)).toBeNull();
    expect(randomDay([], 1)).toBeNull();
  });
});

describe('clampIndex', () => {
  it('clamps into range and handles empties', () => {
    expect(clampIndex(-1, 3)).toBe(0);
    expect(clampIndex(5, 3)).toBe(2);
    expect(clampIndex(1, 3)).toBe(1);
    expect(clampIndex(0, 0)).toBe(0);
  });
});
