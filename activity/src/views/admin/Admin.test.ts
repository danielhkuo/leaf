import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as Client from '../../lib/admin/client';
import type {
  AdminGuild,
  AdminGuildDetail,
  AdminOptions,
  AdminSeries,
  AdminSettings,
} from '../../lib/admin/schemas';
import { expectNoA11yViolations } from '../../lib/test/a11y';
import type * as Timezones from '../../lib/utils/timezones';
import Admin from './Admin.svelte';

const api = vi.hoisted(() => ({
  tokens: [] as string[],
  listGuilds: vi.fn<() => Promise<AdminGuild[]>>(),
  guild: vi.fn<(gid: string) => Promise<AdminGuildDetail>>(),
  options: vi.fn<(gid: string) => Promise<AdminOptions>>(),
  patchSettings: vi.fn<(gid: string, patch: object) => Promise<AdminSettings>>(),
  patchSeries: vi.fn<(gid: string, id: number, patch: object) => Promise<AdminSeries>>(),
}));

// The page builds its own client from the stored token: stand in for the
// class, keep the real error type.
vi.mock('../../lib/admin/client', async (original) => ({
  ...(await original<typeof Client>()),
  AdminApi: class {
    constructor(token: string) {
      api.tokens.push(token);
    }
    listGuilds = api.listGuilds;
    guild = api.guild;
    options = api.options;
    patchSettings = api.patchSettings;
    patchSeries = api.patchSeries;
  },
}));

vi.mock('../../lib/utils/timezones', async (original) => ({
  ...(await original<typeof Timezones>()),
  deviceTimezone: () => 'Europe/Berlin',
  hasTimezoneList: () => true,
  timezoneOptions: (current?: string | null) => [
    ...new Set(['Europe/Berlin', 'UTC', ...(current ? [current] : [])]),
  ],
}));

const { AdminApiError } = await import('../../lib/admin/client');

const ONE = '900000000000000001';
const TWO = '900000000000000002';
const GUILDS: AdminGuild[] = [
  { guild_id: ONE, series_count: 3, name: 'Walpurgis' },
  { guild_id: TWO, series_count: 1 },
];

const SETTINGS: AdminSettings = {
  timezone: 'UTC',
  creator_role_id: null,
  log_channel_id: null,
  max_series_per_user: 3,
  min_account_age_days: 0,
  min_membership_age_days: 0,
  sprout_enabled: false,
  sprout_threshold: 3,
};

function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => void items.delete(key),
    setItem: (key, value) => void items.set(key, String(value)),
  };
}

const refused = (status: number): Error => new AdminApiError(status, `x → ${status}`);

/** Loads the page at `url` (path, query and fragment), signed in unless `token` is null. */
function visit(url: string, token: string | null = 'stored') {
  history.replaceState(null, '', url);
  if (token !== null) localStorage.setItem('leaf:adminToken', token);
  return render(Admin);
}

const heading = (name: string | RegExp) => screen.findByRole('heading', { level: 1, name });
const settingsLoaded = () => screen.findByRole('heading', { level: 2, name: 'Settings' });

beforeEach(() => {
  vi.stubGlobal('localStorage', memoryStorage());
  vi.stubGlobal('sessionStorage', memoryStorage());
  vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined);
  api.tokens.length = 0;
  api.listGuilds.mockReset().mockResolvedValue(GUILDS);
  api.guild
    .mockReset()
    .mockImplementation((gid) =>
      Promise.resolve({ guild_id: gid, settings: SETTINGS, series: [] }),
    );
  api.options.mockReset().mockResolvedValue({ roles: [], channels: [] });
  api.patchSettings.mockReset().mockResolvedValue(SETTINGS);
  api.patchSeries.mockReset();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  history.replaceState(null, '', '/');
});

