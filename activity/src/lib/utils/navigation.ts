// Pure, gap-aware navigation over a series' ordered present-day list, plus
// within-day attachment clamping. `days` is sorted ascending and unique (as
// the days index provides), so "next/prev archived day" skips missing days
// (day 5 → day 9 when 6–8 are missing).

/** The smallest archived day strictly greater than `current`, or null. */
export function nextDay(days: readonly number[], current: number): number | null {
  for (const d of days) {
    if (d > current) return d;
  }
  return null;
}

/** The largest archived day strictly less than `current`, or null. */
export function prevDay(days: readonly number[], current: number): number | null {
  let best: number | null = null;
  for (const d of days) {
    if (d >= current) break;
    best = d;
  }
  return best;
}

/** A random archived day other than `current`; null if there is no other. */
export function randomDay(
  days: readonly number[],
  current: number,
  rng: () => number = Math.random,
): number | null {
  const others = days.filter((d) => d !== current);
  if (others.length === 0) return null;
  const i = Math.min(others.length - 1, Math.floor(rng() * others.length));
  return others[i] ?? null;
}

/** Clamps an attachment index into `[0, count-1]` (0 when there are none). */
export function clampIndex(index: number, count: number): number {
  if (count <= 0) return 0;
  return Math.max(0, Math.min(index, count - 1));
}
