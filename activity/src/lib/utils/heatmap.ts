// Pure layout math for the day-number heatmap. The archive is a sequence of
// numbered days (not calendar dates — see leaf-core `stats`), so the grid is
// timezone-free: day `d` sits at column `(d-1) % 7`, and blocks are aligned
// runs of day numbers. Cell *presence* is layered on top by the component.

export const WEEK = 7;
/** Weeks per block ("month") — 4 weeks = 28 day cells. */
export const BLOCK_WEEKS = 4;
export const BLOCK_DAYS = WEEK * BLOCK_WEEKS;
/** Blocks per year label — 13 × 28 = 364 ≈ a year. */
export const YEAR_BLOCKS = 13;

/** A contiguous run of day numbers laid out as a 4×7 grid. */
export interface Block {
  /** 0-based block index. */
  index: number;
  /** First day number in the block (inclusive). */
  fromDay: number;
  /** Last day number in the block (inclusive). */
  toDay: number;
  /** 1-based year grouping, for separators. */
  year: number;
}

/**
 * The blocks covering days `1..maxDay`, always at least one. Blocks are
 * week-aligned (each starts on a column-0 day) since `BLOCK_DAYS` is a
 * multiple of `WEEK`.
 */
export function monthBlocks(maxDay: number): Block[] {
  const days = Math.max(0, Math.trunc(maxDay));
  const count = Math.max(1, Math.ceil(days / BLOCK_DAYS));
  const blocks: Block[] = [];
  for (let i = 0; i < count; i += 1) {
    blocks.push({
      index: i,
      fromDay: i * BLOCK_DAYS + 1,
      toDay: (i + 1) * BLOCK_DAYS,
      year: Math.floor(i / YEAR_BLOCKS) + 1,
    });
  }
  return blocks;
}

export type CellState = 'present' | 'missing' | 'pre-start' | 'future';

/** One grid cell: a day number and its state. */
export interface CellInfo {
  day: number;
  state: CellState;
}

/** Everything cell classification needs beyond the day number. */
export interface GridContext {
  maxDay: number;
  startDay: number;
  present: ReadonlySet<number>;
}

/** Classifies a single day for rendering. */
export function cellState(day: number, ctx: GridContext): CellState {
  if (day > ctx.maxDay) return 'future';
  if (ctx.present.has(day)) return 'present';
  if (day < ctx.startDay) return 'pre-start';
  return 'missing';
}

/** The block's day cells in row-major (7-per-row) order, classified. */
export function blockGrid(block: Block, ctx: GridContext): CellInfo[] {
  const cells: CellInfo[] = [];
  for (let day = block.fromDay; day <= block.toDay; day += 1) {
    cells.push({ day, state: cellState(day, ctx) });
  }
  return cells;
}
