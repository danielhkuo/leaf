import { describe, expect, it } from 'vitest';

import { BLOCK_DAYS, blockGrid, cellState, type GridContext, monthBlocks } from './heatmap';

describe('monthBlocks', () => {
  it('always yields at least one block, even for an empty series', () => {
    const blocks = monthBlocks(0);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ index: 0, fromDay: 1, toDay: BLOCK_DAYS, year: 1 });
  });

  it('adds a block each time the day count crosses a boundary', () => {
    expect(monthBlocks(BLOCK_DAYS)).toHaveLength(1);
    expect(monthBlocks(BLOCK_DAYS + 1)).toHaveLength(2);
    expect(monthBlocks(2 * BLOCK_DAYS)).toHaveLength(2);
  });

  it('covers contiguous, week-aligned day ranges', () => {
    const blocks = monthBlocks(2 * BLOCK_DAYS);
    expect(blocks[0]).toMatchObject({ fromDay: 1, toDay: BLOCK_DAYS });
    expect(blocks[1]).toMatchObject({ fromDay: BLOCK_DAYS + 1, toDay: 2 * BLOCK_DAYS });
  });

  it('groups blocks into years for separators', () => {
    const blocks = monthBlocks(20 * BLOCK_DAYS);
    expect(blocks[0]?.year).toBe(1);
    expect(blocks[12]?.year).toBe(1);
    expect(blocks[13]?.year).toBe(2);
  });
});

describe('cellState', () => {
  const ctx = (over: Partial<GridContext> = {}): GridContext => ({
    maxDay: 30,
    startDay: 1,
    present: new Set([2, 3]),
    ...over,
  });

  it('marks days beyond max as future', () => {
    expect(cellState(31, ctx())).toBe('future');
  });

  it('marks archived days present and gaps missing', () => {
    expect(cellState(2, ctx())).toBe('present');
    expect(cellState(5, ctx())).toBe('missing');
  });

  it('distinguishes pre-start, present, and missing within an offset window', () => {
    const c = ctx({ maxDay: 110, startDay: 100, present: new Set([100]) });
    expect(cellState(99, c)).toBe('pre-start');
    expect(cellState(100, c)).toBe('present');
    expect(cellState(105, c)).toBe('missing');
  });
});

describe('blockGrid', () => {
  it('produces one classified cell per day in order', () => {
    const [block] = monthBlocks(BLOCK_DAYS);
    const cells = blockGrid(block!, { maxDay: 10, startDay: 1, present: new Set([1]) });
    expect(cells).toHaveLength(BLOCK_DAYS);
    expect(cells[0]).toEqual({ day: 1, state: 'present' });
    expect(cells[1]).toEqual({ day: 2, state: 'missing' });
    expect(cells[10]).toEqual({ day: 11, state: 'future' });
  });
});