describe('Admin sign-in', () => {
  it('offers only the Discord sign-in before there is a session', async () => {
    visit('/admin', null);

    const link = await screen.findByRole('link', { name: 'Sign in with Discord' });
    expect(link).toHaveAttribute('href', '/admin/login');
    expect(screen.queryByRole('button')).toBeNull();
    expect(api.listGuilds).not.toHaveBeenCalled();
  });

  it('takes the token from the fragment, stores it, and keeps the query', async () => {
    visit(`/admin?guild=${TWO}#token=fresh.token`, null);

    await settingsLoaded();

    expect(api.tokens).toEqual(['fresh.token']);
    expect(localStorage.getItem('leaf:adminToken')).toBe('fresh.token');
    expect(location.hash).toBe('');
    expect(location.search).toBe(`?guild=${TWO}`);
    expect(api.guild).toHaveBeenCalledWith(TWO);
  });

  it.each([
    ['denied', 'Sign-in cancelled'],
    ['expired', 'That sign-in took too long'],
    ['exchange_failed', 'Discord rejected the sign-in'],
    ['discord_unavailable', 'Discord isn’t answering'],
    ['no_guilds', 'No server to manage'],
  ])('explains a sign-in that came back with #error=%s', async (code, title) => {
    // A token left over from an earlier session must not hide the failure.
    visit(`/admin#error=${code}`, 'stale');

    expect(await heading(title)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Sign in again' })).toHaveAttribute(
      'href',
      '/admin/login',
    );
    expect(location.hash).toBe('');
    expect(api.listGuilds).not.toHaveBeenCalled();
  });

  it('names the redirect to register when Discord rejects the sign-in', async () => {
    visit('/admin#error=exchange_failed', null);
    expect(await screen.findByText(/\/admin\/callback is listed under/)).toHaveTextContent(
      `${location.origin}/admin/callback`,
    );
  });

  it('asks for a new sign-in when the stored token has expired', async () => {
    api.listGuilds.mockRejectedValueOnce(refused(401));
    visit('/admin');

    expect(await heading('Your session expired')).toBeInTheDocument();
    expect(localStorage.getItem('leaf:adminToken')).toBeNull();
    expect(screen.queryByText(/→/)).toBeNull();
  });

  it('says what is missing when the account manages no server with leaf', async () => {
    api.listGuilds.mockResolvedValueOnce([]);
    visit('/admin');

    expect(await heading('No server to manage')).toBeInTheDocument();
    expect(screen.getByText(/Manage Server permission/)).toBeInTheDocument();
    expect(localStorage.getItem('leaf:adminToken')).toBeNull();
  });

  it('comes back to the linked server after the sign-in round trip', async () => {
    const first = visit(`/admin?guild=${TWO}`, null);
    const link = await screen.findByRole('link', { name: 'Sign in with Discord' });
    // The test page can't follow the link; only its click handler matters.
    link.addEventListener('click', (e) => e.preventDefault());
    await fireEvent.click(link);
    first.unmount();

    // Discord sends the browser back to bare /admin with the token.
    visit('/admin#token=fresh', null);

    await settingsLoaded();
    expect(api.guild).toHaveBeenCalledWith(TWO);
    expect(location.search).toBe(`?guild=${TWO}`);
  });
});

