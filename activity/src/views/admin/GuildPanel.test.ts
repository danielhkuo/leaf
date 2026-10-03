import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AdminApiError, type AdminApi } from '../../lib/admin/client';
import type {
  AdminGuildDetail,
  AdminOptions,
  AdminSeries,
  AdminSettings,
  SeriesPatch,
  SettingsPatch,
} from '../../lib/admin/schemas';
import { stashDraft, takeDraft } from '../../lib/admin/session';
import { expectNoA11yViolations } from '../../lib/test/a11y';
import type * as Timezones from '../../lib/utils/timezones';
import GuildPanel from './GuildPanel.svelte';

// Pin the device's zone, and cut the zone list to a handful: the full one
// (400+ options) only slows the axe run down.
vi.mock('../../lib/utils/timezones', async (original) => ({
  ...(await original<typeof Timezones>()),
  deviceTimezone: () => 'Europe/Berlin',
  hasTimezoneList: () => true,
  timezoneOptions: (current?: string | null) => [
    ...new Set(['America/Chicago', 'Europe/Berlin', 'UTC', ...(current ? [current] : [])]),
  ],
}));

const GUILD = '900000000000000009';
const ROLE_ARTIST = '111111111111111111';
const ROLE_MOD = '222222222222222222';
const CHANNEL_LOG = '333333333333333333';

const SETTINGS: AdminSettings = {
  timezone: 'America/Chicago',
  creator_role_id: ROLE_ARTIST,
  log_channel_id: null,
  max_series_per_user: 3,
  min_account_age_days: 30,
  min_membership_age_days: 7,
  sprout_enabled: true,
  sprout_threshold: 3,
};

const SKETCH: AdminSeries = {
  id: 7,
  name: 'Daily Sketch',
  creator_id: '100000000000000001',
  creator_name: 'Mika',
  privacy: 'public',
  privacy_role_id: null,
  state: 'active',
};
const SEEDLING: AdminSeries = {
  id: 8,
  name: 'Seedling',
  creator_id: '100000000000000002',
  privacy: 'public',
  privacy_role_id: null,
  state: 'sprout',
  archived_days: 1,
};
const POLAROIDS: AdminSeries = {
  id: 9,
  name: 'Old Polaroids',
  creator_id: '100000000000000003',
  creator_name: 'Noor',
  privacy: 'creator_only',
  privacy_role_id: null,
  state: 'revoked',
};

const OPTIONS: AdminOptions = {
  roles: [
    { id: ROLE_ARTIST, name: 'Artist' },
    { id: ROLE_MOD, name: 'Moderator' },
  ],
  channels: [{ id: CHANNEL_LOG, name: 'leaf-log' }],
};

function detail(extra: Partial<AdminGuildDetail> = {}): AdminGuildDetail {
  return {
    guild_id: GUILD,
    name: 'Walpurgis',
    setup_complete: true,
    settings: SETTINGS,
    series: [SKETCH, SEEDLING, POLAROIDS],
    ...extra,
  };
}

const refused = (status: number, code?: string, message?: string): AdminApiError =>
  new AdminApiError(
    status,
    `x → ${status}`,
    code,
    message === undefined ? {} : { detail: message },
  );

function fakeApi(loaded: AdminGuildDetail = detail(), options: AdminOptions | Error = OPTIONS) {
  return {
    guild: vi.fn<(gid: string) => Promise<AdminGuildDetail>>(() => Promise.resolve(loaded)),
    options: vi.fn<(gid: string) => Promise<AdminOptions>>(() =>
      options instanceof Error ? Promise.reject(options) : Promise.resolve(options),
    ),
    patchSettings: vi.fn<(gid: string, patch: SettingsPatch) => Promise<AdminSettings>>(
      (_gid, patch) => {
        const next = { ...loaded.settings };
        if (patch.max_series_per_user !== undefined) {
          next.max_series_per_user = patch.max_series_per_user;
        }
        if (patch.timezone !== undefined) next.timezone = patch.timezone;
        if (patch.log_channel_id !== undefined) next.log_channel_id = patch.log_channel_id || null;
        if (patch.sprout_enabled !== undefined) next.sprout_enabled = patch.sprout_enabled;
        return Promise.resolve(next);
      },
    ),
    patchSeries: vi.fn<(gid: string, id: number, patch: SeriesPatch) => Promise<AdminSeries>>(
      (_gid, id, patch) => {
        const found = loaded.series.find((s) => s.id === id) ?? SKETCH;
        return Promise.resolve({
          id: found.id,
          name: found.name,
          creator_id: found.creator_id,
          privacy: patch.privacy ?? found.privacy,
          privacy_role_id: patch.privacy_role_id ?? found.privacy_role_id,
          state: patch.state ?? found.state,
        });
      },
    ),
  };
}
type FakeApi = ReturnType<typeof fakeApi>;

