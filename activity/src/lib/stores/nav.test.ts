import { beforeEach, describe, expect, it } from 'vitest';

import type { Series } from '../types/api';
import { nav, resolveTarget, stackFor, startTarget, type StartHints } from './nav.svelte';

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

  it('reset replaces the whole stack, bottom first', () => {
    nav.push({ name: 'home', seriesId: 1 });
    nav.reset({ name: 'home', seriesId: 2 });
    expect(nav.current).toEqual({ name: 'home', seriesId: 2 });
    expect(nav.canGoBack).toBe(false);

    nav.reset(
      { name: 'picker' },
      { name: 'home', seriesId: 3 },
      { name: 'viewer', seriesId: 3, day: 9 },
    );
    expect(nav.current).toEqual({ name: 'viewer', seriesId: 3, day: 9 });
    nav.back();
    nav.back();
    expect(nav.current).toEqual({ name: 'picker' });
  });

  it('gives an equal view pushed again a new key', () => {
    nav.push({ name: 'viewer', seriesId: 1, day: 2 });
    const first = nav.key;
    nav.reset({ name: 'picker' }, { name: 'viewer', seriesId: 1, day: 2 });
    expect(nav.key).not.toBe(first);
  });

  it('remembers the scroll of a covered view for the way back only', () => {
    window.scrollY = 640;
    nav.push({ name: 'home', seriesId: 1 });
    expect(nav.lastMove).toBe('push');
    expect(nav.savedScroll).toBe(0);
    window.scrollY = 0;
    nav.back();
    expect(nav.lastMove).toBe('back');
    expect(nav.savedScroll).toBe(640);
  });

  it('counts every move', () => {
    const before = nav.version;
    nav.push({ name: 'mySeries' });
    nav.back();
    nav.back(); // nothing to pop: not a move
    expect(nav.version).toBe(before + 2);
  });
});

function series(id: number, extra: Partial<Series> = {}): Series {
  return {
    id,
    name: `S${id}`,
    description: '',
    creator_id: 'u',
    cadence: 'daily',
    emoji: '🍃',
    start_day: 1,
    max_day: 10,
    ...extra,
  };
}

const NONE: StartHints = { intent: null, link: null, channelId: null, remembered: null };

describe('startTarget', () => {
  const list = [
    series(1, { channel_ids: ['c1'] }),
    series(2, { channel_ids: ['c2'] }),
    series(3, { channel_ids: ['c2'] }),
    series(4, { state: 'revoked', channel_ids: ['c4'] }),
  ];

  it('opens nothing in particular without a hint and with several series', () => {
    expect(startTarget(list, NONE)).toBeNull();
  });

  it('follows a press in chat first, then the activity link', () => {
    const intent = { seriesId: 2, day: 5 };
    const link = { seriesId: 1, day: null };
    expect(startTarget(list, { ...NONE, intent, link, remembered: 3 })).toEqual({
      seriesId: 2,
      day: 5,
    });
    expect(startTarget(list, { ...NONE, link, remembered: 3 })).toEqual({ seriesId: 1, day: null });
  });

  it('skips a hint for a series that is missing or revoked', () => {
    const intent = { seriesId: 99, day: null };
    const link = { seriesId: 4, day: null };
    expect(startTarget(list, { ...NONE, intent, link, remembered: 3 })).toEqual({
      seriesId: 3,
      day: null,
    });
  });

  it('opens the one series in the launch channel, not when there are several', () => {
    expect(startTarget(list, { ...NONE, channelId: 'c1', remembered: 3 })).toEqual({
      seriesId: 1,
      day: null,
    });
    expect(startTarget(list, { ...NONE, channelId: 'c2', remembered: 3 })).toEqual({
      seriesId: 3,
      day: null,
    });
    expect(startTarget(list, { ...NONE, channelId: 'c4' })).toBeNull();
  });

  it('opens the only series, unless it is revoked', () => {
    expect(startTarget([series(5)], NONE)).toEqual({ seriesId: 5, day: null });
    expect(startTarget([series(4, { state: 'revoked' })], NONE)).toBeNull();
  });
});

describe('resolveTarget', () => {
  it('opens the home for a day past the newest one', () => {
    const list = [series(1, { max_day: 10 }), series(2, { max_day: null })];
    expect(resolveTarget(list, { seriesId: 1, day: 10 })).toEqual({ seriesId: 1, day: 10 });
    expect(resolveTarget(list, { seriesId: 1, day: 11 })).toEqual({ seriesId: 1, day: null });
    expect(resolveTarget(list, { seriesId: 2, day: 1 })).toEqual({ seriesId: 2, day: null });
  });
});

describe('stackFor', () => {
  it('keeps the picker at the bottom', () => {
    expect(stackFor(null)).toEqual([{ name: 'picker' }]);
    expect(stackFor({ seriesId: 1, day: null })).toEqual([
      { name: 'picker' },
      { name: 'home', seriesId: 1 },
    ]);
    expect(stackFor({ seriesId: 1, day: 4 })).toEqual([
      { name: 'picker' },
      { name: 'home', seriesId: 1 },
      { name: 'viewer', seriesId: 1, day: 4 },
    ]);
  });
});
