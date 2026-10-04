// The day viewer's chunk on a stalled connection: its import never settles.
// A file of its own because a module import, once settled, stays settled for
// the rest of the file (Gallery.test.ts needs it to load).
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Session } from '../lib/sdk/handshake';
import { nav } from '../lib/stores/nav.svelte';
import { FULL_SIZE, resizeTo, TILE_SIZE } from '../lib/test/viewport';
import type { Series } from '../lib/types/api';
import Gallery from './Gallery.svelte';

const mocks = vi.hoisted(() => ({
  gallery: {
    status: 'ready',
    series: [] as unknown[],
    error: '',
    errorKind: null,
    eligibility: null,
    eligibilityStatus: 'ready',
    epoch: 0,
    refreshing: false,
  },
  home: vi.fn(),
  picker: vi.fn(),
}));

vi.mock('../lib/stores/gallery.svelte', () => ({
  gallery: mocks.gallery,
  initGallery: () => Promise.resolve(),
  lastSeries: () => null,
  peekThumb: () => null,
  refreshAll: () => Promise.resolve(true),
  rememberSeries: () => undefined,
  takeLaunchIntent: () => Promise.resolve({ seriesId: 1, day: 4 }),
}));
vi.mock('../lib/sdk/actions', () => ({
  closeActivity: () => Promise.resolve(true),
  onForeground: () => () => undefined,
}));
vi.mock('./Home.svelte', () => ({ default: mocks.home }));
vi.mock('./Picker.svelte', () => ({ default: mocks.picker }));
vi.mock('./Viewer.svelte', () => new Promise(() => undefined));

const SERIES: Series = {
  id: 1,
  name: 'S1',
  description: '',
  creator_id: 'u',
  cadence: 'daily',
  emoji: '🍃',
  start_day: 1,
  max_day: 10,
};

const SESSION: Session = {
  user: { id: 'me', username: 'me' },
  guildId: 'g1',
  channelId: null,
  platform: 'mobile',
  appName: 'leaf',
  customId: null,
  token: 't',
  expiresAt: Date.now() + 3_600_000,
};

beforeEach(() => {
  vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined);
  nav.reset({ name: 'picker' });
  mocks.gallery.series = [SERIES];
});

describe('Gallery while the day viewer loads', () => {
  it('can be left with Close', async () => {
    render(Gallery, { props: { session: SESSION } });
    expect(await screen.findByText('Loading the day')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 1 }));
  });

  it('can be left with Escape', async () => {
    render(Gallery, { props: { session: SESSION } });
    expect(await screen.findByText('Loading the day')).toBeInTheDocument();

    await fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 1 }));
  });

  it('is not left by an Escape pressed while leaf is a tile', async () => {
    render(Gallery, { props: { session: SESSION } });
    expect(await screen.findByText('Loading the day')).toBeInTheDocument();

    try {
      resizeTo(TILE_SIZE);
      await fireEvent.keyDown(window, { key: 'Escape' });
      expect(nav.current).toEqual({ name: 'viewer', seriesId: 1, day: 4 });
    } finally {
      resizeTo(FULL_SIZE);
    }
    await fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(nav.current).toEqual({ name: 'home', seriesId: 1 }));
  });
});
