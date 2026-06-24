import { describe, expect, it } from 'vitest';

import { clampPan, panLimits } from './zoomClamp';

describe('panLimits', () => {
  it('returns zero limits at zoom 1', () => {
    expect(panLimits(800, 600, 1200, 900, 1)).toEqual({ maxX: 0, maxY: 0 });
  });

  it('uses letterboxed image size for a wide photo in a tall frame', () => {
    // 1600×400 image in 400×600 frame → rendered 400×100; at 2× → 800×200
    const { maxX, maxY } = panLimits(400, 600, 1600, 400, 2);
    expect(maxX).toBe(200);
    expect(maxY).toBe(0);
  });

  it('allows horizontal pan for a wide photo zoomed in', () => {
    // 1600×400 in 800×600 frame → rendered 800×200; at 2× → 1600×400
    const { maxX, maxY } = panLimits(800, 600, 1600, 400, 2);
    expect(maxX).toBe(400);
    expect(maxY).toBe(0);
  });

  it('allows vertical pan for a tall photo zoomed in', () => {
    // 400×1600 in 800×600 frame → rendered 150×600; at 2× → 300×1200
    const { maxX, maxY } = panLimits(800, 600, 400, 1600, 2);
    expect(maxX).toBe(0);
    expect(maxY).toBe(300);
  });

  it('does not use frame size alone when image is letterboxed', () => {
    const letterboxed = panLimits(800, 600, 1600, 400, 2);
    const frameOnly = { maxX: 400, maxY: 150 };
    expect(letterboxed.maxY).toBeLessThan(frameOnly.maxY);
  });
});

describe('clampPan', () => {
  it('clamps offsets to the allowed range', () => {
    const { tx, ty } = clampPan(500, -500, 800, 600, 1600, 400, 2);
    expect(tx).toBe(400);
    expect(Math.abs(ty)).toBe(0);
  });

  it('returns zero offsets when not zoomed', () => {
    const { tx, ty } = clampPan(100, 100, 800, 600, 1600, 400, 1);
    expect(tx).toBe(0);
    expect(ty).toBe(0);
  });
});
