import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Session } from '../lib/sdk/handshake';
import { nav } from '../lib/stores/nav.svelte';
import type { LaunchIntent, Series } from '../lib/types/api';
import Gallery from './Gallery.svelte';

const mocks = vi.hoisted(() => ({
  gallery: {
    status: 'ready' as string,
    series: [] as unknown[],
    error: '',
    errorKind: null as string | null,
    eligibility: null,
    eligibilityStatus: 'ready',
    epoch: 0,
    refreshing: false,
  },
  initGallery: vi.fn(),
  lastSeries: vi.fn(),
  refreshAll: vi.fn(),
  rememberSeries: vi.fn(),
  takeLaunchIntent: vi.fn(),
  closeActivity: vi.fn(),
  onForeground: vi.fn(),
  home: vi.fn(),
  viewer: vi.fn(),
  picker: vi.fn(),
}));

vi.mock('../lib/stores/gallery.svelte', () => ({
  gallery: mocks.gallery,
  initGallery: mocks.initGallery,
  lastSeries: mocks.lastSeries,
  refreshAll: mocks.refreshAll,
  rememberSeries: mocks.rememberSeries,
  takeLaunchIntent: mocks.takeLaunchIntent,
}));
vi.mock('../lib/sdk/actions', () => ({
  closeActivity: mocks.closeActivity,
  onForeground: mocks.onForeground,
}));
// The views are stand-ins: this tests the shell around them.
vi.mock('./Home.svelte', () => ({ default: mocks.home }));
vi.mock('./Viewer.svelte', () => ({ default: mocks.viewer }));
vi.mock('./Picker.svelte', () => ({ default: mocks.picker }));
// As after a deploy replaced the chunk, or on a dropped connection.
vi.mock('./creator/entry', () => {
  throw new Error('Failed to fetch dynamically imported module');
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

function session(extra: Partial<Session> = {}): Session {
  return {
    user: { id: 'me', username: 'me' },
    guildId: 'g1',
    channelId: null,
    platform: 'mobile',
    customId: null,
    token: 't',
    expiresAt: Date.now() + 3_600_000,
    ...extra,
  };
}

/**
 * Whether the layer around Home is inert. Svelte sets the property, which a
 * browser reflects to the attribute and jsdom does not.
 */
function homeInert(container: HTMLElement): boolean {
  const layer = container.querySelector('div') as (HTMLElement & { inert?: boolean }) | null;
  return layer?.inert === true;
}

/** The props a stand-in view was last rendered with. */
function lastProps<T>(view: typeof mocks.home): T {
  return view.mock.calls.at(-1)?.[1] as T;
}

beforeEach(() => {
  // jsdom has no scrolling; the shell scrolls to the top on view changes.
  vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined);
  nav.reset({ name: 'picker' });
  Object.assign(mocks.gallery, {
    status: 'ready',
    series: [series(1), series(2, { channel_ids: ['c2'] })],
    error: '',
    errorKind: null,
    refreshing: false,
  });
  for (const fn of [
    mocks.initGallery,
    mocks.lastSeries,
    mocks.refreshAll,
    mocks.rememberSeries,
    mocks.takeLaunchIntent,
    mocks.closeActivity,
    mocks.onForeground,
    mocks.home,
    mocks.viewer,
    mocks.picker,
  ]) {
    fn.mockReset();
  }
  mocks.initGallery.mockResolvedValue(undefined);
  mocks.lastSeries.mockReturnValue(null);
  mocks.refreshAll.mockResolvedValue(true);
  mocks.takeLaunchIntent.mockResolvedValue(null);
  mocks.closeActivity.mockResolvedValue(true);
  mocks.onForeground.mockReturnValue(() => undefined);
});

describe('Gallery', () => {
  it('opens the day a press in chat asked for, with Home inert under the viewer', async () => {
    mocks.takeLaunchIntent.mockResolvedValue({ seriesId: 1, day: 4 });
    const { container } = render(Gallery, { props: { session: session() } });

    await waitFor(() => expect(nav.current).toEqual({ name: 'viewer', seriesId: 1, day: 4 }));
    expect(lastProps<{ day: number }>(mocks.viewer).day).toBe(4);
    expect(homeInert(container)).toBe(true);
    expect(mocks.rememberSeries).toHaveBeenCalledWith(1);

    nav.back();
    expect(nav.current).toEqual({ name: 'home', seriesId: 1 });
    nav.back();
    expect(nav.current).toEqual({ name: 'picker' });
  });

  it('lifts inert when the viewer closes', async () => {
    mocks.takeLaunchIntent.mockResolvedValue({ seriesId: 1, day: 4 });
    const { container } = render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.viewer).toHaveBeenCalled());

    expect(homeInert(container)).toBe(true);
    lastProps<{ onClose: (day?: number) => void }>(mocks.viewer).onClose(6);
    await waitFor(() => expect(homeInert(container)).toBe(false));
    expect(nav.current).toEqual({ name: 'home', seriesId: 1 });
    expect(lastProps<{ reveal: { day: number } | null }>(mocks.home).reveal).toEqual({ day: 6 });
  });

  it('ignores a close button’s click event instead of revealing a day', async () => {
    mocks.takeLaunchIntent.mockResolvedValue({ seriesId: 1, day: 4 });
    render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.viewer).toHaveBeenCalled());

    lastProps<{ onClose: (day?: unknown) => void }>(mocks.viewer).onClose(new MouseEvent('click'));
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 1 }));
    expect(lastProps<{ reveal: { day: number } | null }>(mocks.home).reveal).toBeNull();
  });

  it('keeps the list hidden until the launch intent has answered', async () => {
    let answer: (intent: LaunchIntent | null) => void = () => undefined;
    mocks.takeLaunchIntent.mockReturnValueOnce(new Promise((r) => (answer = r)));
    render(Gallery, { props: { session: session() } });

    await waitFor(() => expect(mocks.takeLaunchIntent).toHaveBeenCalled());
    expect(screen.getByText('Loading the gallery')).toBeInTheDocument();
    expect(mocks.picker).not.toHaveBeenCalled();

    answer({ seriesId: 2, day: null });
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 2 }));
    expect(mocks.picker).not.toHaveBeenCalled();
  });

  it('opens the activity link, then the series of the launch channel', async () => {
    render(Gallery, { props: { session: session({ customId: 's2d3', channelId: 'c2' }) } });
    await waitFor(() => expect(nav.current).toEqual({ name: 'viewer', seriesId: 2, day: 3 }));
  });

  it('opens the one series of the launch channel', async () => {
    render(Gallery, { props: { session: session({ channelId: 'c2' }) } });
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 2 }));
  });

  it('follows a press in chat on return, unless the person moved on', async () => {
    let foreground = (): void => undefined;
    mocks.onForeground.mockImplementation((cb: () => void) => {
      foreground = cb;
      return () => undefined;
    });
    render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.picker).toHaveBeenCalled());

    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 2, day: null });
    foreground();
    expect(mocks.refreshAll).toHaveBeenCalledWith({ ifOlderThanMs: 30_000 });
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 2 }));

    let answer: (intent: LaunchIntent | null) => void = () => undefined;
    mocks.takeLaunchIntent.mockReturnValueOnce(new Promise((r) => (answer = r)));
    foreground();
    nav.push({ name: 'home', seriesId: 1 });
    answer({ seriesId: 1, day: 2 });
    await Promise.resolve();
    await Promise.resolve();
    expect(nav.current).toEqual({ name: 'home', seriesId: 1 });
  });

  it('refreshes once before giving up on a series it does not know', async () => {
    let foreground = (): void => undefined;
    mocks.onForeground.mockImplementation((cb: () => void) => {
      foreground = cb;
      return () => undefined;
    });
    render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.picker).toHaveBeenCalled());

    mocks.refreshAll.mockImplementation(() => {
      mocks.gallery.series = [...mocks.gallery.series, series(3)];
      return Promise.resolve(true);
    });
    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 3, day: 5 });
    foreground();
    await waitFor(() => expect(nav.current).toEqual({ name: 'viewer', seriesId: 3, day: 5 }));
  });

  it('refreshes once for a day newer than the list it has, then opens that day', async () => {
    let foreground = (): void => undefined;
    mocks.onForeground.mockImplementation((cb: () => void) => {
      foreground = cb;
      return () => undefined;
    });
    render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.picker).toHaveBeenCalled());

    // The bot archived Day 11 and launched leaf; the list still says 10.
    mocks.refreshAll.mockImplementation((opts?: { ifOlderThanMs?: number }) => {
      if (opts?.ifOlderThanMs === undefined) {
        mocks.gallery.series = [series(1, { max_day: 11 }), series(2)];
      }
      return Promise.resolve(true);
    });
    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 1, day: 11 });
    foreground();
    await waitFor(() => expect(nav.current).toEqual({ name: 'viewer', seriesId: 1, day: 11 }));
    expect(mocks.refreshAll).toHaveBeenCalledWith();
  });

  it('opens the series home when the day is still unknown after the refresh', async () => {
    let foreground = (): void => undefined;
    mocks.onForeground.mockImplementation((cb: () => void) => {
      foreground = cb;
      return () => undefined;
    });
    render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.picker).toHaveBeenCalled());

    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 1, day: 99 });
    foreground();
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 1 }));
  });

  it('says the session ended and offers to close leaf', async () => {
    mocks.gallery.status = 'expired';
    render(Gallery, { props: { session: session() } });
    expect(screen.getByText('Your session has ended')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Close leaf' }));
    expect(mocks.closeActivity).toHaveBeenCalled();
  });

  it('retries a failed first load in place', async () => {
    mocks.gallery.status = 'error';
    mocks.gallery.errorKind = 'network';
    mocks.gallery.error = 'Can’t reach leaf. Check your connection and try again.';
    render(Gallery, { props: { session: session() } });
    expect(screen.getByText(/Can’t reach leaf/)).toBeInTheDocument();
    expect(screen.queryByText(/GET \//)).not.toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(mocks.initGallery).toHaveBeenCalledTimes(2);
  });

  it('gives an unavailable series a way back', async () => {
    render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.picker).toHaveBeenCalled());
    nav.push({ name: 'home', seriesId: 99 });
    expect(await screen.findByText('This series isn’t available')).toBeInTheDocument();

    // "Check again" says what it found, since the screen itself stays put.
    await fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
    expect(await screen.findByText(/Still not available/)).toBeInTheDocument();
    mocks.refreshAll.mockResolvedValueOnce(false);
    await fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
    expect(await screen.findByText(/Couldn’t check/)).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Back to series' }));
    expect(nav.current).toEqual({ name: 'picker' });
    expect(nav.canGoBack).toBe(false);
  });

  it('gives a failed creator screen Back and one retry, then says to reopen leaf', async () => {
    const logged = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    render(Gallery, { props: { session: session() } });
    await waitFor(() => expect(mocks.picker).toHaveBeenCalled());
    nav.push({ name: 'createSeries' });

    expect(await screen.findByText('Couldn’t open this screen')).toBeInTheDocument();
    // The header's Back, and only that one.
    expect(screen.getAllByRole('button', { name: 'Back' })).toHaveLength(1);
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    expect(await screen.findByText('This screen didn’t load')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Close leaf' }));
    expect(mocks.closeActivity).toHaveBeenCalled();
    // One attempt and one retry: nothing loops on a failed import.
    expect(logged).toHaveBeenCalledTimes(2);
    logged.mockRestore();
  });
});
