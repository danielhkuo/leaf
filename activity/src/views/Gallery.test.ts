import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Session } from '../lib/sdk/handshake';
import { nav } from '../lib/stores/nav.svelte';
import { FULL_SIZE, resizeTo, TILE_SIZE } from '../lib/test/viewport';
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
  peekThumb: vi.fn(),
  refreshAll: vi.fn(),
  rememberSeries: vi.fn(),
  takeLaunchIntent: vi.fn(),
  closeActivity: vi.fn(),
  onForeground: vi.fn(),
  home: vi.fn(),
  viewer: vi.fn(),
  picker: vi.fn(),
}));

vi.mock('../lib/stores/gallery.svelte', async () => {
  // Reactive, as the real store's state is: the shell has to notice a
  // session that ends while it is on screen.
  const { reactive } = await import('../lib/test/reactive.svelte');
  mocks.gallery = reactive(mocks.gallery);
  return {
    gallery: mocks.gallery,
    initGallery: mocks.initGallery,
    lastSeries: mocks.lastSeries,
    peekThumb: mocks.peekThumb,
    refreshAll: mocks.refreshAll,
    rememberSeries: mocks.rememberSeries,
    takeLaunchIntent: mocks.takeLaunchIntent,
  };
});
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
    appName: 'leaf',
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
  const layer = container.querySelector('.screens > div') as
    | (HTMLElement & { inert?: boolean })
    | null;
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
    mocks.peekThumb,
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
  mocks.peekThumb.mockReturnValue(null);
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
    // The viewer words "Open original post" for the client it is in.
    expect(lastProps<{ platform: string }>(mocks.viewer).platform).toBe('mobile');

    nav.back();
    expect(nav.current).toEqual({ name: 'home', seriesId: 1 });
    nav.back();
    expect(nav.current).toEqual({ name: 'picker' });
  });

  it('tells the viewer when the client is a desktop one', async () => {
    mocks.takeLaunchIntent.mockResolvedValue({ seriesId: 1, day: 4 });
    render(Gallery, { props: { session: session({ platform: 'desktop' }) } });

    await waitFor(() => expect(nav.current).toEqual({ name: 'viewer', seriesId: 1, day: 4 }));
    expect(lastProps<{ platform: string }>(mocks.viewer).platform).toBe('desktop');
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

// Discord for Android does not bring a running Activity forward, so "Open
// gallery" pressed in chat while leaf is a tile reaches leaf only if leaf
// asks. It asks on a timer, and nothing else in leaf polls: these hold the
// timer to running while the tile is up and the session is good, and not
// otherwise.
describe('Gallery, minimised to a tile', () => {
  /** How often the tile asks (TILE_INTENT_EVERY_MS in Gallery.svelte). */
  const EVERY_MS = 4_000;
  const PHONE = { width: 375, height: 667 };

  afterEach(() => {
    vi.useRealTimers();
    resizeTo(FULL_SIZE);
  });

  const pass = (ms: number): Promise<unknown> => vi.advanceTimersByTimeAsync(ms);
  const asks = (): number => mocks.takeLaunchIntent.mock.calls.length;

  /**
   * The gallery on its first view, with what the boot asked for forgotten.
   * The clock is the test's from before the mount, so a timer started at
   * any point is one it can see and run.
   */
  async function open(): Promise<HTMLElement> {
    vi.useFakeTimers();
    const { container } = render(Gallery, { props: { session: session() } });
    await pass(0);
    expect(mocks.picker).toHaveBeenCalled();
    mocks.takeLaunchIntent.mockClear();
    return container;
  }

  /** The card's words, top to bottom, and its picture. */
  function card(container: HTMLElement): { words: string[]; picture: string | null } {
    const tile = container.querySelector('main.tile');
    return {
      words: [...(tile?.querySelectorAll('h1, p') ?? [])].map((el) => el.textContent ?? ''),
      picture: tile?.querySelector('img')?.getAttribute('src') ?? null,
    };
  }

  it('asks nothing at full size, however long it is open', async () => {
    await open();
    await pass(10 * 60_000);
    expect(asks()).toBe(0);
    // A phone with its keyboard up, or on its side: still not a tile.
    resizeTo({ width: 375, height: 300 });
    await pass(60_000);
    expect(asks()).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('asks every few seconds while it is a tile, and stops when leaf is opened again', async () => {
    await open();
    resizeTo(TILE_SIZE);
    await pass(EVERY_MS - 1);
    expect(asks()).toBe(0);
    await pass(1);
    expect(asks()).toBe(1);
    await pass(2 * EVERY_MS);
    expect(asks()).toBe(3);

    resizeTo(PHONE);
    await pass(10 * EVERY_MS);
    expect(asks()).toBe(3);
    expect(vi.getTimerCount()).toBe(0);

    // Minimised again, it starts again.
    resizeTo(TILE_SIZE);
    await pass(EVERY_MS);
    expect(asks()).toBe(4);
  });

  it('stops asking when the session ends, and does not start without one', async () => {
    const container = await open();
    resizeTo(TILE_SIZE);
    await pass(EVERY_MS);
    expect(asks()).toBe(1);

    mocks.gallery.status = 'expired';
    await pass(10 * EVERY_MS);
    expect(asks()).toBe(1);
    expect(vi.getTimerCount()).toBe(0);
    expect(card(container).words).toEqual(['leaf', 'Session ended']);

    // Opened and minimised again: there is still no session to ask with.
    resizeTo(PHONE);
    resizeTo(TILE_SIZE);
    await pass(10 * EVERY_MS);
    expect(asks()).toBe(1);
  });

  it('does not ask while the gallery has not loaded', async () => {
    mocks.gallery.status = 'error';
    vi.useFakeTimers();
    render(Gallery, { props: { session: session() } });
    await pass(0);
    expect(screen.getByText('Couldn’t load the gallery')).toBeInTheDocument();

    resizeTo(TILE_SIZE);
    await pass(10 * EVERY_MS);
    expect(asks()).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('leaves the first view’s own ask alone while leaf is still opening', async () => {
    let answer: (intent: LaunchIntent | null) => void = () => undefined;
    mocks.takeLaunchIntent.mockReturnValueOnce(new Promise((r) => (answer = r)));
    resizeTo(TILE_SIZE);
    vi.useFakeTimers();
    const { container } = render(Gallery, { props: { session: session() } });
    await pass(10 * EVERY_MS);
    // The one ask is the boot's, and the card says what leaf is doing.
    expect(asks()).toBe(1);
    expect(card(container).words).toEqual(['leaf', 'Opening…']);

    // Opened where the launch asked: that is not a press waiting for a tap.
    answer({ seriesId: 2, day: 7 });
    await pass(0);
    expect(card(container).words).toEqual(['S2', 'Day 7']);
    await pass(EVERY_MS);
    expect(asks()).toBe(2);
  });

  it('does not ask again while an answer is still on its way', async () => {
    await open();
    let answer: (intent: LaunchIntent | null) => void = () => undefined;
    mocks.takeLaunchIntent.mockReturnValueOnce(new Promise((r) => (answer = r)));
    resizeTo(TILE_SIZE);
    await pass(5 * EVERY_MS);
    expect(asks()).toBe(1);

    answer(null);
    await pass(EVERY_MS);
    expect(asks()).toBe(2);
  });

  it('opens what was pressed behind the card, and the card says to tap', async () => {
    const container = await open();
    resizeTo(TILE_SIZE);
    await pass(0);
    // On the list there is nothing to name, and nothing waiting.
    expect(card(container)).toEqual({ words: ['leaf'], picture: null });

    mocks.peekThumb.mockImplementation((id: number, day: number | null) => `thumb-${id}-${day}`);
    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 2, day: 7 });
    await pass(EVERY_MS);
    expect(nav.current).toEqual({ name: 'viewer', seriesId: 2, day: 7 });
    expect(card(container)).toEqual({
      words: ['S2', 'Day 7', 'Tap to open'],
      picture: 'thumb-2-7',
    });
    // Nothing on the card to press: the tap is Discord's.
    expect(container.querySelector('main.tile :is(button, a, [tabindex])')).toBeNull();

    // A second press, on a series this time, replaces the first.
    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 1, day: null });
    await pass(EVERY_MS);
    expect(nav.current).toEqual({ name: 'home', seriesId: 1 });
    expect(card(container).words).toEqual(['S1', 'Day 10', 'Tap to open']);

    // Tapped: leaf is open on it. Minimised again, the same screen is not waiting.
    resizeTo(PHONE);
    await pass(0);
    expect(container.querySelector('main.tile')).toBeNull();
    resizeTo(TILE_SIZE);
    await pass(0);
    expect(card(container).words).toEqual(['S1', 'Day 10']);
  });

  it('says nothing of a press that opened nothing', async () => {
    const container = await open();
    nav.push({ name: 'home', seriesId: 1 });
    resizeTo(TILE_SIZE);
    // A series this person cannot see, before and after a refresh.
    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 99, day: 1 });
    await pass(EVERY_MS);

    expect(mocks.refreshAll).toHaveBeenCalledWith();
    expect(nav.current).toEqual({ name: 'home', seriesId: 1 });
    expect(card(container).words).toEqual(['S1', 'Day 10']);
  });

  it('still asks after a press it failed to follow', async () => {
    const logged = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const container = await open();
    nav.push({ name: 'home', seriesId: 1 });
    resizeTo(TILE_SIZE);
    // A series the list on screen lacks, and the refresh to find it throws.
    const thrown = new Error('the refresh threw');
    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 99, day: 1 });
    mocks.refreshAll.mockRejectedValueOnce(thrown);
    await pass(EVERY_MS);
    expect(asks()).toBe(1);
    // That press is lost, and said so in the log: nothing is waiting.
    expect(logged).toHaveBeenCalledWith('leaf: following a press in chat failed', thrown);
    expect(card(container).words).toEqual(['S1', 'Day 10']);

    // The next one is asked for, and followed.
    mocks.takeLaunchIntent.mockResolvedValueOnce({ seriesId: 2, day: 7 });
    await pass(EVERY_MS);
    expect(asks()).toBe(2);
    expect(card(container).words).toEqual(['S2', 'Day 7', 'Tap to open']);
    logged.mockRestore();
  });
});
