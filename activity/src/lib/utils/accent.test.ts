import { describe, expect, it } from 'vitest';

import { accentToken, accentVar } from './accent';

describe('accentToken', () => {
  it('is deterministic for a given id', () => {
    expect(accentToken(42)).toBe(accentToken(42));
  });

  it('assigns distinct colors across the palette', () => {
    const tokens = new Set(Array.from({ length: 7 }, (_, i) => accentToken(i)));
    expect(tokens.size).toBe(7);
  });

  it('wraps large and negative ids into the palette', () => {
    expect(accentToken(7)).toBe(accentToken(0));
    expect(accentToken(-1)).toBe(accentToken(6));
    expect(accentToken(1_000_000)).toMatch(/^--accent-/);
  });
});

describe('accentVar', () => {
  it('wraps the token in a CSS var() reference', () => {
    expect(accentVar(0)).toBe('var(--accent-terraform)');
  });
});
