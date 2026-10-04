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
  it('has a heading and reports the selection', async () => {
    const onSelect = vi.fn();
    render(SeriesPicker, { props: { series, onSelect } });

    expect(screen.getByRole('heading', { level: 1, name: 'Series' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Daily Cat/ })).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: /Daily Johan/ }));

    expect(onSelect).toHaveBeenCalledWith(series[0]);
  });

  it('shows the empty callout without claiming posts appear on their own', () => {
    render(SeriesPicker, {
      props: {
        series: [],
        onSelect: vi.fn(),
        eligibility: { can_create: true, violations: [] },
        eligibilityStatus: 'ready',
      },
    });
    expect(screen.getByText(/No series here yet/)).toBeInTheDocument();
    expect(screen.getByText(/You add its days from chat/)).toBeInTheDocument();
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

  it('puts the reason behind a quiet question when the viewer is not eligible', () => {
    render(SeriesPicker, {
      props: {
        series,
        onSelect: vi.fn(),
        eligibility: {
          can_create: false,
          violations: [
            {
              code: 'missing_creator_role',
              message: 'starting a series here requires the creator role',
              params: { role_name: 'Artists' },
            },
          ],
        },
        eligibilityStatus: 'ready',
      },
    });
    const why = screen.getByText('Want your own series?');
    expect(why.closest('details')).not.toHaveAttribute('open');
    expect(screen.getByText(/needs the @Artists role/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Start a series' })).not.toBeInTheDocument();
  });

  it('says an admin must run /setup in a server that is not set up', () => {
    render(SeriesPicker, {
      props: {
        series: [],
        onSelect: vi.fn(),
        eligibility: {
          can_create: false,
          violations: [{ code: 'guild_not_setup', message: '' }],
        },
        eligibilityStatus: 'ready',
      },
    });
    expect(screen.getByText(/isn’t set up in this server yet/)).toBeInTheDocument();
    expect(screen.getByText(/run \/setup in chat/)).toBeInTheDocument();
    expect(screen.queryByText('Want your own series?')).not.toBeInTheDocument();
  });

  it('links to my series when the viewer owns one', async () => {
    const onManage = vi.fn();
    render(SeriesPicker, { props: { series, onSelect: vi.fn(), ownsSeries: true, onManage } });
    await fireEvent.click(screen.getByRole('button', { name: 'Manage my series' }));
    expect(onManage).toHaveBeenCalled();
  });

  it('lists the series of the launch channel first', () => {
    render(SeriesPicker, { props: { series, onSelect: vi.fn(), inChannel: [2] } });
    const here = screen.getByRole('region', { name: 'In this channel' });
    expect(here).toHaveTextContent('Daily Cat');
    expect(here).not.toHaveTextContent('Daily Johan');
    expect(screen.getByRole('region', { name: 'More series' })).toHaveTextContent('Daily Johan');
  });

  it('shows a revoked series with the reason, and does not open it', () => {
    const onSelect = vi.fn();
    const revoked: Series = { ...series[0]!, id: 3, name: 'Gone Quiet', state: 'revoked' };
    render(SeriesPicker, { props: { series: [revoked], onSelect } });
    expect(screen.queryByRole('button', { name: /Gone Quiet/ })).not.toBeInTheDocument();
    expect(screen.getByText(/A server admin revoked this series/)).toBeInTheDocument();
  });

  it('tells the owner a sprout is hidden until enough days are archived', () => {
    const sprout: Series = {
      ...series[0]!,
      sprout: { archived: 2, threshold: 5 },
      is_owner: true,
      privacy: 'public',
    };
    render(SeriesPicker, { props: { series: [sprout], onSelect: vi.fn() } });
    expect(
      screen.getByText(
        /Only you can see this until 5 days are archived \(2 so far\)\. Then everyone in the server can\./,
      ),
    ).toBeInTheDocument();
  });

  it('does not promise others will see a sprout whose privacy is Only me', () => {
    const sprout: Series = {
      ...series[0]!,
      sprout: { archived: 2, threshold: 5 },
      is_owner: true,
      privacy: 'creator_only',
    };
    render(SeriesPicker, { props: { series: [sprout], onSelect: vi.fn() } });
    expect(screen.getByText(/Its privacy is Only me, so that stays the same/)).toBeInTheDocument();
    expect(screen.queryByText(/until 5 days are archived/)).not.toBeInTheDocument();
  });
});
