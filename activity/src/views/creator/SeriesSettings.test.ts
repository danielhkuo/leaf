import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { nav } from '../../lib/stores/nav.svelte';
import type { SeriesOptions, SeriesSettings as Settings } from '../../lib/types/api';
import { ApiError } from '../../lib/utils/errors';
import SeriesSettings from './SeriesSettings.svelte';

const store = vi.hoisted(() => {
  const api = { getSettings: vi.fn(), getOptions: vi.fn(), patchSeries: vi.fn() };
  return {
    api,
    gallery: { series: [{ id: 1, max_day: 4, start_day: 1 }] as unknown[] },
    getApi: () => api,
    getGuildId: () => 'g1',
    refreshSeries: vi.fn(),
  };
});
vi.mock('../../lib/stores/gallery.svelte', () => store);

const OPTIONS: SeriesOptions = {
  channels: [{ id: 'c1', name: 'art' }],
  roles: [{ id: 'r1', name: 'Member' }],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: false,
  sprout_threshold: 3,
};

function settings(extra: Partial<Settings> = {}): Settings {
  return {
    id: 1,
    name: 'Daily Sketch',
    description: 'One drawing a day.',
    emoji: '✏️',
    cadence: 'daily',
    privacy: 'public',
    privacy_role_id: null,
    channel_id: 'c1',
    detection_mode: 'context_menu',
    state: 'active',
    reminder_enabled: false,
    reminder_time: null,
    reminder_timezone: null,
    reminder_dm: true,
    start_day: 1,
    reminder_error: undefined,
    reminder_error_at: undefined,
    ...extra,
  };
}

const description = () => screen.findByLabelText('Description');
const save = () => fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
const back = () => fireEvent.click(screen.getByRole('button', { name: 'Back' }));

beforeEach(() => {
  // The view logs each failure it then explains on screen.
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  nav.reset({ name: 'picker' }, { name: 'mySeries' }, { name: 'seriesSettings', seriesId: 1 });
  store.gallery.series = [{ id: 1, max_day: 4, start_day: 1 }];
  store.api.getSettings.mockReset().mockResolvedValue(settings());
  store.api.getOptions.mockReset().mockResolvedValue(OPTIONS);
  store.api.patchSeries.mockReset();
  store.refreshSeries.mockReset().mockResolvedValue(true);
});

