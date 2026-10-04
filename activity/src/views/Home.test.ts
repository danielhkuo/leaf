import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { nav } from '../lib/stores/nav.svelte';
import { expectNoA11yViolations } from '../lib/test/a11y';
import type { DaySummary, Series } from '../lib/types/api';
import { ApiError } from '../lib/utils/errors';
import Home from './Home.svelte';

const store = vi.hoisted(() => {
  const api = { getStats: vi.fn(), listMySeries: vi.fn() };
  return {
    api,
    gallery: {
      status: 'ready',
      series: [] as unknown[],
      eligibility: { can_create: true, violations: [] },
      eligibilityStatus: 'ready',
      epoch: 0,
      refreshing: false,
    },
    getApi: () => api,
    getGuildId: () => 'g1',
    loadDaysIndex: vi.fn(),
    refreshAll: vi.fn(),
  };
});
vi.mock('../lib/stores/gallery.svelte', async () => {
  // Reactive, as the real store's state is: a refresh bumps `epoch`, and the
  // loads that read it run again.
  const { reactive } = await import('../lib/test/reactive.svelte');
  store.gallery = reactive(store.gallery);
  return store;
});

const STATS = { total: 3, current_streak: 3, longest_streak: 3, missed: 0, max_day: 3 };

/** The owner's own list: one row, the series these tests show. */
function mine(channel: Record<string, unknown>): unknown[] {
  return [
    {
      id: 1,
      name: 'Daily Sketch',
      emoji: '✏️',
      state: 'active',
      cadence: 'daily',
      channel_id: 'c1',
      archived_days: 0,
      reminder_enabled: false,
      ...channel,
    },
  ];
}

const GONE_TITLE = 'This series’ channel is gone';
/** The first archive step, as a person reads it. */
function firstStep(): string {
  return (document.querySelector('.steps li')?.textContent ?? '').replace(/\s+/g, ' ').trim();
}

function series(extra: Partial<Series> = {}): Series {
  return {
    id: 1,
    name: 'Daily Sketch',
    description: '',
    creator_id: 'owner',
    cadence: 'daily',
    emoji: '✏️',
    start_day: 1,
    max_day: 3,
    ...extra,
  };
}

function index(): DaySummary[] {
  return [1, 2, 3].map((day) => ({
    day,
    posted_at: Math.floor(new Date(2024, 5, day, 12).getTime() / 1000),
    thumb_url: null,
  }));
}

beforeEach(() => {
  nav.reset({ name: 'picker' }, { name: 'home', seriesId: 1 });
  store.gallery.epoch = 0;
  store.api.getStats.mockReset().mockResolvedValue(STATS);
  store.api.listMySeries.mockReset().mockResolvedValue(mine({ channel_name: 'daily-sketch' }));
  store.loadDaysIndex.mockReset().mockResolvedValue(index());
  store.refreshAll.mockReset().mockResolvedValue(true);
});

