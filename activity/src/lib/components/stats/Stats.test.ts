import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import Stats from './Stats.svelte';

describe('Stats', () => {
  it('renders streaks and totals from a fixture', () => {
    render(Stats, {
      props: {
        stats: { total: 10, current_streak: 3, longest_streak: 7, missed: 2, max_day: 10 },
      },
    });

    expect(screen.getByText('Current streak')).toBeInTheDocument();
    expect(screen.getByText('Longest streak')).toBeInTheDocument();
    expect(screen.getByText('Missed')).toBeInTheDocument();
    expect(screen.getByText('3')).toBeInTheDocument();
    expect(screen.getByText('7')).toBeInTheDocument();
  });
});
