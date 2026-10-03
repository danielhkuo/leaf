import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import Stats from './Stats.svelte';

const STATS = { total: 10, current_streak: 3, longest_streak: 1, missed: 2, max_day: 10 };

describe('Stats', () => {
  it('labels runs as day-number runs, with units', () => {
    render(Stats, { props: { stats: STATS } });

    expect(screen.getByText('Latest run')).toBeInTheDocument();
    expect(screen.getByText('Longest run')).toBeInTheDocument();
    expect(screen.getByText('Days archived')).toBeInTheDocument();
    expect(screen.getByText('Skipped day numbers')).toBeInTheDocument();
    expect(screen.queryByText(/streak/i)).not.toBeInTheDocument();
    expect(screen.getByText('Latest run').nextElementSibling).toHaveTextContent('3 days');
    expect(screen.getByText('Longest run').nextElementSibling).toHaveTextContent('1 day');
    expect(screen.getByText('Days archived').nextElementSibling).toHaveTextContent('10');
  });

  it('says when the last post was', () => {
    const now = new Date(2024, 5, 20, 9).getTime();
    const lastPostedAt = Math.floor(new Date(2024, 5, 19, 12).getTime() / 1000);
    render(Stats, { props: { stats: STATS, lastPostedAt, now } });
    expect(screen.getByText(/Last post/)).toHaveTextContent('Last post yesterday');
  });

  it('keeps its shape while loading, with one status for the whole card', () => {
    render(Stats, { props: { stats: null } });
    expect(screen.getByText('Latest run')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Loading stats');
  });
});
