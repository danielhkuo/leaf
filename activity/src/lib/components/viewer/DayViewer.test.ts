import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';

import type { Day } from '../../types/api';
import DayViewer from './DayViewer.svelte';

const IMG = {
  url: '/api/media/a',
  thumb_url: '/api/media/a?thumb',
  content_type: 'image/png',
  missing: false,
};
const IMG_B = { ...IMG, url: '/api/media/b', thumb_url: '/api/media/b?thumb' };
const VIDEO = { ...IMG, url: '/api/media/v', content_type: 'video/mp4' };

const DAY: Day = {
  day: 5,
  caption: 'A nice day',
  posted_at: 1_700_000_000,
  jump_url: 'https://discord.com/channels/g/c/m',
  media: [IMG],
};

function props(over: Partial<Record<string, unknown>> = {}) {
  return {
    dayNumber: 5,
    day: DAY as Day | null,
    seriesName: 'Daily Johan',
    hasPrev: true,
    hasNext: true,
    onPrev: vi.fn(),
    onNext: vi.fn(),
    onRandom: vi.fn(),
    onClose: vi.fn(),
    openLink: vi.fn().mockResolvedValue('opened'),
    ...over,
  };
}

const photo = () => screen.getByRole('img', { name: 'A nice day' });

describe('DayViewer', () => {
  it('renders the day number, caption, and the full-res image', () => {
    render(DayViewer, { props: props() });
    expect(screen.getByText('Day 5')).toBeInTheDocument();
    expect(screen.getByText('A nice day')).toBeInTheDocument();
    expect(photo()).toHaveAttribute('src', '/api/media/a');
  });

  it('gives the date in the server’s zone, machine-readable too', () => {
    const { container } = render(DayViewer, { props: props({ timeZone: 'Asia/Tokyo' }) });
    const time = container.querySelector('time');
    // 22:13 UTC on the 14th is already the 15th in Tokyo.
    expect(time?.textContent).toMatch(/15/);
    expect(time).toHaveAttribute('datetime', '2023-11-14T22:13:20.000Z');
  });

  it('maps arrow keys and escape to the right callbacks', async () => {
    const p = props();
    render(DayViewer, { props: p });
    await fireEvent.keyDown(window, { key: 'ArrowRight' });
    await fireEvent.keyDown(window, { key: 'ArrowLeft' });
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(p.onNext).toHaveBeenCalledOnce();
    expect(p.onPrev).toHaveBeenCalledOnce();
    expect(p.onClose).toHaveBeenCalledOnce();
  });

  it('leaves the keys alone while it is put away behind the tile', async () => {
    const p = props();
    const { container } = render(DayViewer, { props: p });
    // As Minimisable marks the screens while leaf is a tile.
    container.setAttribute('inert', '');
    await fireEvent.keyDown(window, { key: 'ArrowRight' });
    await fireEvent.keyDown(window, { key: 'ArrowLeft' });
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(p.onNext).not.toHaveBeenCalled();
    expect(p.onPrev).not.toHaveBeenCalled();
    expect(p.onClose).not.toHaveBeenCalled();

    container.removeAttribute('inert');
    await fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(p.onNext).toHaveBeenCalledOnce();
  });

  it('does not navigate past the ends', async () => {
    const p = props({ hasNext: false });
    render(DayViewer, { props: p });
    await fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(p.onNext).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: /^Next/ })).toBeNull();
  });

  it('leaves the arrow keys to a video that has focus', async () => {
    const p = props({ day: { ...DAY, media: [VIDEO] } });
    const { container } = render(DayViewer, { props: p });
    const video = container.querySelector('video');
    expect(video).toHaveAttribute('preload', 'metadata');
    await fireEvent.keyDown(video!, { key: 'ArrowRight' });
    expect(p.onNext).not.toHaveBeenCalled();
    await fireEvent.keyDown(video!, { key: 'Escape' });
    expect(p.onClose).toHaveBeenCalledOnce();
  });

  it('has one Close, and it does not pass its click event on', async () => {
    const p = props();
    render(DayViewer, { props: p });
    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(p.onClose).toHaveBeenCalledOnce();
    expect(p.onClose).toHaveBeenCalledWith();
  });

  it('releases the full-res image on unmount', () => {
    const { unmount } = render(DayViewer, { props: props() });
    expect(screen.queryByRole('img', { name: 'A nice day' })).toBeInTheDocument();
    unmount();
    expect(screen.queryByRole('img', { name: 'A nice day' })).not.toBeInTheDocument();
  });

  it('keeps exactly one full-res image across day navigation (no accumulation)', async () => {
    const { container, rerender } = render(DayViewer, { props: props() });
    expect(container.querySelectorAll('img.full')).toHaveLength(1);

    // Navigate through several days; the full-res <img> is reused, never piled
    // up — the structural guarantee behind "memory flat over a 50-day browse".
    for (let day = 6; day <= 10; day += 1) {
      await rerender(
        props({
          dayNumber: day,
          day: { ...DAY, day, media: [{ ...IMG, url: `/api/media/${day}` }] },
        }),
      );
      expect(container.querySelectorAll('img.full')).toHaveLength(1);
    }
  });

  it('orders header, stage, and footer for the grid layout', () => {
    const { container } = render(DayViewer, { props: props() });
    const viewer = container.querySelector('.viewer');
    expect(viewer).not.toBeNull();
    const children = viewer!.children;
    expect(children[0]?.classList.contains('top')).toBe(true);
    expect(children[1]?.classList.contains('stage')).toBe(true);
    expect(children[2]?.classList.contains('bottom')).toBe(true);
    expect(viewer!.querySelector('.stage .frame, .stage img.full')).not.toBeNull();
  });
});