interface Extra {
  onUnauthorized?: (draftKept: boolean) => void;
  onDirtyChange?: (dirty: boolean) => void;
  onBack?: () => void;
  onSignIn?: () => void;
}

async function open(api: FakeApi = fakeApi(), extra: Extra = {}) {
  const view = render(GuildPanel, {
    props: { api: api as unknown as AdminApi, guildId: GUILD, ...extra },
  });
  await screen.findByRole('heading', { level: 2, name: 'Settings' });
  // The pickers' lists arrive separately.
  await waitFor(() => expect(screen.queryByText('Loading…')).toBeNull());
  return { api, ...view };
}

const type = (label: string, value: string) =>
  fireEvent.input(screen.getByLabelText(label), { target: { value } });
const pick = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const saveButton = () => screen.getByRole('button', { name: 'Save changes' });
/** The settings form's own status line (each series row has one too). */
const formStatus = () => screen.getByText(/^(Unsaved changes|Saving…|Saved.*)$/);
const row = (name: string) => {
  const item = screen.getByText(name, { selector: '.name' }).closest('li');
  if (!item) throw new Error(`no row for ${name}`);
  return within(item);
};

/** A working Storage. Newer Node defines its own unusable `sessionStorage` global. */
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

beforeEach(() => {
  vi.stubGlobal('sessionStorage', memoryStorage());
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe('GuildPanel', () => {
  it('names the server being edited', async () => {
    await open();
    expect(screen.getByRole('heading', { level: 1, name: 'Walpurgis' })).toBeInTheDocument();
  });

  it('falls back to the server id when Discord gave no name', async () => {
    await open(fakeApi(detail({ name: undefined })));
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(`Server ${GUILD}`);
  });

  it('labels every setting with its own control, help text included', async () => {
    await open();

    const zone = screen.getByLabelText('Timezone');
    expect(zone.tagName).toBe('SELECT');
    expect(zone).toHaveValue('America/Chicago');
    expect(zone).toHaveAccessibleDescription(/gallery calendar/);

    expect(screen.getByLabelText('Series per member')).toHaveValue('3');
    expect(screen.getByLabelText('Minimum Discord account age, in days')).toHaveValue('30');
    expect(screen.getByLabelText('Minimum time in this server, in days')).toHaveValue('7');
    expect(screen.getByLabelText('Sprout stage for new series')).toBeChecked();
    expect(screen.getByLabelText('Days before a sprout is published')).toHaveValue('3');
  });

  it('offers roles and channels by name, never as ids to type', async () => {
    await open();

    const role = screen.getByLabelText('Creator role');
    expect(role.tagName).toBe('SELECT');
    expect(role).toHaveValue(ROLE_ARTIST);
    expect(within(role).getByRole('option', { name: '@Artist' })).toBeInTheDocument();
    expect(within(role).getByRole('option', { name: 'Anyone can start a series' })).toHaveValue('');

    const log = screen.getByLabelText('Log channel');
    expect(log.tagName).toBe('SELECT');
    expect(log).toHaveValue('');
    expect(within(log).getByRole('option', { name: '#leaf-log' })).toHaveValue(CHANNEL_LOG);
  });

  it('keeps a saved role Discord no longer lists on offer', async () => {
    const gone = '999999999999990042';
    await open(fakeApi(detail({ settings: { ...SETTINGS, creator_role_id: gone } })));

    const role = screen.getByLabelText('Creator role');
    expect(role).toHaveValue(gone);
    expect(
      within(role).getByRole('option', { name: 'A role leaf can’t see (…0042)' }),
    ).toBeInTheDocument();
  });

  it('falls back to id text boxes when the lists can’t be loaded, and can try again', async () => {
    const api = fakeApi(detail(), refused(404, 'not_found'));
    await open(api);

    const role = screen.getByLabelText('Creator role');
    expect(role.tagName).toBe('INPUT');
    expect(role).toHaveValue(ROLE_ARTIST);
    expect(role).toHaveAccessibleDescription(/Paste a role ID/);
    expect(screen.getByLabelText('Log channel').tagName).toBe('INPUT');

    api.options.mockResolvedValueOnce(OPTIONS);
    await fireEvent.click(screen.getByRole('button', { name: 'Load the roles again' }));

    await waitFor(() => expect(screen.getByLabelText('Creator role').tagName).toBe('SELECT'));
  });

  it('uses a text box for just the list Discord did not return', async () => {
    await open(fakeApi(detail(), { channels: OPTIONS.channels, roles_unavailable: true }));

    expect(screen.getByLabelText('Creator role').tagName).toBe('INPUT');
    expect(screen.getByLabelText('Log channel').tagName).toBe('SELECT');
  });

  it('says when the server has not been set up yet', async () => {
    await open(fakeApi(detail({ setup_complete: false })));
    expect(screen.getByText('leaf isn’t set up in this server yet')).toBeInTheDocument();
    expect(screen.getByText(/Run \/setup in the server/)).toBeInTheDocument();
  });

  it('has no accessibility violations', async () => {
    const { container } = await open();
    await expectNoA11yViolations(container);
  });

  it('has none with a question, an unsaved pick and a refused field on screen either', async () => {
    const { container } = await open();
    await fireEvent.click(row('Daily Sketch').getByRole('button', { name: 'Revoke' }));
    await fireEvent.change(row('Seedling').getByLabelText(/Who can see it/), {
      target: { value: 'role_gated' },
    });
    await type('Series per member', '');
    await fireEvent.click(saveButton());

    await expectNoA11yViolations(container);
  });
});

describe('GuildPanel settings', () => {
  it('has nothing to save until something is changed', async () => {
    const onDirtyChange = vi.fn();
    await open(fakeApi(), { onDirtyChange });

    expect(saveButton()).toBeDisabled();
    expect(onDirtyChange).toHaveBeenLastCalledWith(false);

    await type('Series per member', '5');

    expect(saveButton()).toBeEnabled();
    expect(formStatus()).toHaveTextContent('Unsaved changes');
    expect(onDirtyChange).toHaveBeenLastCalledWith(true);
  });

  it('sends only the changed fields, then shows what was stored', async () => {
    const { api } = await open();
    await type('Series per member', '05');
    await pick('Log channel', CHANNEL_LOG);

    await fireEvent.click(saveButton());

    await waitFor(() => expect(formStatus()).toHaveTextContent('Saved'));
    expect(api.patchSettings).toHaveBeenCalledWith(GUILD, {
      max_series_per_user: 5,
      log_channel_id: CHANNEL_LOG,
    });
    expect(formStatus()).toHaveAttribute('role', 'status');
    expect(screen.getByLabelText('Series per member')).toHaveValue('5');
    expect(saveButton()).toBeDisabled();
  });

  it('drops "Saved" as soon as the form is edited again', async () => {
    await open();
    await type('Series per member', '5');
    await fireEvent.click(saveButton());
    await waitFor(() => expect(formStatus()).toHaveTextContent('Saved'));

    await type('Series per member', '6');
    expect(formStatus()).toHaveTextContent('Unsaved changes');
    await type('Series per member', '5');

    expect(screen.queryByText('Saved')).toBeNull();
  });

  it('refuses an emptied number next to the field and next to Save, without a request', async () => {
    const { api } = await open();
    await type('Series per member', '');

    await fireEvent.click(saveButton());

    const field = screen.getByLabelText('Series per member');
    expect(field).toHaveAttribute('aria-invalid', 'true');
    expect(field).toHaveAccessibleDescription(/Enter a whole number, 1 or more\./);
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Not saved. One setting above needs a change.',
    );
    expect(api.patchSettings).not.toHaveBeenCalled();

    await type('Series per member', '4');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('shows the server’s refusal next to Save, in words', async () => {
    const api = fakeApi();
    api.patchSettings.mockRejectedValueOnce(
      refused(422, 'invalid_limit', 'Series per member can’t be more than 50.'),
    );
    await open(api);
    await type('Series per member', '500');

    await fireEvent.click(saveButton());

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Not saved. Series per member can’t be more than 50.',
    );
    expect(screen.getByLabelText('Series per member')).toHaveValue('500');
    expect(saveButton()).toBeEnabled();
  });

  it('puts a refusal that names a field under that field', async () => {
    const api = fakeApi();
    api.patchSettings.mockRejectedValueOnce(refused(422, 'unknown_channel'));
    await open(api);
    await pick('Log channel', CHANNEL_LOG);

    await fireEvent.click(saveButton());

    await waitFor(() =>
      expect(screen.getByLabelText('Log channel')).toHaveAttribute('aria-invalid', 'true'),
    );
    expect(screen.getByLabelText('Log channel')).toHaveAccessibleDescription(
      /leaf can’t find that channel/,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('One setting above needs a change.');
  });

  it('never shows a raw request string for a failed save', async () => {
    const api = fakeApi();
    api.patchSettings.mockRejectedValueOnce(refused(500));
    await open(api);
    await type('Series per member', '5');

    await fireEvent.click(saveButton());

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('leaf had a problem. Try again in a moment.');
    expect(alert).not.toHaveTextContent('→');
  });

  it('does not call a save that got no answer "Not saved": leaf may have stored it', async () => {
    const api = fakeApi();
    api.patchSettings.mockRejectedValueOnce(
      new AdminApiError(0, 'x', undefined, { kind: 'timeout' }),
    );
    await open(api);
    await type('Series per member', '5');

    await fireEvent.click(saveButton());

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(
      'Save not confirmed. leaf is taking too long to answer. Try again in a moment.',
    );
    expect(alert).not.toHaveTextContent('Not saved');
    // Saving again sends the same values, so it is still on offer.
    expect(saveButton()).toBeEnabled();
  });

  it('fills in the device’s timezone on request', async () => {
    const { api } = await open();

    await fireEvent.click(screen.getByRole('button', { name: 'Use my timezone (Europe/Berlin)' }));
    expect(screen.getByLabelText('Timezone')).toHaveValue('Europe/Berlin');
    expect(screen.queryByRole('button', { name: /Use my timezone/ })).toBeNull();

    await fireEvent.click(saveButton());
    await waitFor(() =>
      expect(api.patchSettings).toHaveBeenCalledWith(GUILD, { timezone: 'Europe/Berlin' }),
    );
  });

  it('warns about a stored timezone leaf never understood', async () => {
    await open(fakeApi(detail({ settings: { ...SETTINGS, timezone: 'Chicago time' } })));
    // The check is the browser's, so the page does not speak for leaf.
    expect(screen.getByText(/isn’t a timezone this browser knows/)).toBeInTheDocument();
    expect(screen.queryByText(/isn’t a timezone leaf knows/)).toBeNull();
  });

  it('switches the threshold off with the sprout stage and leaves it out of the save', async () => {
    const { api } = await open();
    await type('Days before a sprout is published', '9');

    await fireEvent.click(screen.getByLabelText('Sprout stage for new series'));

    expect(screen.getByLabelText('Days before a sprout is published')).toBeDisabled();
    await fireEvent.click(saveButton());
    await waitFor(() =>
      expect(api.patchSettings).toHaveBeenCalledWith(GUILD, { sprout_enabled: false }),
    );
  });

  it('says how many sprouts a save published and reloads the rows', async () => {
    const api = fakeApi();
    api.patchSettings.mockResolvedValueOnce({
      ...SETTINGS,
      sprout_enabled: false,
      sprouts_published: 1,
    });
    await open(api);
    api.guild.mockResolvedValueOnce(
      detail({ series: [SKETCH, { ...SEEDLING, state: 'active' }, POLAROIDS] }),
    );

    await fireEvent.click(screen.getByLabelText('Sprout stage for new series'));
    await fireEvent.click(saveButton());

    await waitFor(() => expect(formStatus()).toHaveTextContent('Saved. 1 sprout was published.'));
    await waitFor(() => expect(row('Seedling').getByText(/Active/)).toBeInTheDocument());
    expect(row('Seedling').queryByRole('button', { name: 'Publish now' })).toBeNull();
  });
});

describe('GuildPanel when the session runs out', () => {
  it('keeps the unsaved settings and hands over to sign-in', async () => {
    const api = fakeApi();
    api.patchSettings.mockRejectedValueOnce(refused(401, 'unauthorized'));
    const onUnauthorized = vi.fn();
    await open(api, { onUnauthorized });
    await type('Series per member', '9');

    await fireEvent.click(saveButton());

    await waitFor(() => expect(onUnauthorized).toHaveBeenCalledWith(true));
    expect(takeDraft(GUILD)).toEqual({ maxSeries: '9' });
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('reports a 401 from a series action too, with the form’s draft kept', async () => {
    const api = fakeApi();
    api.patchSeries.mockRejectedValueOnce(refused(401));
    const onUnauthorized = vi.fn();
    await open(api, { onUnauthorized });
    await type('Minimum time in this server, in days', '14');

    await fireEvent.click(row('Old Polaroids').getByRole('button', { name: 'Restore' }));

    await waitFor(() => expect(onUnauthorized).toHaveBeenCalledWith(true));
    expect(takeDraft(GUILD)).toEqual({ minMembershipAge: '14' });
  });

  it('does not promise the settings back when the browser would not keep them', async () => {
    const api = fakeApi();
    api.patchSettings.mockRejectedValueOnce(refused(401, 'unauthorized'));
    const onUnauthorized = vi.fn();
    await open(api, { onUnauthorized });
    await type('Series per member', '9');
    const locked = (): never => {
      throw new DOMException('denied', 'SecurityError');
    };
    vi.stubGlobal('sessionStorage', { getItem: locked, setItem: locked, removeItem: locked });

    await fireEvent.click(saveButton());

    await waitFor(() => expect(onUnauthorized).toHaveBeenCalledWith(false));
  });

  it('says nothing was kept when there was nothing unsaved', async () => {
    const api = fakeApi();
    api.guild.mockRejectedValueOnce(refused(401));
    const onUnauthorized = vi.fn();
    render(GuildPanel, {
      props: { api: api as unknown as AdminApi, guildId: GUILD, onUnauthorized },
    });

    await waitFor(() => expect(onUnauthorized).toHaveBeenCalledWith(false));
  });

  it('brings the kept settings back over freshly loaded ones', async () => {
    stashDraft(GUILD, { maxSeries: '9' });
    const onDirtyChange = vi.fn();

    await open(fakeApi(), { onDirtyChange });

    expect(screen.getByLabelText('Series per member')).toHaveValue('9');
    expect(screen.getByText('Your unsaved changes are back')).toBeInTheDocument();
    expect(saveButton()).toBeEnabled();
    expect(onDirtyChange).toHaveBeenLastCalledWith(true);
    // Read once: a reload starts from what is saved.
    expect(takeDraft(GUILD)).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Discard them' }));

    expect(screen.getByLabelText('Series per member')).toHaveValue('3');
    expect(screen.queryByText('Your unsaved changes are back')).toBeNull();
    expect(saveButton()).toBeDisabled();
  });
});

describe('GuildPanel loading', () => {
  it('offers Try again when the server does not load, and loads on retry', async () => {
    const api = fakeApi();
    api.guild.mockRejectedValueOnce(refused(500));
    render(GuildPanel, { props: { api: api as unknown as AdminApi, guildId: GUILD } });

    expect(await screen.findByText('Couldn’t load this server')).toBeInTheDocument();
    expect(screen.getByText('leaf had a problem. Try again in a moment.')).toBeInTheDocument();
    expect(screen.queryByText(/→/)).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    expect(await screen.findByRole('heading', { level: 2, name: 'Series' })).toBeInTheDocument();
    expect(api.guild).toHaveBeenCalledTimes(2);
  });

  it('offers a fresh sign-in for a server this one no longer covers', async () => {
    const api = fakeApi();
    api.guild.mockRejectedValueOnce(refused(404, 'not_found'));
    const onSignIn = vi.fn();
    const onBack = vi.fn();
    render(GuildPanel, {
      props: { api: api as unknown as AdminApi, guildId: GUILD, onSignIn, onBack },
    });

    expect(await screen.findByText('This server isn’t available')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Sign in again' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Choose another server' }));

    expect(onSignIn).toHaveBeenCalledOnce();
    expect(onBack).toHaveBeenCalledOnce();
  });
});

describe('GuildPanel series', () => {
  it('names each creator and shows a sprout’s progress', async () => {
    await open();

    expect(row('Daily Sketch').getByText('by Mika · Active')).toBeInTheDocument();
    expect(
      row('Seedling').getByText('by member 100000000000000002 · 🌱 Sprout · 1 of 3 days archived'),
    ).toBeInTheDocument();
    expect(row('Old Polaroids').getByText('by Noor · Revoked')).toBeInTheDocument();
  });

  it('says so when there are no series', async () => {
    await open(fakeApi(detail({ series: [] })));
    expect(screen.getByText(/No series yet/)).toBeInTheDocument();
  });

  it('does not change who can see a series until Save is pressed', async () => {
    const { api } = await open();
    const sketch = row('Daily Sketch');

    await fireEvent.change(sketch.getByLabelText(/Who can see it/), {
      target: { value: 'creator_only' },
    });

    expect(api.patchSeries).not.toHaveBeenCalled();
    expect(
      sketch.getByText('Hide “Daily Sketch” from everyone except its creator?'),
    ).toBeInTheDocument();

    await fireEvent.click(sketch.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(sketch.getByRole('status')).toHaveTextContent('Saved'));
    expect(api.patchSeries).toHaveBeenCalledWith(GUILD, 7, { privacy: 'creator_only' });
    expect(sketch.getByLabelText(/Who can see it/)).toHaveValue('creator_only');
    expect(sketch.queryByRole('button', { name: 'Save' })).toBeNull();
    // The answer carried no name; the one from the list is kept.
    expect(sketch.getByText('by Mika · Active')).toBeInTheDocument();
  });

  it('puts the select back when Cancel is pressed', async () => {
    const { api } = await open();
    const sketch = row('Daily Sketch');
    await fireEvent.change(sketch.getByLabelText(/Who can see it/), {
      target: { value: 'creator_only' },
    });

    await fireEvent.click(sketch.getByRole('button', { name: 'Cancel' }));

    expect(sketch.getByLabelText(/Who can see it/)).toHaveValue('public');
    expect(api.patchSeries).not.toHaveBeenCalled();
  });

  it('rolls the select back and says why when a change that got no answer isn’t stored', async () => {
    const api = fakeApi();
    api.patchSeries.mockRejectedValueOnce(
      new AdminApiError(0, 'x', undefined, { kind: 'network' }),
    );
    await open(api);
    const sketch = row('Daily Sketch');
    await fireEvent.change(sketch.getByLabelText(/Who can see it/), {
      target: { value: 'creator_only' },
    });

    await fireEvent.click(sketch.getByRole('button', { name: 'Save' }));

    expect(await sketch.findByRole('alert')).toHaveTextContent(
      'Who can see “Daily Sketch” wasn’t changed. Can’t reach leaf. Check your connection and try again.',
    );
    expect(sketch.getByLabelText(/Who can see it/)).toHaveValue('public');
    expect(sketch.getByRole('status').textContent).toBe('');
    // "Wasn't changed" comes from reading the series again, not from the silence.
    expect(api.guild).toHaveBeenCalledTimes(2);
    expect(sketch.getByLabelText(/Who can see it/)).toBeEnabled();
    expect(sketch.queryByRole('button', { name: 'Check again' })).toBeNull();
    // The failure is the row's own: the settings form has no alert.
    expect(screen.getAllByRole('alert')).toHaveLength(1);
  });

  it('confirms a change whose answer was lost once leaf shows it stored', async () => {
    const api = fakeApi();
    await open(api);
    api.patchSeries.mockRejectedValueOnce(
      new AdminApiError(0, 'x', undefined, { kind: 'timeout' }),
    );
    api.guild.mockResolvedValueOnce(
      detail({ series: [{ ...SKETCH, state: 'revoked' }, SEEDLING, POLAROIDS] }),
    );
    const sketch = row('Daily Sketch');
    await fireEvent.click(sketch.getByRole('button', { name: 'Revoke' }));

    await fireEvent.click(
      within(sketch.getByRole('group')).getByRole('button', { name: 'Revoke' }),
    );

    await waitFor(() => expect(sketch.getByRole('status')).toHaveTextContent(/^Revoked\./));
    expect(sketch.queryByRole('alert')).toBeNull();
    expect(sketch.getByText('by Mika · Revoked')).toBeInTheDocument();
    expect(sketch.getByRole('button', { name: 'Restore' })).toBeEnabled();
  });

  it('never says a change failed while it can’t tell, and holds the row until it can', async () => {
    const api = fakeApi();
    const { container } = await open(api);
    api.patchSeries.mockRejectedValueOnce(
      new AdminApiError(0, 'x', undefined, { kind: 'timeout' }),
    );
    api.guild.mockRejectedValueOnce(new AdminApiError(0, 'x', undefined, { kind: 'network' }));
    const sketch = row('Daily Sketch');
    await fireEvent.change(sketch.getByLabelText(/Who can see it/), {
      target: { value: 'creator_only' },
    });

    await fireEvent.click(sketch.getByRole('button', { name: 'Save' }));

    const alert = await sketch.findByRole('alert');
    expect(alert).toHaveTextContent(
      'leaf didn’t answer in time, so this page can’t tell whether the change went through. Check again in a moment.',
    );
    expect(alert).not.toHaveTextContent('wasn’t');
    // Nothing in the row can act on a state nobody knows.
    expect(sketch.getByLabelText(/Who can see it/)).toBeDisabled();
    expect(sketch.getByRole('button', { name: 'Revoke' })).toBeDisabled();
    const again = sketch.getByRole('button', { name: 'Check again' });
    await waitFor(() => expect(again).toHaveFocus());
    await expectNoA11yViolations(container);

    // leaf had stored it all along.
    api.guild.mockResolvedValueOnce(
      detail({ series: [{ ...SKETCH, privacy: 'creator_only' }, SEEDLING, POLAROIDS] }),
    );
    await fireEvent.click(again);

    await waitFor(() => expect(sketch.getByRole('status')).toHaveTextContent('Saved'));
    expect(sketch.queryByRole('alert')).toBeNull();
    expect(sketch.queryByRole('button', { name: 'Check again' })).toBeNull();
    expect(sketch.getByLabelText(/Who can see it/)).toHaveValue('creator_only');
    expect(sketch.getByLabelText(/Who can see it/)).toBeEnabled();
    expect(sketch.getByRole('button', { name: 'Revoke' })).toBeEnabled();
    expect(api.patchSeries).toHaveBeenCalledTimes(1);
  });

  it('keeps asking to check again while the series still can’t be read', async () => {
    const api = fakeApi();
    await open(api);
    const lost = (): AdminApiError => new AdminApiError(0, 'x', undefined, { kind: 'network' });
    api.patchSeries.mockRejectedValueOnce(lost());
    api.guild.mockRejectedValueOnce(lost()).mockRejectedValueOnce(lost());
    const polaroids = row('Old Polaroids');

    await fireEvent.click(polaroids.getByRole('button', { name: 'Restore' }));
    await fireEvent.click(await polaroids.findByRole('button', { name: 'Check again' }));

    await waitFor(() => expect(api.guild).toHaveBeenCalledTimes(3));
    expect(await polaroids.findByRole('alert')).toHaveTextContent(
      'This page can’t reach leaf, so it can’t tell whether the change went through. Check your connection, then check again.',
    );
    expect(polaroids.getByRole('button', { name: 'Check again' })).toBeEnabled();
    expect(polaroids.getByRole('button', { name: 'Restore' })).toBeDisabled();
  });

  it('asks for the role before a series can be limited to one, and sends both', async () => {
    const { api } = await open();
    const sketch = row('Daily Sketch');

    await fireEvent.change(sketch.getByLabelText(/Who can see it/), {
      target: { value: 'role_gated' },
    });

    expect(sketch.getByText('Choose the role that can see “Daily Sketch”.')).toBeInTheDocument();
    expect(sketch.getByRole('button', { name: 'Save' })).toBeDisabled();

    await fireEvent.change(sketch.getByLabelText(/^Role/), { target: { value: ROLE_MOD } });

    expect(
      sketch.getByText('Show “Daily Sketch” only to members with @Moderator?'),
    ).toBeInTheDocument();
    await fireEvent.click(sketch.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(api.patchSeries).toHaveBeenCalledWith(GUILD, 7, {
        privacy: 'role_gated',
        privacy_role_id: ROLE_MOD,
      }),
    );
    await waitFor(() => expect(sketch.getByLabelText(/^Role/)).toHaveValue(ROLE_MOD));
  });

  it('shows which role a role-only series is limited to', async () => {
    const gated: AdminSeries = { ...SKETCH, privacy: 'role_gated', privacy_role_id: ROLE_ARTIST };
    await open(fakeApi(detail({ series: [gated] })));

    // Each row's controls say which series they belong to.
    const role = row('Daily Sketch').getByLabelText('Role that can see Daily Sketch');
    expect(row('Daily Sketch').getByLabelText('Who can see it: Daily Sketch')).toBeInTheDocument();
    expect(role).toHaveValue(ROLE_ARTIST);
    expect(within(role).getByRole('option', { name: '@Artist' })).toBeInTheDocument();
  });

  it('flags a role-only series that has no role', async () => {
    const broken: AdminSeries = { ...SKETCH, privacy: 'role_gated', privacy_role_id: null };
    await open(fakeApi(detail({ series: [broken] })));

    expect(row('Daily Sketch').getByText(/No role is set/)).toBeInTheDocument();
  });

  it('takes a pasted role id when the role list is unavailable', async () => {
    const api = fakeApi(detail(), refused(503, 'discord_unavailable'));
    await open(api);
    const sketch = row('Daily Sketch');
    await fireEvent.change(sketch.getByLabelText(/Who can see it/), {
      target: { value: 'role_gated' },
    });
    const role = sketch.getByLabelText(/^Role/);
    expect(role.tagName).toBe('INPUT');

    await fireEvent.input(role, { target: { value: '1234' } });
    expect(sketch.getByRole('button', { name: 'Save' })).toBeDisabled();

    await fireEvent.input(role, { target: { value: ROLE_MOD } });
    await fireEvent.click(sketch.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(api.patchSeries).toHaveBeenCalledWith(GUILD, 7, {
        privacy: 'role_gated',
        privacy_role_id: ROLE_MOD,
      }),
    );
  });

  it('asks before revoking, and Cancel leaves the series alone', async () => {
    const { api } = await open();
    const sketch = row('Daily Sketch');

    await fireEvent.click(sketch.getByRole('button', { name: 'Revoke' }));

    const question = sketch.getByRole('group', { name: /Hide “Daily Sketch” from the gallery/ });
    await waitFor(() => expect(question).toHaveFocus());
    expect(api.patchSeries).not.toHaveBeenCalled();

    await fireEvent.click(within(question).getByRole('button', { name: 'Cancel' }));

    expect(sketch.queryByRole('group')).toBeNull();
    expect(api.patchSeries).not.toHaveBeenCalled();
    await waitFor(() => expect(sketch.getByRole('button', { name: 'Revoke' })).toHaveFocus());
  });

  it('revokes on the second press and offers Restore', async () => {
    const { api } = await open();
    const sketch = row('Daily Sketch');
    await fireEvent.click(sketch.getByRole('button', { name: 'Revoke' }));

    await fireEvent.click(
      within(sketch.getByRole('group')).getByRole('button', { name: 'Revoke' }),
    );

    await waitFor(() => expect(sketch.getByRole('status')).toHaveTextContent(/^Revoked\./));
    expect(api.patchSeries).toHaveBeenCalledWith(GUILD, 7, { state: 'revoked' });
    expect(sketch.getByText('by Mika · Revoked')).toBeInTheDocument();
    const restore = sketch.getByRole('button', { name: 'Restore' });
    expect(restore).toBeEnabled();
    await waitFor(() => expect(restore).toHaveFocus());
  });

  it('holds the row while a request is out, so a second tap can’t send another', async () => {
    const api = fakeApi();
    let finish: (series: AdminSeries) => void = () => undefined;
    api.patchSeries.mockReturnValueOnce(
      new Promise<AdminSeries>((resolve) => {
        finish = resolve;
      }),
    );
    await open(api);
    const polaroids = row('Old Polaroids');

    await fireEvent.click(polaroids.getByRole('button', { name: 'Restore' }));

    const busy = polaroids.getByRole('button', { name: 'Restoring…' });
    expect(busy).toBeDisabled();
    expect(polaroids.getByLabelText(/Who can see it/)).toBeDisabled();

    finish({ ...POLAROIDS, state: 'active' });
    await waitFor(() => expect(polaroids.getByRole('status')).toHaveTextContent('Restored.'));
    expect(polaroids.getByRole('button', { name: 'Revoke' })).toBeEnabled();
  });

  it('says when a restored series comes back as a sprout', async () => {
    const api = fakeApi();
    api.patchSeries.mockResolvedValueOnce({ ...POLAROIDS, state: 'sprout', archived_days: 1 });
    await open(api);
    const polaroids = row('Old Polaroids');

    await fireEvent.click(polaroids.getByRole('button', { name: 'Restore' }));

    await waitFor(() =>
      expect(polaroids.getByRole('status')).toHaveTextContent(/^Restored as a sprout/),
    );
    expect(api.patchSeries).toHaveBeenCalledWith(GUILD, 9, { state: 'active' });
    expect(polaroids.getByRole('button', { name: 'Publish now' })).toBeInTheDocument();
  });

  it('publishes a sprout on request', async () => {
    const { api } = await open();
    const seedling = row('Seedling');

    await fireEvent.click(seedling.getByRole('button', { name: 'Publish now' }));

    await waitFor(() => expect(seedling.getByRole('status')).toHaveTextContent(/^Published\./));
    expect(api.patchSeries).toHaveBeenCalledWith(GUILD, 8, { state: 'active' });
    expect(seedling.queryByRole('button', { name: 'Publish now' })).toBeNull();
    expect(row('Daily Sketch').queryByRole('button', { name: 'Publish now' })).toBeNull();
  });

  it('does not say Published when leaf answers with the series still a sprout', async () => {
    const api = fakeApi();
    api.patchSeries.mockResolvedValueOnce({ ...SEEDLING, state: 'sprout' });
    await open(api);
    const seedling = row('Seedling');

    await fireEvent.click(seedling.getByRole('button', { name: 'Publish now' }));

    expect(await seedling.findByRole('alert')).toHaveTextContent(
      '“Seedling” wasn’t published. leaf kept it as a sprout.',
    );
    expect(seedling.getByRole('status').textContent).toBe('');
    expect(seedling.getByText(/🌱 Sprout/)).toBeInTheDocument();
    expect(seedling.getByRole('button', { name: 'Publish now' })).toBeEnabled();
  });

  it('reports a failed revoke in the row and keeps the series as it was', async () => {
    const api = fakeApi();
    api.patchSeries.mockRejectedValueOnce(refused(404, 'not_found'));
    await open(api);
    const sketch = row('Daily Sketch');
    await fireEvent.click(sketch.getByRole('button', { name: 'Revoke' }));

    await fireEvent.click(
      within(sketch.getByRole('group')).getByRole('button', { name: 'Revoke' }),
    );

    expect(await sketch.findByRole('alert')).toHaveTextContent(
      '“Daily Sketch” wasn’t revoked. leaf can’t find this series any more. Reload the page to see the current list.',
    );
    expect(sketch.getByText('by Mika · Active')).toBeInTheDocument();
    expect(sketch.getByRole('button', { name: 'Revoke' })).toBeEnabled();
    // leaf said no, so there is nothing to look up.
    expect(api.guild).toHaveBeenCalledTimes(1);
  });
});
