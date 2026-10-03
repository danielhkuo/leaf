// Contrast guard for the design tokens. axe's `color-contrast` rule cannot run
// under jsdom (no layout), so the colours themselves are checked here: every
// text colour against every surface it is used on, plus the non-text edges
// that have to be seen (control borders, the focus ring).

import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// Read from disk: Vitest replaces every `.css` import (even `?raw`) with an
// empty module unless CSS processing is switched on for the whole suite.
const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../../app.css'), 'utf8');

type Rgb = readonly [number, number, number];

/** Every `--token: #rrggbb;` declared in app.css. */
const TOKENS = new Map<string, string>(
  [...css.matchAll(/--([a-z0-9-]+):\s*(#[0-9a-f]{6})\s*;/gi)].map((m) => [
    m[1] ?? '',
    (m[2] ?? '').toLowerCase(),
  ]),
);

function token(name: string): Rgb {
  const hex = TOKENS.get(name);
  if (!hex) throw new Error(`--${name} is not a hex colour token in app.css`);
  const channel = (i: number): number => Number.parseInt(hex.slice(i, i + 2), 16);
  return [channel(1), channel(3), channel(5)];
}

/** WCAG 2.x relative luminance of an sRGB colour. */
function luminance([r, g, b]: Rgb): number {
  const linear = (c: number): number => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}

/** WCAG 2.x contrast ratio, 1 to 21. */
function ratio(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

/** Where body copy, labels, links and status text actually sit. */
const SURFACES = ['canvas', 'surface-1', 'surface-2'] as const;
/** Colours used for text at body size or smaller. */
const TEXT = [
  'ink',
  'ink-muted',
  'ink-subtle',
  'link',
  'visited',
  'success',
  'warning',
  'error',
] as const;

const AA_TEXT = 4.5; // WCAG 1.4.3
const AA_NON_TEXT = 3; // WCAG 1.4.11 / 2.4.7

describe('contrast maths', () => {
  it('matches the WCAG reference points', () => {
    expect(ratio([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5);
    expect(ratio([255, 255, 255], [255, 255, 255])).toBe(1);
    // #767676 on white is the canonical "just passes AA" grey.
    expect(ratio([0x76, 0x76, 0x76], [255, 255, 255])).toBeCloseTo(4.54, 2);
  });
});

describe('design token contrast', () => {
  for (const fg of TEXT) {
    for (const bg of SURFACES) {
      it(`--${fg} text on --${bg} meets ${AA_TEXT}:1`, () => {
        expect(ratio(token(fg), token(bg))).toBeGreaterThanOrEqual(AA_TEXT);
      });
    }
  }

  it('primary button label meets 4.5:1 on the call-to-action blue', () => {
    expect(ratio(token('inverse-ink'), token('inverse-canvas'))).toBeGreaterThanOrEqual(AA_TEXT);
  });

  for (const bg of SURFACES) {
    it(`--control-border is a visible edge on --${bg}`, () => {
      expect(ratio(token('control-border'), token(bg))).toBeGreaterThanOrEqual(AA_NON_TEXT);
    });

    it(`--focus-ring is visible on --${bg}`, () => {
      expect(ratio(token('focus-ring'), token(bg))).toBeGreaterThanOrEqual(AA_NON_TEXT);
    });
  }

  it('the focus halo separates from the ring, so one of the two shows on any photo', () => {
    expect(ratio(token('focus-halo'), token('focus-ring'))).toBeGreaterThanOrEqual(AA_NON_TEXT);
  });
});
