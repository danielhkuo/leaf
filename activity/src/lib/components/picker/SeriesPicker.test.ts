import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import type { Series } from '../../types/api';
import SeriesPicker from './SeriesPicker.svelte';

const series: Series[] = [
  {
    id: 1,
    name: 'Daily Johan',
    description: 'a daily thing',
    creator_id: 'u',
    cadence: 'daily',
    emoji: '🍃',
    start_day: 1,
    max_day: 5,
  },
  {
    id: 2,
    name: 'Daily Cat',
    description: '',
    creator_id: 'u',
    cadence: 'daily',
    emoji: '🐈',
    start_day: 1,
    max_day: null,
  },
];

describe('SeriesPicker', () => {
  it('renders a card per series and reports the selection', async () => {
    const onSelect = vi.fn();
    render(SeriesPicker, { props: { series, onSelect } });

    expect(screen.getByRole('button', { name: /Daily Cat/ })).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: /Daily Johan/ }));

    expect(onSelect).toHaveBeenCalledWith(series[0]);
  });

  it('shows the empty callout when there are no series', () => {
    render(SeriesPicker, { props: { series: [], onSelect: vi.fn() } });
    expect(screen.getByText(/No series here yet/)).toBeInTheDocument();
  });
});
