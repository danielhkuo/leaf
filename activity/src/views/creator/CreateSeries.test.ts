import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { draft, resetDraft } from '../../lib/stores/createDraft.svelte';
import { nav } from '../../lib/stores/nav.svelte';
import { session } from '../../lib/stores/session.svelte';
import type { Session } from '../../lib/sdk/handshake';
import type { SeriesOptions } from '../../lib/types/api';
import { ApiError } from '../../lib/utils/errors';
import CreateSeries from './CreateSeries.svelte';

const store = vi.hoisted(() => {
  const api = { getEligibility: vi.fn(), getOptions: vi.fn(), createSeries: vi.fn() };
  return {
    api,
    getApi: () => api,
    getGuildId: () => 'g1',
    adoptSeries: vi.fn(),
    refreshEligibility: vi.fn(),
    rememberSeries: vi.fn(),
  };
});
vi.mock('../../lib/stores/gallery.svelte', () => store);

const OPTIONS: SeriesOptions = {
  channels: [
    { id: 'c1', name: 'art' },
    { id: 'c2', name: 'daily' },
  ],
  roles: [{ id: 'r1', name: 'Member' }],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: false,
  sprout_threshold: 3,
};

const SESSION: Session = {
  user: { id: 'u1', username: 'kit' },
  guildId: 'g1',
  channelId: 'c2',
  platform: 'mobile',
  customId: null,
  token: 't',
  expiresAt: 0,
};

const CREATED = { id: 7, name: 'Morning Pages', emoji: '🍃' };

async function fillAndStart(name = 'Morning Pages'): Promise<void> {
  await fireEvent.input(await screen.findByLabelText('Series name'), { target: { value: name } });
  await fireEvent.click(screen.getByRole('button', { name: 'Start a series' }));
}

beforeEach(() => {
  // The view logs each failure it then explains on screen.
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  resetDraft();
  nav.reset({ name: 'picker' }, { name: 'createSeries' });
  session.value = { status: 'authed', session: SESSION };
  store.api.getEligibility.mockReset().mockResolvedValue({ can_create: true, violations: [] });
  store.api.getOptions.mockReset().mockResolvedValue(OPTIONS);
  store.api.createSeries.mockReset().mockResolvedValue(CREATED);
  store.adoptSeries.mockReset().mockResolvedValue(true);
  store.refreshEligibility.mockReset().mockResolvedValue(undefined);
  store.rememberSeries.mockReset();
});

describe('CreateSeries', () => {
  it('focuses its heading and preselects the channel leaf was opened in', async () => {
    render(CreateSeries);
    expect(screen.getByRole('heading', { name: 'Start a series' })).toHaveFocus();
    expect(await screen.findByLabelText('Channel')).toHaveValue('c2');
  });

  it('adopts, remembers and opens the new series, and clears the draft', async () => {
    render(CreateSeries);
    await fillAndStart();

    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 7, created: true }));
    expect(store.api.createSeries).toHaveBeenCalledWith(
      'g1',
      expect.objectContaining({ name: 'Morning Pages', channel_id: 'c2' }),
    );
    expect(store.adoptSeries).toHaveBeenCalledWith(CREATED);
    expect(store.rememberSeries).toHaveBeenCalledWith(7);
    expect(store.refreshEligibility).toHaveBeenCalled();
    expect(draft.name).toBe('');
    // Back from the new series' home goes to the list, not to the form.
    nav.back();
    expect(nav.current).toEqual({ name: 'picker' });
  });

  it('puts a taken name’s message on the name field and keeps the draft', async () => {
    store.api.createSeries.mockRejectedValue(new ApiError(409, 'POST → 409', 'name_taken'));
    render(CreateSeries);
    await fillAndStart('Taken');

    const name = screen.getByLabelText('Series name');
    await waitFor(() => expect(name).toHaveAttribute('aria-invalid', 'true'));
    expect(name).toHaveAccessibleDescription(/already used in this server/);
    expect(nav.current).toEqual({ name: 'createSeries' });
    expect(draft.name).toBe('Taken');
    expect(screen.getByRole('button', { name: 'Start a series' })).toBeEnabled();
  });

  it('says a lost connection in plain words, never the request label', async () => {
    store.api.createSeries.mockRejectedValue(
      new ApiError(0, 'POST /guilds/g1/series → no response', undefined, { kind: 'network' }),
    );
    render(CreateSeries);
    await fillAndStart();

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Can’t reach leaf. Check your connection and try again.',
    );
    expect(screen.queryByText(/POST/)).not.toBeInTheDocument();
  });

  it('says what is in the way instead of showing the form', async () => {
    store.api.getEligibility.mockResolvedValue({
      can_create: false,
      violations: [{ code: 'max_series', message: '', params: { limit: 3, current: 3 } }],
    });
    render(CreateSeries);
    expect(await screen.findByText('You can’t start a series here yet')).toBeInTheDocument();
    expect(screen.getByText(/You have 3 of 3 series here/)).toBeInTheDocument();
    expect(screen.queryByLabelText('Series name')).not.toBeInTheDocument();
  });

  it('treats a server that is not set up as that, not as a load failure', async () => {
    store.api.getEligibility.mockRejectedValue(new ApiError(500, 'GET → 500'));
    store.api.getOptions.mockRejectedValue(new ApiError(403, 'GET → 403', 'guild_not_setup'));
    render(CreateSeries);
    expect(await screen.findByText('leaf isn’t set up in this server yet')).toBeInTheDocument();
    expect(screen.getByText(/needs to run \/setup in chat/)).toBeInTheDocument();
  });

  it('still shows the form when only the eligibility check failed', async () => {
    store.api.getEligibility.mockRejectedValue(new ApiError(500, 'GET → 500'));
    render(CreateSeries);
    expect(await screen.findByLabelText('Series name')).toBeInTheDocument();
  });

  it('offers Try again when the form could not load, and loads on the retry', async () => {
    store.api.getOptions.mockRejectedValueOnce(
      new ApiError(0, 'GET → no response', undefined, { kind: 'network' }),
    );
    render(CreateSeries);
    expect(await screen.findByText('Couldn’t load this screen')).toBeInTheDocument();
    expect(screen.queryByText(/reopening the gallery/)).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByLabelText('Series name')).toBeInTheDocument();
  });

  it('reloads the roles in place, keeping what was typed', async () => {
    store.api.getOptions.mockResolvedValueOnce({ ...OPTIONS, roles: [], roles_unavailable: true });
    render(CreateSeries);
    await fireEvent.input(await screen.findByLabelText('Series name'), {
      target: { value: 'Kept' },
    });

    await fireEvent.click(screen.getByRole('button', { name: 'Load roles again' }));
    await waitFor(() => expect(screen.getByLabelText('Only members with a role')).toBeEnabled());
    expect(screen.getByLabelText('Series name')).toHaveValue('Kept');
  });
});
