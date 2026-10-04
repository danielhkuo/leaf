import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { FULL_SIZE, resizeTo, TILE_SIZE } from '../test/viewport';
import type * as LayoutModule from './layout.svelte';

/** `layout_mode` values of ACTIVITY_LAYOUT_MODE_UPDATE. */
const FOCUSED = 0;
const PIP = 1;
const GRID = 2;

// The store keeps module state (the last size and mode), so each test loads
// a fresh copy.
let store: typeof LayoutModule;
beforeEach(async () => {
  vi.resetModules();
  resizeTo(FULL_SIZE);
  store = await import('./layout.svelte');
});
afterEach(() => resizeTo(FULL_SIZE));

describe('isTile', () => {
  it('calls a viewport a tile when both sides are under 260px, whatever Discord says', () => {
    const { isTile } = store;
    for (const mode of [null, FOCUSED, PIP, GRID, -1]) {
      // Android's tile, a bigger square, a 16:9 window.
      expect(isTile(120, 120, mode)).toBe(true);
      expect(isTile(160, 160, mode)).toBe(true);
      expect(isTile(240, 135, mode)).toBe(true);
      expect(isTile(259, 259, mode)).toBe(true);
    }
  });

  it('does not call a phone, or anything with one long side, a tile', () => {
    const { isTile } = store;
    for (const mode of [null, FOCUSED, GRID]) {
      expect(isTile(260, 260, mode)).toBe(false);
      expect(isTile(259, 260, mode)).toBe(false);
      expect(isTile(260, 259, mode)).toBe(false);
      // The narrowest phones, upright and on their side.
      expect(isTile(320, 568, mode)).toBe(false);
      expect(isTile(568, 320, mode)).toBe(false);
      // A split-screen sliver: narrow, but tall enough to use.
      expect(isTile(200, 700, mode)).toBe(false);
      expect(isTile(1280, 800, mode)).toBe(false);
    }
  });

  it('takes Discord’s word for a somewhat bigger window in picture-in-picture', () => {
    const { isTile } = store;
    expect(isTile(320, 180, PIP)).toBe(true);
    expect(isTile(479, 479, PIP)).toBe(true);
    // The same window with no report, or any other, is not one.
    expect(isTile(320, 180, null)).toBe(false);
    expect(isTile(320, 180, FOCUSED)).toBe(false);
    expect(isTile(320, 180, GRID)).toBe(false);
  });

  it('does not believe picture-in-picture of a full-size viewport', () => {
    // A client that reported the mode and never the return must not leave
    // the gallery stuck behind the tile.
    const { isTile } = store;
    expect(isTile(480, 270, PIP)).toBe(false);
    expect(isTile(375, 667, PIP)).toBe(false);
    expect(isTile(667, 375, PIP)).toBe(false);
    expect(isTile(1280, 800, PIP)).toBe(false);
  });

  it('does not take a viewport with no size for a tile', () => {
    const { isTile } = store;
    expect(isTile(0, 0, null)).toBe(false);
    expect(isTile(0, 120, PIP)).toBe(false);
    expect(isTile(120, 0, null)).toBe(false);
  });
});

describe('layout.tile', () => {
  it('follows the viewport while it is watched, and not after', () => {
    const { layout, watchViewport } = store;
    expect(layout.tile).toBe(false);
    const stop = watchViewport();

    resizeTo(TILE_SIZE);
    flushSync();
    expect(layout.tile).toBe(true);

    resizeTo({ width: 375, height: 667 });
    flushSync();
    expect(layout.tile).toBe(false);

    stop();
    resizeTo(TILE_SIZE);
    flushSync();
    expect(layout.tile).toBe(false);
  });

  it('measures the viewport when the watch starts', () => {
    const { layout, watchViewport } = store;
    // Shrunk without a word, before anything was listening.
    Object.assign(window, { innerWidth: 120, innerHeight: 120 });
    expect(layout.tile).toBe(false);

    const stop = watchViewport();
    flushSync();
    expect(layout.tile).toBe(true);
    stop();
  });

  it('starts from the size the page already has', async () => {
    Object.assign(window, { innerWidth: 160, innerHeight: 160 });
    vi.resetModules();
    const fresh = await import('./layout.svelte');
    expect(fresh.layout.tile).toBe(true);
  });

  it('follows the layout mode Discord reports', () => {
    const { layout, setLayoutMode, watchViewport } = store;
    const stop = watchViewport();
    resizeTo({ width: 320, height: 180 });
    flushSync();
    expect(layout.tile).toBe(false);

    setLayoutMode(PIP);
    flushSync();
    expect(layout.tile).toBe(true);

    setLayoutMode(FOCUSED);
    flushSync();
    expect(layout.tile).toBe(false);
    stop();
  });

  it('forgets picture-in-picture once the viewport has grown out of the tile', () => {
    // A client that never reports the return: afterwards a phone whose
    // webview shrinks for the on-screen keyboard is still a phone.
    const { layout, setLayoutMode, watchViewport } = store;
    const stop = watchViewport();
    setLayoutMode(PIP);
    resizeTo({ width: 320, height: 180 });
    flushSync();
    expect(layout.tile).toBe(true);

    resizeTo({ width: 375, height: 667 });
    flushSync();
    expect(layout.tile).toBe(false);

    resizeTo({ width: 375, height: 300 });
    flushSync();
    expect(layout.tile).toBe(false);
    // Under 260px both ways it is a tile with nothing reported.
    resizeTo({ width: 240, height: 135 });
    flushSync();
    expect(layout.tile).toBe(true);

    // A new report counts again.
    resizeTo({ width: 375, height: 667 });
    setLayoutMode(PIP);
    resizeTo({ width: 320, height: 180 });
    flushSync();
    expect(layout.tile).toBe(true);
    stop();
  });

  it('keeps a report that comes before the window has finished shrinking', () => {
    const { layout, setLayoutMode, watchViewport } = store;
    const stop = watchViewport();
    resizeTo({ width: 375, height: 667 });
    setLayoutMode(PIP);
    // Sizes on the way down are not a return from the tile.
    resizeTo({ width: 375, height: 520 });
    flushSync();
    expect(layout.tile).toBe(false);
    resizeTo({ width: 320, height: 180 });
    flushSync();
    expect(layout.tile).toBe(true);
    stop();
  });
});