describe('Home', () => {
  it('shows the owner how to archive the first post, worded for phones', async () => {
    render(Home, {
      props: {
        series: series({ max_day: null }),
        userId: 'owner',
        canGoBack: true,
        platform: 'mobile',
        appName: 'Sketchbook Archive',
        created: true,
      },
    });

    expect(screen.getByText('Series created')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Archive your first post' })).toBeInTheDocument();
    expect(screen.getByText('Press and hold your message.')).toBeInTheDocument();
    // The app is named as Discord's Apps list names it.
    expect(screen.getByText('Sketchbook Archive')).toBeInTheDocument();
    expect(screen.getAllByText('Archive to Series')).not.toHaveLength(0);
    expect(await screen.findByText('#daily-sketch')).toBeInTheDocument();
    expect(screen.queryByText(/appear here as they’re posted/)).not.toBeInTheDocument();
    // Its channel is there: nothing to warn about.
    expect(screen.queryByText(GONE_TITLE)).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
    expect(store.refreshAll).toHaveBeenCalled();
    expect(await screen.findByText(/Nothing archived yet/)).toBeInTheDocument();
    expect(store.api.getStats).not.toHaveBeenCalled();
  });

  it.each([
    ['a current server: no name, and the flag', { channel_name: null, channel_missing: true }],
    ['no name alone', { channel_name: null }],
    [
      'the flag, over a name the server still had',
      { channel_name: 'general', channel_missing: true },
    ],
  ])('tells the owner the series’ channel is gone (%s)', async (_, channel) => {
    store.api.listMySeries.mockResolvedValue(mine(channel));
    render(Home, {
      props: {
        series: series({ max_day: null }),
        userId: 'owner',
        canGoBack: true,
        platform: 'mobile',
      },
    });

    expect(await screen.findByText(GONE_TITLE)).toBeInTheDocument();
    expect(
      screen.getByText(
        'It was deleted or hidden from leaf, so nothing posted there can be archived.',
      ),
    ).toBeInTheDocument();
    // The steps name no channel, least of all the one that is gone.
    expect(firstStep()).toBe(
      'Minimise leaf, then post your photo or video in one of this server’s series channels.',
    );
    expect(screen.queryByText(/#general/)).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Choose another channel' }));
    expect(nav.current).toEqual({ name: 'seriesSettings', seriesId: 1 });
  });

  it('says so over the calendar too, where the steps are behind a disclosure', async () => {
    store.api.listMySeries.mockResolvedValue(mine({ channel_name: null, channel_missing: true }));
    const { container } = render(Home, {
      props: { series: series(), userId: 'owner', canGoBack: true },
    });

    expect(await screen.findByText(GONE_TITLE)).toBeInTheDocument();
    expect(await screen.findByRole('button', { name: /^Day 3, / })).toBeInTheDocument();
    expect(screen.getByText('How to archive a post')).toBeInTheDocument();
    expect(firstStep()).toBe('Post your photo or video in one of this server’s series channels.');
    await expectNoA11yViolations(container);
  });

  it('asks again after a refresh: the channel may have gone while leaf was open', async () => {
    render(Home, { props: { series: series(), userId: 'owner', canGoBack: true } });
    await waitFor(() => expect(store.api.listMySeries).toHaveBeenCalledTimes(1));
    expect(screen.queryByText(GONE_TITLE)).not.toBeInTheDocument();

    store.api.listMySeries.mockResolvedValue(mine({ channel_name: null, channel_missing: true }));
    store.gallery.epoch += 1;
    expect(await screen.findByText(GONE_TITLE)).toBeInTheDocument();
    expect(store.api.listMySeries).toHaveBeenCalledTimes(2);

    // And back, once an admin has put it right.
    store.api.listMySeries.mockResolvedValue(mine({ channel_name: 'daily-sketch' }));
    store.gallery.epoch += 1;
    await waitFor(() => expect(screen.queryByText(GONE_TITLE)).not.toBeInTheDocument());
  });

  it('shows a viewer nothing about the channel, and does not ask', async () => {
    store.api.listMySeries.mockResolvedValue(mine({ channel_name: null, channel_missing: true }));
    render(Home, { props: { series: series(), userId: 'viewer', canGoBack: true } });

    expect(await screen.findByRole('button', { name: /^Day 3, / })).toBeInTheDocument();
    expect(store.api.listMySeries).not.toHaveBeenCalled();
    expect(screen.queryByText(GONE_TITLE)).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Choose another channel' }),
    ).not.toBeInTheDocument();
  });

  it.each([
    [
      'the server says the channel is there',
      () => Promise.resolve(mine({ channel_name: null, channel_missing: false })),
    ],
    [
      'the series never had a channel, and the server sends no flag',
      () => Promise.resolve(mine({ channel_id: null, channel_name: null })),
    ],
    ['the list did not load', () => Promise.reject(new ApiError(503, 'GET /series/mine → 503'))],
    ['the list does not have the series', () => Promise.resolve([])],
  ])('does not call the channel gone on no more than a missing name (%s)', async (_, answer) => {
    const answered = answer();
    store.api.listMySeries.mockReturnValue(answered);
    render(Home, {
      props: { series: series({ max_day: null }), userId: 'owner', canGoBack: true },
    });
    await answered.catch(() => undefined);
    await tick();

    expect(store.api.listMySeries).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(GONE_TITLE)).not.toBeInTheDocument();
    expect(firstStep()).toBe('Post your photo or video in one of this server’s series channels.');
  });

  it('words the steps for desktop', () => {
    render(Home, {
      props: { series: series({ max_day: null }), userId: 'owner', canGoBack: false },
    });
    expect(screen.getByText(/Right-click your message/)).toBeInTheDocument();
  });

  it('tells anyone else there are no days yet', () => {
    render(Home, {
      props: { series: series({ max_day: null }), userId: 'viewer', canGoBack: true },
    });
    expect(screen.getByText('No days yet')).toBeInTheDocument();
    expect(screen.queryByText('Archive your first post')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Series settings' })).not.toBeInTheDocument();
  });

  it('explains a revoked series and loads nothing for it', () => {
    render(Home, {
      props: { series: series({ state: 'revoked' }), userId: 'owner', canGoBack: true },
    });
    expect(screen.getByText('A server admin revoked this series')).toBeInTheDocument();
    expect(store.api.getStats).not.toHaveBeenCalled();
    expect(store.loadDaysIndex).not.toHaveBeenCalled();
    expect(store.api.listMySeries).not.toHaveBeenCalled();
  });

  it('tells the owner a sprout is hidden, and for how long', () => {
    render(Home, {
      props: {
        series: series({ sprout: { archived: 2, threshold: 5 }, privacy: 'public' }),
        userId: 'owner',
        canGoBack: true,
      },
    });
    expect(screen.getByText(/Only you can see this series for now/)).toBeInTheDocument();
    expect(
      screen.getByText('Everyone in the server can see it once 5 days are archived (2 so far).'),
    ).toBeInTheDocument();
  });

  it('does not promise others will see a sprout whose privacy is Only me', () => {
    render(Home, {
      props: {
        series: series({ sprout: { archived: 0, threshold: 5 }, privacy: 'creator_only' }),
        userId: 'owner',
        canGoBack: true,
      },
    });
    expect(screen.getByText('🌱 Only you can see this series')).toBeInTheDocument();
    expect(screen.getByText(/Its privacy is Only me, so that stays the same/)).toBeInTheDocument();
    expect(screen.queryByText(/for now|can see it once/)).not.toBeInTheDocument();
  });

  it('offers Try again when the stats fail, and loads them again', async () => {
    store.api.getStats
      .mockReset()
      .mockRejectedValueOnce(new ApiError(503, 'GET /stats → 503'))
      .mockResolvedValue(STATS);
    render(Home, { props: { series: series(), userId: 'viewer', canGoBack: true } });

    expect(await screen.findByText('Couldn’t load the stats')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() =>
      expect(screen.queryByText('Couldn’t load the stats')).not.toBeInTheDocument(),
    );
    expect(screen.getByText('Latest run')).toBeInTheDocument();
    expect(store.api.getStats).toHaveBeenCalledTimes(2);
  });

  it('offers Try again when the calendar fails', async () => {
    store.loadDaysIndex
      .mockReset()
      .mockRejectedValueOnce(new ApiError(0, 'GET /days', undefined, { kind: 'network' }))
      .mockResolvedValue(index());
    render(Home, { props: { series: series(), userId: 'viewer', canGoBack: true } });

    expect(await screen.findByText('Couldn’t load the calendar')).toBeInTheDocument();
    expect(screen.getByText(/Check your connection/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('button', { name: /^Day 3, / })).toBeInTheDocument();
  });

  it('opens the latest day and a day by number', async () => {
    render(Home, { props: { series: series(), userId: 'viewer', canGoBack: true } });

    await fireEvent.click(await screen.findByRole('button', { name: 'Latest: Day 3' }));
    expect(nav.current).toEqual({ name: 'viewer', seriesId: 1, day: 3 });

    nav.back();
    await fireEvent.input(screen.getByLabelText('Go to day number'), { target: { value: '7' } });
    await fireEvent.submit(screen.getByLabelText('Go to day number'));
    expect(nav.current).toEqual({ name: 'viewer', seriesId: 1, day: 3 });
    expect(
      screen.getByText(/Day 7 isn’t archived, so Day 3, the closest, opened/),
    ).toBeInTheDocument();
  });

  it('does not open a second viewer over an open one', async () => {
    render(Home, { props: { series: series(), userId: 'viewer', canGoBack: true } });
    const cell = await screen.findByRole('button', { name: /^Day 2, / });
    await fireEvent.click(cell);
    await fireEvent.click(cell);
    nav.back();
    expect(nav.current).toEqual({ name: 'home', seriesId: 1 });
  });

  it('refreshes on request and says how it went', async () => {
    store.refreshAll.mockResolvedValueOnce(false);
    render(Home, { props: { series: series(), userId: 'viewer', canGoBack: true } });
    await fireEvent.click(await screen.findByRole('button', { name: 'Refresh' }));
    expect(await screen.findByText(/Couldn’t refresh/)).toBeInTheDocument();
  });

  it('has no axe violations, with a calendar and as an empty owner’s page', async () => {
    const full = render(Home, { props: { series: series(), userId: 'owner', canGoBack: true } });
    await screen.findByRole('button', { name: /^Day 3, / });
    await expectNoA11yViolations(full.container);
    full.unmount();

    const empty = render(Home, {
      props: { series: series({ max_day: null }), userId: 'owner', canGoBack: true },
    });
    await expectNoA11yViolations(empty.container);
  });

  it('keeps Back, create (a text button and a phone-width icon) and settings in the header', () => {
    render(Home, { props: { series: series(), userId: 'owner', canGoBack: true } });
    expect(screen.getByRole('heading', { level: 1, name: 'Daily Sketch' })).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Start a series' })).toHaveLength(2);
    expect(screen.getByRole('button', { name: 'Series settings' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Back to series list' })).toBeInTheDocument();
  });
});
