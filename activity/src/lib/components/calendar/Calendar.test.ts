import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import { expectNoA11yViolations } from '../../test/a11y';
import type { DaySummary } from '../../types/api';
import Calendar from './Calendar.svelte';

function entry(day: number, year: number, month: number, dom: number): DaySummary {
  return {
    day,
    posted_at: Math.floor(new Date(year, month, dom, 12).getTime() / 1000),
    thumb_url: `t${day}`,
  };
}

describe('Calendar', () => {
  it('names each archived day with its full date and opens it', async () => {
    const onOpenDay = vi.fn();
    render(Calendar, {
      props: { index: [entry(1, 2024, 5, 3), entry(2, 2024, 5, 5)], onOpenDay, weekStart: 0 },
    });

    const first = screen.getByRole('button', { name: /^Day 1, .*June 3, 2024/ });
    expect(screen.getByRole('button', { name: /^Day 2, .*June 5, 2024/ })).toBeInTheDocument();

    await fireEvent.click(first);
    expect(onOpenDay).toHaveBeenCalledWith(1);
  });

  it('shows the newest month first and one line for empty months', () => {
    render(Calendar, {
      props: {
        index: [entry(1, 2024, 0, 3), entry(2, 2024, 3, 5)],
        onOpenDay: vi.fn(),
        weekStart: 0,
      },
    });
    const headings = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent);
    const april = headings.findIndex((h) => h?.includes('April 2024'));
    const january = headings.findIndex((h) => h?.includes('January 2024'));
    expect(april).toBeGreaterThanOrEqual(0);
    expect(april).toBeLessThan(january);
    expect(screen.getByText(/No days archived from February to March 2024/)).toBeInTheDocument();
  });

  it('opens the first of several days that share a date', async () => {
    const onOpenDay = vi.fn();
    render(Calendar, {
      props: { index: [entry(8, 2024, 5, 4), entry(7, 2024, 5, 4)], onOpenDay, weekStart: 0 },
    });
    const cell = screen.getByRole('button', { name: /^Day 7 and 1 more, / });
    expect(cell).toHaveAttribute('data-days', '7 8');
    await fireEvent.click(cell);
    expect(onOpenDay).toHaveBeenCalledWith(7);
  });

  it('keeps days with an impossible date reachable under Undated', async () => {
    const onOpenDay = vi.fn();
    render(Calendar, {
      props: {
        index: [entry(2, 2024, 5, 4), { day: 1, posted_at: 0, thumb_url: null }],
        onOpenDay,
      },
    });
    expect(screen.getByRole('heading', { name: 'Undated' })).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Day 1, date unknown, no preview' }));
    expect(onOpenDay).toHaveBeenCalledWith(1);
  });

  it('says which timezone the dates are in', () => {
    render(Calendar, {
      props: {
        index: [{ ...entry(1, 2024, 5, 3), local_date: '2024-06-03' }],
        onOpenDay: vi.fn(),
        timeZone: 'America/Chicago',
      },
    });
    expect(screen.getByText('Dates are in America/Chicago time.')).toBeInTheDocument();
  });

  it('falls back to the hatched tile when a thumbnail fails to load', async () => {
    const { container } = render(Calendar, {
      props: { index: [entry(1, 2024, 5, 3)], onOpenDay: vi.fn() },
    });
    const img = container.querySelector('img');
    expect(img).not.toBeNull();
    expect(screen.getByRole('button', { name: /^Day 1,/ })).not.toHaveAccessibleName(/no preview/);
    if (img) await fireEvent.error(img);
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('.missing')).not.toBeNull();
    // The label says what is drawn.
    expect(screen.getByRole('button', { name: /^Day 1,.*, no preview$/ })).toBeInTheDocument();
  });

  it('has no axe violations', async () => {
    const { container } = render(Calendar, {
      props: {
        index: [
          entry(1, 2024, 0, 3),
          entry(2, 2024, 3, 5),
          { day: 3, posted_at: 0, thumb_url: null },
        ],
        onOpenDay: vi.fn(),
        timeZone: 'America/Chicago',
      },
    });
    await expectNoA11yViolations(container);
  });

  it('renders nothing for an empty index', () => {
    const { container } = render(Calendar, { props: { index: [], onOpenDay: vi.fn() } });
    expect(container.querySelectorAll('button')).toHaveLength(0);
  });
});
