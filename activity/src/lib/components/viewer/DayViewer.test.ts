import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import type { Day } from '../../types/api';
import DayViewer from './DayViewer.svelte';

const IMG = {
  url: '/api/media/a',
  thumb_url: '/api/media/a?thumb',
  content_type: 'image/png',
  missing: false,
};

const DAY: Day = {
  day: 5,
  caption: 'A nice day',
  posted_at: 1_700_000_000,
  jump_url: 'https://discord.com/channels/g/c/m',
  media: [IMG],
};

function props(over: Partial<Record<string, unknown>> = {}) {
  return {
    day: DAY,
    seriesName: 'Daily Johan',
    hasPrev: true,
    hasNext: true,
    onPrev: vi.fn(),
    onNext: vi.fn(),
    onRandom: vi.fn(),
    onClose: vi.fn(),
    onJump: vi.fn(),
    ...over,
  };
}

describe('DayViewer', () => {
  it('renders the day number, caption, and the full-res image', () => {
    render(DayViewer, { props: props() });
    expect(screen.getByText('Day 5')).toBeInTheDocument();
    expect(screen.getByText('A nice day')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'A nice day' })).toHaveAttribute('src', '/api/media/a');
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

  it('does not navigate past the ends', async () => {
    const p = props({ hasNext: false });
    render(DayViewer, { props: p });
    await fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(p.onNext).not.toHaveBeenCalled();
  });

  it('releases the full-res image on unmount', () => {
    const { unmount } = render(DayViewer, { props: props() });
    expect(screen.queryByRole('img', { name: 'A nice day' })).toBeInTheDocument();
    unmount();
    expect(screen.queryByRole('img', { name: 'A nice day' })).not.toBeInTheDocument();
  });

  it('switches attachments via the carousel dots', async () => {
    const multi: Day = {
      ...DAY,
      media: [IMG, { ...IMG, url: '/api/media/b', thumb_url: '/api/media/b?thumb' }],
    };
    render(DayViewer, { props: props({ day: multi }) });

    const dots = screen.getAllByRole('button', { name: /Attachment/ });
    expect(dots).toHaveLength(2);
    expect(screen.getByRole('img', { name: 'A nice day' })).toHaveAttribute('src', '/api/media/a');

    await fireEvent.click(dots[1]!);
    expect(screen.getByRole('img', { name: 'A nice day' })).toHaveAttribute('src', '/api/media/b');
  });

  it('keeps exactly one full-res image across day navigation (no accumulation)', async () => {
    const { container, rerender } = render(DayViewer, { props: props() });
    expect(container.querySelectorAll('img.full')).toHaveLength(1);

    // Navigate through several days; the full-res <img> is reused, never piled
    // up — the structural guarantee behind "memory flat over a 50-day browse".
    for (let day = 6; day <= 10; day += 1) {
      await rerender(
        props({ day: { ...DAY, day, media: [{ ...IMG, url: `/api/media/${day}` }] } }),
      );
      expect(container.querySelectorAll('img.full')).toHaveLength(1);
    }
  });
});