describe('Admin server list', () => {
  it('lists servers by name with their series counts', async () => {
    visit('/admin');

    expect(await heading('Choose a server')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Walpurgis.*3 series/ })).toBeInTheDocument();
    // No name from Discord: the id is the fallback.
    expect(
      screen.getByRole('button', { name: new RegExp(`Server ${TWO}.*1 series`) }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Switch server' })).toBeNull();
  });

  it('has no accessibility violations on the sign-in card or the server list', async () => {
    const signedOut = visit('/admin#error=no_guilds', null);
    await heading('No server to manage');
    await expectNoA11yViolations(signedOut.container);
    signedOut.unmount();

    const { container } = visit('/admin');
    await heading('Choose a server');
    await expectNoA11yViolations(container);
  });

  it('opens a server into the URL, and Switch server comes back', async () => {
    visit('/admin');
    await fireEvent.click(await screen.findByRole('button', { name: /Walpurgis/ }));

    const title = await heading('Walpurgis');
    await waitFor(() => expect(title).toHaveFocus());
    expect(location.search).toBe(`?guild=${ONE}`);

    await fireEvent.click(screen.getByRole('button', { name: 'Switch server' }));

    expect(await heading('Choose a server')).toBeInTheDocument();
    expect(location.search).toBe('');
  });

  it('follows the browser’s Back button', async () => {
    visit('/admin');
    await fireEvent.click(await screen.findByRole('button', { name: /Walpurgis/ }));
    await settingsLoaded();

    history.back();

    expect(await heading('Choose a server')).toBeInTheDocument();
    expect(location.search).toBe('');
  });

  it('opens the only server straight away, with no Switch server', async () => {
    api.listGuilds.mockResolvedValueOnce([GUILDS[0]!]);
    visit('/admin');

    expect(await heading('Walpurgis')).toBeInTheDocument();
    expect(location.search).toBe(`?guild=${ONE}`);
    expect(screen.queryByRole('button', { name: 'Switch server' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeInTheDocument();
  });

  it('reopens the server named in the URL after a refresh', async () => {
    visit(`/admin?guild=${TWO}`);

    await settingsLoaded();
    expect(api.guild).toHaveBeenCalledWith(TWO);
    expect(screen.getByRole('button', { name: 'Switch server' })).toBeInTheDocument();
  });

  it('says so when a link names a server this sign-in can’t manage', async () => {
    visit('/admin?guild=123456');

    expect(await heading('Choose a server')).toBeInTheDocument();
    expect(screen.getByText(/server this sign-in can’t manage/)).toBeInTheDocument();
    expect(location.search).toBe('');
    expect(api.guild).not.toHaveBeenCalled();
  });

  it('offers Try again when the list does not load, and signs out on request', async () => {
    api.listGuilds.mockRejectedValueOnce(refused(503));
    visit('/admin');

    expect(await screen.findByText('Couldn’t load your servers')).toBeInTheDocument();
    expect(screen.getByText('leaf had a problem. Try again in a moment.')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await heading('Choose a server')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
    expect(await screen.findByRole('link', { name: 'Sign in with Discord' })).toBeInTheDocument();
    expect(localStorage.getItem('leaf:adminToken')).toBeNull();
  });
});

describe('Admin with unsaved settings', () => {
  async function editing() {
    visit(`/admin?guild=${ONE}`);
    await settingsLoaded();
    await fireEvent.input(screen.getByLabelText('Series per member'), { target: { value: '9' } });
  }
  const question = () => screen.queryByRole('group', { name: /unsaved changes/ });

  it('asks before Switch server throws them away', async () => {
    await editing();

    screen.getByRole('button', { name: 'Switch server' }).focus();
    await fireEvent.click(screen.getByRole('button', { name: 'Switch server' }));

    await waitFor(() => expect(question()).toHaveFocus());
    expect(screen.getByLabelText('Series per member')).toHaveValue('9');
    await expectNoA11yViolations(document.body);

    await fireEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
    expect(question()).toBeNull();
    expect(screen.getByLabelText('Series per member')).toHaveValue('9');
    expect(screen.getByRole('button', { name: 'Switch server' })).toHaveFocus();

    await fireEvent.click(screen.getByRole('button', { name: 'Switch server' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Discard changes' }));
    expect(await heading('Choose a server')).toBeInTheDocument();
  });

  it('asks before Sign out throws them away', async () => {
    await editing();

    await fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
    expect(localStorage.getItem('leaf:adminToken')).toBe('stored');

    await fireEvent.click(screen.getByRole('button', { name: 'Discard changes' }));
    expect(await screen.findByRole('link', { name: 'Sign in with Discord' })).toBeInTheDocument();
    expect(localStorage.getItem('leaf:adminToken')).toBeNull();
    expect(location.search).toBe('');
  });

  it('stays on the panel when Back is pressed, until the admin decides', async () => {
    visit('/admin');
    await fireEvent.click(await screen.findByRole('button', { name: /Walpurgis/ }));
    await settingsLoaded();
    await fireEvent.input(screen.getByLabelText('Series per member'), { target: { value: '9' } });

    history.back();

    await waitFor(() => expect(question()).not.toBeNull());
    expect(location.search).toBe(`?guild=${ONE}`);
    expect(screen.getByLabelText('Series per member')).toHaveValue('9');

    await fireEvent.click(screen.getByRole('button', { name: 'Discard changes' }));
    expect(await heading('Choose a server')).toBeInTheDocument();
    expect(location.search).toBe('');
  });

  it('has the browser confirm closing the tab, and only while something is unsaved', async () => {
    visit(`/admin?guild=${ONE}`);
    await settingsLoaded();
    const closing = (): boolean => {
      const event = new Event('beforeunload', { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    };

    expect(closing()).toBe(false);
    await fireEvent.input(screen.getByLabelText('Series per member'), { target: { value: '9' } });
    await waitFor(() => expect(closing()).toBe(true));
  });

  it('leaves without asking when nothing was changed', async () => {
    visit(`/admin?guild=${ONE}`);
    await settingsLoaded();

    await fireEvent.click(screen.getByRole('button', { name: 'Switch server' }));

    expect(await heading('Choose a server')).toBeInTheDocument();
  });

  it('keeps them through an expired session and brings them back after sign-in', async () => {
    await editing();
    api.patchSettings.mockRejectedValueOnce(refused(401));

    await fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

    const title = await heading('Your session expired');
    await waitFor(() => expect(title).toHaveFocus());
    expect(screen.getByText(/unsaved settings are kept/)).toBeInTheDocument();
    expect(localStorage.getItem('leaf:adminToken')).toBeNull();
    // Nothing is unsaved on screen any more, so leaving for Discord is not blocked.
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);

    const link = screen.getByRole('link', { name: 'Sign in again' });
    link.addEventListener('click', (e) => e.preventDefault());
    await fireEvent.click(link);
    document.body.replaceChildren();

    visit('/admin#token=renewed', null);

    await settingsLoaded();
    expect(api.guild).toHaveBeenLastCalledWith(ONE);
    expect(screen.getByLabelText('Series per member')).toHaveValue('9');
    expect(screen.getByText('Your unsaved changes are back')).toBeInTheDocument();
  });
});
