import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

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
  it('renders a clickable cell per archived day and opens it', async () => {
    const onOpenDay = vi.fn();
    render(Calendar, {
      props: { index: [entry(1, 2024, 5, 3), entry(2, 2024, 5, 5)], onOpenDay },
    });

    expect(screen.getByRole('button', { name: 'Day 1' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Day 2' })).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Day 1' }));
    expect(onOpenDay).toHaveBeenCalledWith(1);
  });

  it('renders nothing for an empty index', () => {
    const { container } = render(Calendar, { props: { index: [], onOpenDay: vi.fn() } });
    expect(container.querySelectorAll('button')).toHaveLength(0);
  });
});