describe('DayViewer with several files', () => {
  const multi: Day = { ...DAY, media: [IMG, IMG_B] };

  it('switches files via the dots and says which one is showing', async () => {
    render(DayViewer, { props: props({ day: multi }) });

    const dots = screen.getAllByRole('button', { name: /^Photo \d of 2$/ });
    expect(dots).toHaveLength(2);
    expect(photo()).toHaveAttribute('src', '/api/media/a');
    expect(screen.getByText('1 / 2')).toBeInTheDocument();

    await fireEvent.click(dots[1]!);
    expect(photo()).toHaveAttribute('src', '/api/media/b');
    expect(screen.getByText('2 / 2')).toBeInTheDocument();
    expect(dots[1]).toHaveAttribute('aria-current', 'true');
    expect(dots[0]).not.toHaveAttribute('aria-current');
  });

  it('shows no dots or count for a single file', () => {
    render(DayViewer, { props: props() });
    expect(screen.queryByRole('button', { name: /^Photo/ })).toBeNull();
    expect(screen.queryByText('1 / 1')).toBeNull();
  });

  it('pages through the day’s files before it changes day', async () => {
    const p = props({ day: multi });
    render(DayViewer, { props: p });

    // On the first photo: forward is the next photo, back is the day before.
    expect(screen.getByRole('button', { name: 'Next photo' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Previous day' })).toBeInTheDocument();
    await fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(p.onNext).not.toHaveBeenCalled();
    expect(photo()).toHaveAttribute('src', '/api/media/b');

    // On the last: back is the photo before, forward is the next day.
    await fireEvent.click(screen.getByRole('button', { name: 'Previous photo' }));
    expect(p.onPrev).not.toHaveBeenCalled();
    expect(photo()).toHaveAttribute('src', '/api/media/a');
    await fireEvent.click(screen.getByRole('button', { name: 'Next photo' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Next day' }));
    expect(p.onNext).toHaveBeenCalledOnce();
  });

  it('offers the other files of the only day, with no day to go to', async () => {
    render(DayViewer, { props: props({ day: multi, hasPrev: false, hasNext: false }) });
    expect(screen.queryByRole('button', { name: /^Previous/ })).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Next photo' }));
    expect(screen.queryByRole('button', { name: /^Next/ })).toBeNull();
    expect(screen.getByRole('button', { name: 'Previous photo' })).toBeInTheDocument();
  });

  it('lands on the last file of a day entered backwards, the first going forwards', async () => {
    const p = props({ day: { ...DAY, day: 6, media: [IMG] }, dayNumber: 6 });
    const { rerender } = render(DayViewer, { props: p });

    await fireEvent.keyDown(window, { key: 'ArrowLeft' });
    expect(p.onPrev).toHaveBeenCalledOnce();
    await rerender({ ...p, dayNumber: 5, day: multi });
    expect(photo()).toHaveAttribute('src', '/api/media/b');

    await fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(p.onNext).toHaveBeenCalledOnce();
    await rerender({ ...p, dayNumber: 7, day: { ...multi, day: 7 } });
    expect(photo()).toHaveAttribute('src', '/api/media/a');
  });

  it('calls a mixed day’s items files', () => {
    render(DayViewer, { props: props({ day: { ...DAY, media: [IMG, VIDEO] } }) });
    expect(screen.getByRole('button', { name: 'Next file' })).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /^File \d of 2$/ })).toHaveLength(2);
  });
});

describe('DayViewer while a day loads or fails', () => {
  it('keeps the shell: Close, the arrows and the date from the calendar', async () => {
    const p = props({
      day: null,
      summary: { day: 5, posted_at: 1_700_000_000, thumb_url: '/api/media/a?thumb' },
      timeZone: 'UTC',
    });
    const { container } = render(DayViewer, { props: p });

    expect(screen.getByRole('dialog', { name: 'Day 5, Daily Johan' })).toBeInTheDocument();
    expect(screen.getByRole('status', { name: 'Loading Day 5' })).toBeInTheDocument();
    expect(container.querySelector('time')?.textContent).toMatch(/Nov 14, 2023|14 Nov 2023/);
    expect(container.querySelector('img.preview')).toHaveAttribute('src', '/api/media/a?thumb');
    expect(screen.getByRole('button', { name: 'Open original post' })).toBeDisabled();

    await fireEvent.click(screen.getByRole('button', { name: 'Next day' }));
    expect(p.onNext).toHaveBeenCalledOnce();
    await fireEvent.keyDown(window, { key: 'Escape' });
    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(p.onClose).toHaveBeenCalledTimes(2);
  });

  it('says why a day did not load and offers to try again when that can help', async () => {
    const onRetry = vi.fn();
    const { rerender } = render(DayViewer, {
      props: props({ day: null, error: 'Can’t reach leaf.', onRetry }),
    });
    expect(screen.getByRole('alert')).toHaveTextContent('Couldn’t load this day');
    expect(screen.getByRole('alert')).toHaveTextContent('Can’t reach leaf.');
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(onRetry).toHaveBeenCalledOnce();
    expect(screen.getByRole('button', { name: 'Previous day' })).toBeInTheDocument();

    await rerender(props({ day: null, error: 'It may have been removed.', onRetry: undefined }));
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('offers a photo that failed again, renewing the day first', async () => {
    const onRetry = vi.fn();
    const { container } = render(DayViewer, { props: props({ onRetry }) });
    await fireEvent.error(container.querySelector('img.full')!);
    expect(screen.getByRole('alert')).toHaveTextContent('This photo didn’t load.');

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(onRetry).toHaveBeenCalledOnce();
    expect(container.querySelector('img.full')).toHaveAttribute('src', '/api/media/a');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('says so when a video didn’t play, and points at the post', async () => {
    const { container } = render(DayViewer, {
      props: props({ day: { ...DAY, media: [VIDEO] } }),
    });
    await fireEvent.error(container.querySelector('video')!);
    expect(screen.getByRole('alert')).toHaveTextContent('This video didn’t play');
    expect(container.querySelector('video')).toBeNull();
    expect(screen.getByRole('button', { name: 'Open original post' })).toBeEnabled();
    // Nothing can renew the day here, so there is nothing to offer.
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('offers a video that failed again, renewing the day first', async () => {
    const onRetry = vi.fn();
    const { container } = render(DayViewer, {
      props: props({ day: { ...DAY, media: [VIDEO] }, onRetry }),
    });
    const first = container.querySelector('video')!;
    await fireEvent.error(first);
    expect(screen.getByRole('alert')).toHaveTextContent('Try again, or use “Open original post”');

    const again = screen.getByRole('button', { name: 'Try again' });
    again.focus();
    await fireEvent.click(again);
    expect(onRetry).toHaveBeenCalledOnce();
    expect(screen.queryByRole('alert')).toBeNull();
    // A new element for the same address, and focus stays in the dialog.
    const second = container.querySelector('video');
    expect(second).toHaveAttribute('src', '/api/media/v');
    expect(second).not.toBe(first);
    expect(document.activeElement).toBe(screen.getByRole('dialog'));

    // It can fail again, and be offered again.
    await fireEvent.error(second!);
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
  });

  it('points at the original post when no file was saved', () => {
    render(DayViewer, {
      props: props({ day: { ...DAY, media: [{ ...IMG, url: '', missing: true }] } }),
    });
    expect(screen.getByText('No file was saved for this day')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Open original post' })).toBeEnabled();
  });
});

describe('DayViewer caption and actions', () => {
  it('shows Discord markup as words and keeps line breaks', () => {
    const day = { ...DAY, caption: 'first <:leaf:123>\nsecond <@42>' };
    const { container } = render(DayViewer, { props: props({ day }) });
    expect(container.querySelector('.caption')?.textContent?.trim()).toBe(
      'first :leaf:\nsecond @member',
    );
  });

  it('offers More only for a caption that is cut off, and toggles it', async () => {
    const heights = vi
      .spyOn(HTMLElement.prototype, 'scrollHeight', 'get')
      .mockImplementation(function (this: HTMLElement) {
        return this.classList.contains('caption') ? 200 : 0;
      });
    try {
      const { container } = render(DayViewer, { props: props() });
      const more = await screen.findByRole('button', { name: 'More' });
      expect(more).toHaveAttribute('aria-expanded', 'false');

      await fireEvent.click(more);
      const less = screen.getByRole('button', { name: 'Less' });
      expect(less).toHaveAttribute('aria-expanded', 'true');
      expect(container.querySelector('.caption')).toHaveClass('expanded');
      expect(container.querySelector('.caption')).toHaveAttribute('tabindex', '0');

      await fireEvent.click(less);
      expect(container.querySelector('.caption')).not.toHaveClass('expanded');
    } finally {
      heights.mockRestore();
    }
  });

  it('measures a resized caption in the next frame, not inside the observer', async () => {
    // The observers the viewer makes, and the frames it asks for.
    const observers: ResizeObserverCallback[] = [];
    const frames = new Map<number, FrameRequestCallback>();
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(told: ResizeObserverCallback) {
          observers.push(told);
        }
        observe = vi.fn();
        unobserve = vi.fn();
        disconnect = vi.fn();
      },
    );
    vi.stubGlobal('requestAnimationFrame', (run: FrameRequestCallback) => {
      frames.set(frames.size + 1, run);
      return frames.size;
    });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
    let captionHeight = 0;
    const heights = vi
      .spyOn(HTMLElement.prototype, 'scrollHeight', 'get')
      .mockImplementation(function (this: HTMLElement) {
        return this.classList.contains('caption') ? captionHeight : 0;
      });
    const resized = (): void => {
      for (const told of observers) told([], {} as ResizeObserver);
    };
    try {
      render(DayViewer, { props: props() });
      await tick();
      expect(screen.queryByRole('button', { name: 'More' })).toBeNull();

      // The viewer got narrower and the caption no longer fits. Told twice
      // before a frame: one measurement is waiting, not two.
      captionHeight = 200;
      resized();
      resized();
      await tick();
      // Nothing on the page has changed size while observers are being told.
      expect(screen.queryByRole('button', { name: 'More' })).toBeNull();
      expect(frames.size).toBe(1);

      for (const run of frames.values()) run(0);
      await tick();
      expect(screen.getByRole('button', { name: 'More' })).toBeInTheDocument();
    } finally {
      heights.mockRestore();
      vi.unstubAllGlobals();
    }
  });

  it('has no More for a caption that fits', () => {
    render(DayViewer, { props: props() });
    expect(screen.queryByRole('button', { name: 'More' })).toBeNull();
  });

  /**
   * The line that says what became of "Open original post". A photo that is
   * still loading is a status too, so this one is found by its class.
   */
  function linkStatus(): HTMLElement {
    const found = screen.getAllByRole('status').filter((el) => el.matches('.link-status'));
    expect(found).toHaveLength(1);
    return found[0]!;
  }
  const openPost = () =>
    fireEvent.click(screen.getByRole('button', { name: 'Open original post' }));

  it('hands the post’s address to Discord, with a status line waiting for the answer', async () => {
    const p = props();
    render(DayViewer, { props: p });
    // In the page before there is anything to say, so the text is announced.
    expect(linkStatus()).toBeEmptyDOMElement();

    await openPost();
    expect(p.openLink).toHaveBeenCalledOnce();
    expect(p.openLink).toHaveBeenCalledWith(DAY.jump_url);
  });

  it('on a phone says the post is open behind leaf, and how to get to it', async () => {
    render(DayViewer, { props: props({ platform: 'mobile' }) });
    await openPost();

    await waitFor(() =>
      expect(linkStatus()).toHaveTextContent(
        'Opened in the channel, behind leaf. Minimise leaf to see it: tap the arrow at the top left, or press Back on Android.',
      ),
    );
    // It opened: the link itself is not needed.
    expect(screen.queryByText(DAY.jump_url)).toBeNull();
  });

  it('on desktop says only that it opened', async () => {
    render(DayViewer, { props: props({ platform: 'desktop' }) });
    await openPost();

    await waitFor(() => expect(linkStatus()).toHaveTextContent(/^Opened in the channel\.$/));
  });

  it('says nothing when the person stayed on Discord’s prompt', async () => {
    const p = props({ platform: 'mobile', openLink: vi.fn().mockResolvedValue('cancelled') });
    render(DayViewer, { props: p });
    await openPost();
    await waitFor(() => expect(p.openLink).toHaveBeenCalledOnce());
    await Promise.resolve();

    expect(linkStatus()).toBeEmptyDOMElement();
    expect(screen.queryByText(DAY.jump_url)).toBeNull();
  });

  it('takes the line down after a few seconds', async () => {
    vi.useFakeTimers();
    try {
      render(DayViewer, { props: props({ platform: 'mobile' }) });
      await openPost();
      await vi.advanceTimersByTimeAsync(0);
      expect(linkStatus()).toHaveTextContent(/^Opened in the channel/);

      await vi.advanceTimersByTimeAsync(7_999);
      expect(linkStatus()).toHaveTextContent(/^Opened in the channel/);
      await vi.advanceTimersByTimeAsync(1);
      expect(linkStatus()).toBeEmptyDOMElement();

      // A later press says it again, for its own few seconds.
      await openPost();
      await vi.advanceTimersByTimeAsync(7_999);
      expect(linkStatus()).toHaveTextContent(/^Opened in the channel/);
      await vi.advanceTimersByTimeAsync(1);
      expect(linkStatus()).toBeEmptyDOMElement();
    } finally {
      vi.useRealTimers();
    }
  });

  it('takes the line down when the day changes', async () => {
    const p = props({ platform: 'mobile' });
    const { rerender } = render(DayViewer, { props: p });
    await openPost();
    await waitFor(() => expect(linkStatus()).toHaveTextContent(/^Opened in the channel/));

    await rerender({ ...p, dayNumber: 6, day: { ...DAY, day: 6 } });
    expect(linkStatus()).toBeEmptyDOMElement();
    // Gone, not hidden: it does not come back with the day.
    await rerender({ ...p, dayNumber: 5, day: DAY });
    expect(linkStatus()).toBeEmptyDOMElement();
  });

  it('does not say a post opened on a day the person has left by the time Discord answers', async () => {
    let answer: (outcome: string) => void = () => undefined;
    const p = props({ openLink: vi.fn(() => new Promise<string>((r) => (answer = r))) });
    const { rerender } = render(DayViewer, { props: p });
    await openPost();
    await rerender({ ...p, dayNumber: 6, day: { ...DAY, day: 6 } });

    answer('opened');
    // Long enough for the answer to be taken in and drawn.
    await new Promise((done) => setTimeout(done, 0));
    await tick();
    expect(linkStatus()).toBeEmptyDOMElement();
    // Nor when the person pages back to it.
    await rerender({ ...p, dayNumber: 5, day: DAY });
    expect(linkStatus()).toBeEmptyDOMElement();
  });

  it('keeps the line up for its few seconds after the later of two presses', async () => {
    vi.useFakeTimers();
    try {
      const answers: ((outcome: string) => void)[] = [];
      const openLink = vi.fn(() => new Promise<string>((r) => answers.push(r)));
      render(DayViewer, { props: props({ openLink }) });
      // Two presses before Discord answers either.
      await openPost();
      await openPost();
      expect(answers).toHaveLength(2);

      answers[0]!('opened');
      await vi.advanceTimersByTimeAsync(5_000);
      answers[1]!('opened');
      await vi.advanceTimersByTimeAsync(7_999);
      // The first answer's timer ran out 4.999s ago; the line is the second's.
      expect(linkStatus()).toHaveTextContent(/^Opened in the channel/);
      await vi.advanceTimersByTimeAsync(1);
      expect(linkStatus()).toBeEmptyDOMElement();
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows the link itself when Discord would not open it', async () => {
    const p = props({ openLink: vi.fn().mockResolvedValue('failed') });
    const { rerender } = render(DayViewer, { props: p });
    await fireEvent.click(screen.getByRole('button', { name: 'Open original post' }));
    expect(await screen.findByText(DAY.jump_url)).toBeInTheDocument();
    expect(linkStatus()).toHaveTextContent(/Discord didn’t open the post/);
    expect(linkStatus()).not.toHaveTextContent(/Opened in the channel/);

    // The notice belongs to that day.
    await rerender({ ...p, dayNumber: 6, day: { ...DAY, day: 6 } });
    await waitFor(() => expect(screen.queryByText(DAY.jump_url)).toBeNull());
  });

  it('swaps the link for the opened line once Discord does open it', async () => {
    const openLink = vi.fn().mockResolvedValueOnce('failed').mockResolvedValue('opened');
    render(DayViewer, { props: props({ openLink }) });
    await openPost();
    expect(await screen.findByText(DAY.jump_url)).toBeInTheDocument();

    await openPost();
    await waitFor(() => expect(linkStatus()).toHaveTextContent(/^Opened in the channel\.$/));
    expect(screen.queryByText(DAY.jump_url)).toBeNull();
  });

  it('hides Random when there is no other day', () => {
    render(DayViewer, { props: props({ onRandom: undefined }) });
    expect(screen.queryByRole('button', { name: 'Random' })).toBeNull();
  });
});

/**
 * A pointer event as a browser sends it. jsdom has no `PointerEvent`, so
 * this is a mouse event with the pointer's id added.
 */
function pointer(
  target: Element,
  type: 'pointerdown' | 'pointermove' | 'pointerup' | 'pointercancel',
  x: number,
  y: number,
  id = 1,
): Promise<boolean> {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y });
  Object.defineProperties(event, {
    pointerId: { value: id },
    pointerType: { value: 'touch' },
  });
  return fireEvent(target, event);
}

/** One finger dragged from `from` to `to` on `target`, then lifted. */
async function swipe(
  target: Element,
  from: { x: number; y: number },
  to: { x: number; y: number },
): Promise<void> {
  await pointer(target, 'pointerdown', from.x, from.y);
  await pointer(target, 'pointermove', (from.x + to.x) / 2, (from.y + to.y) / 2);
  await pointer(target, 'pointermove', to.x, to.y);
  await pointer(target, 'pointerup', to.x, to.y);
}

const LEFT = [
  { x: 300, y: 200 },
  { x: 180, y: 205 },
] as const;
const RIGHT = [
  { x: 100, y: 200 },
  { x: 220, y: 195 },
] as const;

describe('DayViewer touch', () => {
  const multi: Day = { ...DAY, media: [IMG, IMG_B] };
  const frame = (container: HTMLElement) => container.querySelector('.frame')!;
  const stage = (container: HTMLElement) => container.querySelector('.stage')!;

  it('pages a swipe on a photo through the files, then the days, one step each', async () => {
    const p = props({ day: multi });
    const { container } = render(DayViewer, { props: p });

    // The photo and the stage around it both hear the touch; only the photo
    // may act on it, or one swipe would skip a file.
    await swipe(frame(container), ...LEFT);
    expect(photo()).toHaveAttribute('src', '/api/media/b');
    expect(p.onNext).not.toHaveBeenCalled();

    await swipe(frame(container), ...RIGHT);
    expect(photo()).toHaveAttribute('src', '/api/media/a');
    expect(p.onPrev).not.toHaveBeenCalled();

    // Past the first file is the day before, entered at its last file.
    await swipe(frame(container), ...RIGHT);
    expect(p.onPrev).toHaveBeenCalledOnce();
    expect(photo()).toHaveAttribute('src', '/api/media/b');

    // Past the last is the day after.
    await swipe(frame(container), ...LEFT);
    expect(p.onNext).toHaveBeenCalledOnce();
  });

  it('does not turn the page for a short, a vertical or a dead-end swipe', async () => {
    const p = props({ hasNext: false });
    const { container } = render(DayViewer, { props: p });
    await swipe(frame(container), { x: 300, y: 200 }, { x: 270, y: 200 });
    await swipe(frame(container), { x: 300, y: 100 }, { x: 240, y: 300 });
    await swipe(frame(container), ...LEFT);
    expect(p.onNext).not.toHaveBeenCalled();
    expect(p.onPrev).not.toHaveBeenCalled();
  });

  it('zooms a photo on a double tap, and back on the next', async () => {
    const { container } = render(DayViewer, { props: props() });
    const tap = async () => {
      await pointer(frame(container), 'pointerdown', 150, 150);
      await pointer(frame(container), 'pointerup', 150, 150);
    };
    await tap();
    expect(frame(container)).not.toHaveClass('zoomed');
    await tap();
    expect(frame(container)).toHaveClass('zoomed');

    // Zoomed, a drag pans the photo: it must not turn the page.
    expect(screen.getByRole('button', { name: 'Zoom out' })).toBeEnabled();
    await swipe(frame(container), ...LEFT);
    expect(frame(container)).toHaveClass('zoomed');

    await tap();
    await tap();
    expect(frame(container)).not.toHaveClass('zoomed');
  });

  it('does nothing for a touch the browser took away', async () => {
    const p = props();
    const { container, rerender } = render(DayViewer, { props: p });
    await pointer(frame(container), 'pointerdown', 300, 200);
    await pointer(frame(container), 'pointermove', 180, 200);
    await pointer(frame(container), 'pointercancel', 180, 200);
    await pointer(frame(container), 'pointerup', 180, 200);

    // The same on a stage with no photo.
    await rerender({ ...p, day: null });
    await pointer(stage(container), 'pointerdown', 300, 200);
    await pointer(stage(container), 'pointermove', 180, 200);
    await pointer(stage(container), 'pointercancel', 180, 200);
    await pointer(stage(container), 'pointerup', 180, 200);
    expect(p.onNext).not.toHaveBeenCalled();
    expect(p.onPrev).not.toHaveBeenCalled();
  });

  it('turns the page for a swipe on a loading day and on a note', async () => {
    const p = props({ day: null });
    const { container, rerender } = render(DayViewer, { props: p });
    await swipe(screen.getByRole('status', { name: 'Loading Day 5' }), ...LEFT);
    expect(p.onNext).toHaveBeenCalledOnce();

    await rerender({ ...p, day: { ...DAY, media: [{ ...IMG, url: '', missing: true }] } });
    await swipe(screen.getByText('No file was saved for this day'), ...RIGHT);
    expect(p.onPrev).toHaveBeenCalledOnce();
    await swipe(stage(container), ...LEFT);
    expect(p.onNext).toHaveBeenCalledTimes(2);
  });

  it('leaves a touch that starts on an arrow to the arrow', async () => {
    const p = props({ day: null });
    render(DayViewer, { props: p });
    await swipe(screen.getByRole('button', { name: 'Next day' }), ...LEFT);
    expect(p.onNext).not.toHaveBeenCalled();
  });

  it('turns the page for a swipe on a photo that failed, and does not zoom it', async () => {
    const p = props({ day: multi, onRetry: vi.fn() });
    const { container } = render(DayViewer, { props: p });
    await fireEvent.error(container.querySelector('img.full')!);
    const note = screen.getByText('This photo didn’t load.');

    await pointer(note, 'pointerdown', 150, 150);
    await pointer(note, 'pointerup', 150, 150);
    await pointer(note, 'pointerdown', 150, 150);
    await pointer(note, 'pointerup', 150, 150);
    expect(frame(container)).not.toHaveClass('zoomed');

    // A touch on Try again is the button's own.
    await swipe(screen.getByRole('button', { name: 'Try again' }), ...LEFT);
    expect(screen.getByText('This photo didn’t load.')).toBeInTheDocument();

    await swipe(note, ...LEFT);
    expect(photo()).toHaveAttribute('src', '/api/media/b');
    expect(screen.queryByRole('alert')).toBeNull();
    expect(p.onNext).not.toHaveBeenCalled();
  });

  describe('on a video', () => {
    // A portrait clip filling a phone's stage: 300 x 600, from the top.
    function videoDay() {
      const p = props({ day: { ...DAY, media: [VIDEO, IMG_B] } });
      const { container } = render(DayViewer, { props: p });
      const video = container.querySelector('video')!;
      vi.spyOn(video, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 300, 600));
      return { p, video };
    }

    it('turns the page for a swipe that starts on the picture', async () => {
      const { p, video } = videoDay();
      await swipe(video, { x: 250, y: 300 }, { x: 120, y: 310 });
      expect(photo()).toHaveAttribute('src', '/api/media/b');
      expect(p.onNext).not.toHaveBeenCalled();
    });

    it('leaves the band along the bottom to the player’s controls', async () => {
      const { p, video } = videoDay();
      // The lowest quarter: a drag there moves the seek bar.
      await swipe(video, { x: 250, y: 460 }, { x: 120, y: 460 });
      await swipe(video, { x: 250, y: 590 }, { x: 120, y: 590 });
      expect(document.querySelector('video')).toBe(video);
      expect(p.onPrev).not.toHaveBeenCalled();
      expect(p.onNext).not.toHaveBeenCalled();

      // Just above it is the picture again.
      await swipe(video, { x: 120, y: 440 }, { x: 250, y: 440 });
      expect(p.onPrev).toHaveBeenCalledOnce();
    });

    it('keeps at least 56px for the controls of a short video', async () => {
      const { p, video } = videoDay();
      vi.spyOn(video, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 100, 300, 120));
      await swipe(video, { x: 120, y: 170 }, { x: 250, y: 170 });
      expect(p.onPrev).not.toHaveBeenCalled();
      await swipe(video, { x: 120, y: 150 }, { x: 250, y: 150 });
      expect(p.onPrev).toHaveBeenCalledOnce();
    });
  });
});

describe('DayViewer focus', () => {
  it('keeps focus in the dialog when the arrow that had it goes away', async () => {
    const multi: Day = { ...DAY, media: [IMG, IMG_B] };
    render(DayViewer, { props: props({ day: multi, hasPrev: false, hasNext: false }) });
    const dialog = screen.getByRole('dialog');

    // Onto the last photo of the last day: there is no Next any more.
    const next = screen.getByRole('button', { name: 'Next photo' });
    next.focus();
    await fireEvent.click(next);
    expect(screen.queryByRole('button', { name: /^Next/ })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(dialog));

    // And back to the first of the first.
    const previous = screen.getByRole('button', { name: 'Previous photo' });
    previous.focus();
    await fireEvent.click(previous);
    expect(screen.queryByRole('button', { name: /^Previous/ })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(dialog));
  });

  it('leaves focus alone while the arrow that has it stays', async () => {
    const p = props();
    render(DayViewer, { props: p });
    const next = screen.getByRole('button', { name: 'Next day' });
    next.focus();
    await fireEvent.click(next);
    expect(document.activeElement).toBe(next);
  });
});
