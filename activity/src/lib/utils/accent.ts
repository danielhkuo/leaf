// Per-series accent colors. Each series gets one identity color from the
// design system's product palette (see DESIGN.md and the UI/UX plan),
// derived deterministically from its id so it is stable without a schema
// column. This is the spec's "per-product accent" device mapped to series.

const PALETTE = [
  '--accent-indigo',
  '--accent-green',
  '--accent-cyan',
  '--accent-orange',
  '--accent-pink',
  '--accent-violet',
  '--accent-lime',
] as const;

/** The CSS custom-property name for a series' accent. */
export function accentToken(seriesId: number): string {
  const n = PALETTE.length;
  const i = ((Math.trunc(seriesId) % n) + n) % n;
  return PALETTE[i] ?? '--accent-green';
}

/** A `var(--accent-…)` reference for a series' accent. */
export function accentVar(seriesId: number): string {
  return `var(${accentToken(seriesId)})`;
}