describe('SeriesSettings', () => {
  it('focuses its heading and loads the form', async () => {
    render(SeriesSettings, { props: { seriesId: 1 } });
    expect(screen.getByRole('heading', { name: 'Series settings' })).toHaveFocus();
    expect(await description()).toHaveValue('One drawing a day.');
    expect(store.api.getSettings).toHaveBeenCalledWith('g1', 1);
  });

  it('saves only what changed, says so, and refreshes the gallery’s list', async () => {
    store.api.patchSeries.mockResolvedValue(settings({ description: 'Fresh words' }));
    render(SeriesSettings, { props: { seriesId: 1 } });
    await fireEvent.input(await description(), { target: { value: 'Fresh words' } });
    await save();

    expect(store.api.patchSeries).toHaveBeenCalledWith('g1', 1, { description: 'Fresh words' });
    expect(await screen.findByText('Saved')).toHaveAttribute('role', 'status');
    expect(store.refreshSeries).toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Save changes' })).toBeDisabled();
  });

  it('leaves straight away when nothing is unsaved', async () => {
    render(SeriesSettings, { props: { seriesId: 1 } });
    await description();
    await back();
    expect(nav.current).toEqual({ name: 'mySeries' });
  });

  it('asks before leaving with unsaved changes', async () => {
    render(SeriesSettings, { props: { seriesId: 1 } });
    await fireEvent.input(await description(), { target: { value: 'Half done' } });
    await back();

    expect(nav.current).toEqual({ name: 'seriesSettings', seriesId: 1 });
    const prompt = screen.getByRole('group', { name: 'Leave without saving?' });
    expect(prompt).toBeInTheDocument();
    const keep = screen.getByRole('button', { name: 'Keep editing' });
    await waitFor(() => expect(keep).toHaveFocus());

    // A second press of Back is not an answer.
    await back();
    expect(nav.current).toEqual({ name: 'seriesSettings', seriesId: 1 });

    await fireEvent.click(keep);
    expect(screen.queryByRole('group', { name: 'Leave without saving?' })).not.toBeInTheDocument();
    expect(await description()).toHaveValue('Half done');

    await back();
    await fireEvent.click(screen.getByRole('button', { name: 'Discard changes' }));
    expect(nav.current).toEqual({ name: 'mySeries' });
  });

  it('drops the question once the changes are saved', async () => {
    store.api.patchSeries.mockResolvedValue(settings({ description: 'Done' }));
    render(SeriesSettings, { props: { seriesId: 1 } });
    await fireEvent.input(await description(), { target: { value: 'Done' } });
    await back();
    expect(screen.getByRole('group', { name: 'Leave without saving?' })).toBeInTheDocument();

    await save();
    await screen.findByText('Saved');
    expect(screen.queryByRole('group', { name: 'Leave without saving?' })).not.toBeInTheDocument();
    await back();
    expect(nav.current).toEqual({ name: 'mySeries' });
  });

  it('puts a refused save’s message on its field and keeps the edit', async () => {
    store.api.patchSeries.mockRejectedValue(new ApiError(409, 'PATCH → 409', 'name_taken'));
    render(SeriesSettings, { props: { seriesId: 1 } });
    const name = await screen.findByLabelText('Series name');
    await fireEvent.input(name, { target: { value: 'Taken' } });
    await save();

    await waitFor(() => expect(name).toHaveAttribute('aria-invalid', 'true'));
    expect(name).toHaveAccessibleDescription(/already used in this server/);
    expect(name).toHaveValue('Taken');
    expect(screen.queryByText('Saved')).not.toBeInTheDocument();
  });

  it('takes the first day number from the gallery’s list when the settings lack it', async () => {
    store.gallery.series = [{ id: 1, max_day: 4, start_day: 5 }];
    store.api.getSettings.mockResolvedValue(settings({ start_day: undefined }));
    // The answer to the save does not carry it either; the refreshed list does.
    store.api.patchSeries.mockResolvedValue(settings({ start_day: undefined }));
    store.refreshSeries.mockImplementation(() => {
      store.gallery.series = [{ id: 1, max_day: 4, start_day: 3 }];
      return Promise.resolve(true);
    });
    render(SeriesSettings, { props: { seriesId: 1 } });

    const first = await screen.findByLabelText('First day number');
    expect(first).toHaveValue('5');
    expect(screen.getByLabelText('Series name')).toHaveValue('Daily Sketch');

    await fireEvent.input(first, { target: { value: '3' } });
    await save();
    expect(store.api.patchSeries).toHaveBeenCalledWith('g1', 1, { start_day: 3 });
    expect(await screen.findByText('Saved')).toBeInTheDocument();
    expect(first).toHaveValue('3');
    expect(screen.getByRole('button', { name: 'Save changes' })).toBeDisabled();
  });

  it('leaves the first day number out when nothing says what it is', async () => {
    store.gallery.series = [];
    store.api.getSettings.mockResolvedValue(settings({ start_day: undefined }));
    render(SeriesSettings, { props: { seriesId: 1 } });
    expect(await screen.findByLabelText('Series name')).toHaveValue('Daily Sketch');
    expect(screen.queryByLabelText('First day number')).not.toBeInTheDocument();
  });

  it('does not say Saved over a rename the server ignored', async () => {
    // An older server drops the name it was sent and answers 200.
    store.api.patchSeries.mockResolvedValue(settings({ emoji: '🍃' }));
    render(SeriesSettings, { props: { seriesId: 1 } });
    const name = await screen.findByLabelText('Series name');
    await fireEvent.input(name, { target: { value: 'Renamed' } });
    await fireEvent.click(screen.getByRole('button', { name: '🍃' }));
    await save();

    await waitFor(() => expect(name).toHaveAttribute('aria-invalid', 'true'));
    expect(name).toHaveAccessibleDescription(
      /^The name wasn’t changed\..*Your other changes were saved\.$/,
    );
    expect(name).toHaveValue('Renamed');
    expect(screen.queryByText('Saved')).not.toBeInTheDocument();
    expect(screen.getByText('Unsaved changes')).toBeInTheDocument();
    // The emoji did save: only the name is still waiting.
    expect(screen.getByLabelText('Reaction emoji')).toHaveValue('🍃');
  });

  it('does not say Saved over a first day number the server ignored', async () => {
    store.api.getSettings.mockResolvedValue(settings({ start_day: undefined }));
    store.api.patchSeries.mockResolvedValue(settings({ start_day: undefined }));
    render(SeriesSettings, { props: { seriesId: 1 } });

    const first = await screen.findByLabelText('First day number');
    await fireEvent.input(first, { target: { value: '200' } });
    await save();

    // The refreshed list still says Day 1.
    await waitFor(() => expect(first).toHaveAttribute('aria-invalid', 'true'));
    expect(first).toHaveAccessibleDescription(/The first day number wasn’t changed\./);
    expect(screen.queryByText('Saved')).not.toBeInTheDocument();
  });

  it('turns read-only when the series was revoked while the form was open', async () => {
    store.api.patchSeries.mockRejectedValue(new ApiError(403, 'PATCH → 403', 'revoked'));
    render(SeriesSettings, { props: { seriesId: 1 } });
    await fireEvent.input(await description(), { target: { value: 'Too late' } });
    store.api.getSettings.mockResolvedValue(settings({ state: 'revoked' }));
    await save();

    expect(await screen.findByText('A server admin revoked this series')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save changes' })).not.toBeInTheDocument();
    // Nothing is unsaved any more, so Back just leaves.
    await back();
    expect(nav.current).toEqual({ name: 'mySeries' });
  });

  it('only talks about ownership when the server says the series is not there', async () => {
    store.api.getSettings.mockRejectedValue(new ApiError(404, 'GET → 404', 'not_found'));
    render(SeriesSettings, { props: { seriesId: 1 } });
    expect(await screen.findByText('These settings aren’t available')).toBeInTheDocument();
    expect(screen.getByText(/Only the member who started a series/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
  });

  it('offers Try again for a failed load, without guessing at ownership', async () => {
    store.api.getOptions.mockRejectedValueOnce(
      new ApiError(503, 'GET → 503', 'discord_unavailable'),
    );
    render(SeriesSettings, { props: { seriesId: 1 } });
    expect(await screen.findByText('Couldn’t load the settings')).toBeInTheDocument();
    expect(screen.getByText(/can’t reach Discord right now/)).toBeInTheDocument();
    expect(screen.queryByText(/may not own/)).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await description()).toBeInTheDocument();
  });
});
