import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { nav } from '../../lib/stores/nav.svelte';
import { expectNoA11yViolations } from '../../lib/test/a11y';
import type { MySeries as Mine } from '../../lib/types/api';
import { ApiError } from '../../lib/utils/errors';
import MySeries from './MySeries.svelte';

const store = vi.hoisted(() => {
  const api = { listMySeries: vi.fn(), getSettings: vi.fn() };
  return {
    api,
    gallery: {
      series: [] as unknown[],
      eligibility: { can_create: true, violations: [] as unknown[] },
    },
    getApi: () => api,
    getGuildId: () => 'g1',
  };
});
vi.mock('../../lib/stores/gallery.svelte', () => store);

function mine(extra: Partial<Mine> = {}): Mine {
  return {
    id: 1,
    name: 'Daily Sketch',
    emoji: '✏️',
    state: 'active',
    cadence: 'daily',
    channel_id: 'c1',
    channel_name: 'daily-sketch',
    archived_days: 124,
    reminder_enabled: true,
    reminder_error: undefined,
    ...extra,
  };
}

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  nav.reset({ name: 'picker' }, { name: 'mySeries' });
  store.gallery.series = [];
  store.gallery.eligibility = { can_create: true, violations: [] };
  store.api.listMySeries.mockReset().mockResolvedValue([mine()]);
  // What the view reads from a series' settings: whether its reminders arrive.
  store.api.getSettings.mockReset().mockResolvedValue({ reminder_enabled: true });
});

describe('MySeries', () => {
  it('lists each series with its facts and opens its settings', async () => {
    const { container } = render(MySeries);
    expect(screen.getByRole('heading', { name: 'My series' })).toHaveFocus();

    const card = await screen.findByRole('button', { name: /^Daily Sketch/ });
    expect(card).toHaveAccessibleName(/settings/);
    expect(card).toHaveTextContent('daily · #daily-sketch · 124 days archived · reminders on');
    // A button may hold only phrasing content.
    expect(card.querySelector('div, section, p')).toBeNull();
    await expectNoA11yViolations(container);

    await fireEvent.click(card);
    expect(nav.current).toEqual({ name: 'seriesSettings', seriesId: 1 });
  });

  it('shows a sprout’s progress and who will see it, from the gallery’s list', async () => {
    store.gallery.series = [{ id: 2, privacy: 'public', sprout: { archived: 1, threshold: 3 } }];
    store.api.listMySeries.mockResolvedValue([
      mine({ id: 2, name: 'Coffee', state: 'sprout', archived_days: 1, reminder_enabled: false }),
    ]);
    render(MySeries);

    const card = await screen.findByRole('button', { name: /^Coffee/ });
    expect(card).toHaveTextContent('🌱 Sprout · 1 of 3 days');
    expect(card).toHaveTextContent('1 day archived');
    expect(card).toHaveTextContent(
      'Only you can see it until 3 days are archived. Then everyone in the server can.',
    );
  });

  it('explains a revoked series instead of leaving a bare badge', async () => {
    store.api.listMySeries.mockResolvedValue([mine({ state: 'revoked', channel_name: null })]);
    render(MySeries);
    const card = await screen.findByRole('button', { name: /^Daily Sketch/ });
    expect(card).toHaveTextContent('Revoked');
    expect(card).toHaveTextContent(/A server admin revoked this series/);
    expect(card).toHaveTextContent(/Ask an admin to restore it/);
  });

  it('says when reminders are on but not arriving', async () => {
    store.api.listMySeries.mockResolvedValue([mine({ reminder_error: 'dm_closed' })]);
    render(MySeries);
    const card = await screen.findByRole('button', { name: /^Daily Sketch/ });
    expect(card).toHaveTextContent('reminders can’t reach you');
    expect(card).not.toHaveTextContent('reminders on');
    // The list said so itself: nothing more to ask.
    expect(store.api.getSettings).not.toHaveBeenCalled();
  });

  it('learns from a series’ settings that its reminders are not arriving', async () => {
    store.api.listMySeries.mockResolvedValue([
      mine(),
      mine({ id: 2, name: 'Coffee', reminder_enabled: false }),
      mine({ id: 3, name: 'Walks', cadence: 'freeform' }),
      mine({ id: 4, name: 'Old', state: 'revoked' }),
    ]);
    store.api.getSettings.mockResolvedValue({
      reminder_enabled: true,
      reminder_error: 'dm_closed',
    });
    render(MySeries);

    const card = await screen.findByRole('button', { name: /^Daily Sketch/ });
    await waitFor(() => expect(card).toHaveTextContent('reminders can’t reach you'));
    expect(card).not.toHaveTextContent('reminders on');
    // Only a series that leaf sends reminders for is asked about.
    expect(store.api.getSettings).toHaveBeenCalledTimes(1);
    expect(store.api.getSettings).toHaveBeenCalledWith('g1', 1);
  });

  it('keeps saying reminders are on when the settings could not be checked', async () => {
    store.api.getSettings.mockRejectedValue(new ApiError(500, 'GET → 500'));
    render(MySeries);
    const card = await screen.findByRole('button', { name: /^Daily Sketch/ });
    await waitFor(() => expect(store.api.getSettings).toHaveBeenCalled());
    expect(card).toHaveTextContent('reminders on');
  });

  it('offers to start a series when there are none and the member may', async () => {
    store.api.listMySeries.mockResolvedValue([]);
    render(MySeries);
    await fireEvent.click(await screen.findByRole('button', { name: 'Start a series' }));
    expect(nav.current).toEqual({ name: 'createSeries' });
  });

  it('offers Try again when the list could not load', async () => {
    store.api.listMySeries.mockRejectedValueOnce(new ApiError(500, 'GET → 500'));
    render(MySeries);
    expect(await screen.findByText('Couldn’t load your series')).toBeInTheDocument();
    expect(screen.queryByText(/reopening the gallery/)).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('button', { name: /^Daily Sketch/ })).toBeInTheDocument();
  });
});
