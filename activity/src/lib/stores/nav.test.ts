import { beforeEach, describe, expect, it } from 'vitest';

import { nav } from './nav.svelte';

describe('nav back-stack', () => {
  beforeEach(() => {
    nav.reset({ name: 'picker' });
  });

  it('starts at the reset view with no history', () => {
    expect(nav.current).toEqual({ name: 'picker' });
    expect(nav.canGoBack).toBe(false);
  });

  it('pushes and pops views in order', () => {
    nav.push({ name: 'home', seriesId: 7 });
    expect(nav.current).toEqual({ name: 'home', seriesId: 7 });
    expect(nav.canGoBack).toBe(true);

    nav.push({ name: 'viewer', seriesId: 7, day: 3 });
    expect(nav.current).toEqual({ name: 'viewer', seriesId: 7, day: 3 });

    nav.back();
    expect(nav.current).toEqual({ name: 'home', seriesId: 7 });
  });

  it('never pops past the root', () => {
    nav.back();
    nav.back();
    expect(nav.current).toEqual({ name: 'picker' });
    expect(nav.canGoBack).toBe(false);
  });

  it('reset replaces the whole stack', () => {
    nav.push({ name: 'home', seriesId: 1 });
    nav.reset({ name: 'home', seriesId: 2 });
    expect(nav.current).toEqual({ name: 'home', seriesId: 2 });
    expect(nav.canGoBack).toBe(false);
  });
});
