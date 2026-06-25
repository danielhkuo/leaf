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

  it('offers the start CTA when the viewer is eligible', async () => {
    const onCreate = vi.fn();
    render(SeriesPicker, {
      props: {
        series: [],
        onSelect: vi.fn(),
        eligibility: { can_create: true, violations: [] },
        eligibilityStatus: 'ready',
        onCreate,
      },
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Start a series' }));
    expect(onCreate).toHaveBeenCalled();
  });

  it('shows the policy reason instead of a CTA when ineligible', () => {
    render(SeriesPicker, {
      props: {
        series: [],
        onSelect: vi.fn(),
        eligibility: {
          can_create: false,
          violations: [{ code: 'missing_creator_role', message: 'You need the creator role.' }],
        },
        eligibilityStatus: 'ready',
      },
    });
    expect(screen.getByText('You need the creator role.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Start a series' })).not.toBeInTheDocument();
  });

  it('links to my series when the viewer owns one', async () => {
    const onManage = vi.fn();
    render(SeriesPicker, { props: { series, onSelect: vi.fn(), ownsSeries: true, onManage } });
    await fireEvent.click(screen.getByRole('button', { name: 'Manage my series' }));
    expect(onManage).toHaveBeenCalled();
  });
});
