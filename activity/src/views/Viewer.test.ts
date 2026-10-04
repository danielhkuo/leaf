// The viewer's data side: days come through the gallery store (its cache, a
// reload after a refresh), the shell stays up while they do, and closing
// says which day was open.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Day, DaySummary, Series } from '../lib/types/api';
import { ApiError } from '../lib/utils/errors';
import Viewer from './Viewer.svelte';

const mocks = vi.hoisted(() => ({
  getDay: vi.fn(),
  peekDay: vi.fn(),
  loadDaysIndex: vi.fn(),
  setEpoch: vi.fn<(epoch: number) => void>(),
}));

vi.mock('../lib/stores/gallery.svelte', async () => {
  // A real rune, so the viewer's effects see the epoch change.
  const { galleryStub } = await import('../lib/test/galleryStub.svelte');
  mocks.setEpoch.mockImplementation((n) => void (galleryStub.epoch = n));
  return {
    gallery: galleryStub,
    getDay: mocks.getDay,
    peekDay: mocks.peekDay,
    loadDaysIndex: mocks.loadDaysIndex,
  };
});
vi.mock('../lib/sdk/actions', () => ({ openExternalLink: () => Promise.resolve('opened') }));

const SERIES: Series = {
  id: 7,
  name: 'Daily Sketch',
  description: '',
  creator_id: 'u',
  cadence: 'daily',
  emoji: '🍃',
  start_day: 1,
  max_day: 9,
  timezone: 'UTC',
};

function dayOf(day: number, caption = `Caption ${day}`): Day {
  return {
    day,
    caption,
    posted_at: 1_700_000_000 + day * 86_400,
    jump_url: `https://discord.com/channels/g/c/${day}`,
    media: [
      {
        url: `/api/media/${day}`,
        thumb_url: `/api/media/${day}?thumb=1`,
        content_type: 'image/png',
        missing: false,
      },
    ],
  };
}

// Days 6 to 8 are not archived: prev/next skip the gap.
const INDEX: DaySummary[] = [4, 5, 9].map((day) => ({
  day,
  posted_at: 1_700_000_000 + day * 86_400,
  thumb_url: `/api/media/${day}?thumb=1`,
}));

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  let reject: (reason: unknown) => void = () => undefined;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const dialog = (day: number) => screen.findByRole('dialog', { name: `Day ${day}, Daily Sketch` });

beforeEach(() => {
  for (const fn of [mocks.getDay, mocks.peekDay, mocks.loadDaysIndex]) fn.mockReset();
  mocks.setEpoch(0);
  mocks.peekDay.mockReturnValue(null);
  mocks.loadDaysIndex.mockResolvedValue(INDEX);
  mocks.getDay.mockImplementation((_series: number, day: number) => Promise.resolve(dayOf(day)));
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

describe('Viewer', () => {
  it('loads the day through the store and closes with the day on screen', async () => {
    const onClose = vi.fn();
    render(Viewer, { props: { series: SERIES, day: 5, onClose } });
    expect(await screen.findByText('Caption 5')).toBeInTheDocument();
    expect(mocks.getDay).toHaveBeenCalledWith(7, 5, { fresh: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Next day' }));
    expect(await screen.findByText('Caption 9')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalledWith(9);
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenLastCalledWith(9);
  });

  it('keeps its shell while the next day loads, with that day’s preview', async () => {
    render(Viewer, { props: { series: SERIES, day: 5, onClose: vi.fn() } });
    await screen.findByText('Caption 5');

    const slow = deferred<Day>();
    mocks.getDay.mockReturnValueOnce(slow.promise);
    await fireEvent.click(await screen.findByRole('button', { name: 'Previous day' }));

    expect(await dialog(4)).toBeInTheDocument();
    expect(screen.getByRole('status', { name: 'Loading Day 4' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Close' })).toBeInTheDocument();
    expect(screen.queryByText('Caption 5')).toBeNull();
    expect(document.querySelector('img.preview')).toHaveAttribute('src', '/api/media/4?thumb=1');
    // Still steerable: back to Day 5 without waiting for Day 4.
    await fireEvent.click(screen.getByRole('button', { name: 'Next day' }));
    expect(await screen.findByText('Caption 5')).toBeInTheDocument();

    slow.resolve(dayOf(4));
    await Promise.resolve();
    expect(screen.queryByText('Caption 4')).toBeNull();
  });

  it('shows a day the store already has without a loading state', async () => {
    mocks.peekDay.mockImplementation((_series: number, day: number) => dayOf(day));
    render(Viewer, { props: { series: SERIES, day: 5, onClose: vi.fn() } });
    expect(screen.getByText('Caption 5')).toBeInTheDocument();
    expect(mocks.getDay).not.toHaveBeenCalled();
  });

  it('reloads the open day after the gallery refreshes, keeping it on screen meanwhile', async () => {
    render(Viewer, { props: { series: SERIES, day: 5, onClose: vi.fn() } });
    await screen.findByText('Caption 5');
    expect(mocks.loadDaysIndex).toHaveBeenCalledTimes(1);

    const again = deferred<Day>();
    mocks.getDay.mockReturnValueOnce(again.promise);
    mocks.setEpoch(1);
    flushSync();
    await waitFor(() => expect(mocks.getDay).toHaveBeenCalledTimes(2));
    expect(mocks.loadDaysIndex).toHaveBeenCalledTimes(2);
    expect(screen.getByText('Caption 5')).toBeInTheDocument();
    expect(screen.queryByRole('status', { name: 'Loading Day 5' })).toBeNull();

    again.resolve(dayOf(5, 'Edited caption'));
    expect(await screen.findByText('Edited caption')).toBeInTheDocument();
  });

  it('keeps the day when a reload fails for a passing reason, drops it when it is gone', async () => {
    render(Viewer, { props: { series: SERIES, day: 5, onClose: vi.fn() } });
    await screen.findByText('Caption 5');

    mocks.getDay.mockRejectedValueOnce(new ApiError(0, 'GET', undefined, { kind: 'network' }));
    mocks.setEpoch(1);
    flushSync();
    await waitFor(() => expect(mocks.getDay).toHaveBeenCalledTimes(2));
    await Promise.resolve();
    expect(screen.getByText('Caption 5')).toBeInTheDocument();

    mocks.getDay.mockRejectedValueOnce(new ApiError(404, 'GET'));
    mocks.setEpoch(2);
    flushSync();
    expect(await screen.findByText('Couldn’t load this day')).toBeInTheDocument();
    expect(screen.getByText(/This day isn’t in the archive/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('retries a failed load past the cache, inside the same dialog', async () => {
    mocks.getDay.mockRejectedValueOnce(new ApiError(0, 'GET', undefined, { kind: 'network' }));
    render(Viewer, { props: { series: SERIES, day: 5, onClose: vi.fn() } });
    expect(await screen.findByText('Couldn’t load this day')).toBeInTheDocument();
    expect(screen.getByText(/Check your connection/)).toBeInTheDocument();
    expect(await dialog(5)).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByText('Caption 5')).toBeInTheDocument();
    expect(mocks.getDay).toHaveBeenLastCalledWith(7, 5, { fresh: true });
  });

  it('offers Random only when there is another day', async () => {
    mocks.loadDaysIndex.mockResolvedValue(INDEX.slice(1, 2));
    render(Viewer, { props: { series: SERIES, day: 5, onClose: vi.fn() } });
    await screen.findByText('Caption 5');
    expect(screen.queryByRole('button', { name: 'Random' })).toBeNull();
    expect(screen.queryByRole('button', { name: /day$/ })).toBeNull();
  });
});
