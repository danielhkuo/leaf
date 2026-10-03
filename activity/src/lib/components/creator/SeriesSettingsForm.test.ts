import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import { expectNoA11yViolations } from '../../test/a11y';
import type { SeriesOptions, SeriesSettings, UpdateSeriesInput } from '../../types/api';
import type * as Timezones from '../../utils/timezones';
import SeriesSettingsForm from './SeriesSettingsForm.svelte';

// The reminder defaults follow the device's zone, so pin it. The zone list is
// cut to a handful: the full one (400+ options) only slows the axe run down.
vi.mock('../../utils/timezones', async (original) => ({
  ...(await original<typeof Timezones>()),
  deviceTimezone: () => 'Europe/Berlin',
  timezoneOptions: (current?: string | null) => [
    ...new Set(['America/Chicago', 'Europe/Berlin', 'UTC', ...(current ? [current] : [])]),
  ],
}));

const options: SeriesOptions = {
  channels: [
    { id: 'c1', name: 'art' },
    { id: 'c2', name: 'daily' },
  ],
  roles: [{ id: 'r1', name: 'Member', held: true }],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: true,
  sprout_threshold: 3,
};

function settings(extra: Partial<SeriesSettings> = {}): SeriesSettings {
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

interface Extra {
  options?: SeriesOptions;
  saving?: boolean;
  saved?: boolean;
  error?: string | null;
  errorCode?: string | null;
  hasDays?: boolean;
  onSave?: (patch: UpdateSeriesInput) => unknown;
  onDirtyChange?: (dirty: boolean) => void;
}

function renderForm(saved: SeriesSettings = settings(), extra: Extra = {}) {
  const onSave = vi.fn<(patch: UpdateSeriesInput) => unknown>();
  const view = render(SeriesSettingsForm, {
    props: { settings: saved, options, saving: false, saved: false, error: null, onSave, ...extra },
  });
  return { onSave, ...view };
}

const saveButton = () => screen.getByRole('button', { name: 'Save changes' });
const save = () => fireEvent.click(saveButton());
/** The form's own status line (the role filter has one too, when shown). */
const status = () => screen.getByText(/^(Unsaved changes|Saved|Saving…)$/);

describe('SeriesSettingsForm', () => {
  it('has nothing to save until something is changed', () => {
    const onDirtyChange = vi.fn();
    renderForm(settings(), { onDirtyChange });
    expect(saveButton()).toBeDisabled();
    expect(onDirtyChange).toHaveBeenLastCalledWith(false);
    expect(screen.queryByText('Unsaved changes')).not.toBeInTheDocument();
    // With nothing to say, the save bar rests at the end of the form.
    expect(saveButton().closest('.savebar')).not.toHaveClass('pinned');
  });

  it('offers no passive capture switch', () => {
    renderForm();
    expect(screen.queryByText(/Passive capture/i)).not.toBeInTheDocument();
  });

  it('sends only the field that was changed', async () => {
    const onDirtyChange = vi.fn();
    const { onSave } = renderForm(settings({ channel_id: 'dropped' }), { onDirtyChange });

    await fireEvent.input(screen.getByLabelText('Description'), {
      target: { value: 'Fresh words' },
    });
    expect(status()).toHaveTextContent('Unsaved changes');
    expect(status()).toHaveAttribute('role', 'status');
    expect(onDirtyChange).toHaveBeenLastCalledWith(true);

    await save();
    expect(onSave).toHaveBeenCalledWith({ description: 'Fresh words' });
  });

  it('explains a channel the server no longer allows without blocking other edits', () => {
    renderForm(settings({ channel_id: 'dropped' }));
    expect(screen.getByLabelText('Channel')).toHaveValue('dropped');
    expect(screen.getByText(/no longer one this server allows for series/)).toBeInTheDocument();
    expect(saveButton()).toBeDisabled();
  });

  it('renames the series and moves its first day', async () => {
    const { onSave } = renderForm();
    await fireEvent.input(screen.getByLabelText('Series name'), { target: { value: 'Renamed' } });
    await fireEvent.input(screen.getByLabelText('First day number'), { target: { value: '200' } });
    await save();
    expect(onSave).toHaveBeenCalledWith({ name: 'Renamed', start_day: 200 });
  });

  it('always offers the name, and the first day number only when it is known', async () => {
    const { onSave } = renderForm(settings({ start_day: undefined }));
    expect(screen.queryByLabelText('First day number')).not.toBeInTheDocument();
    const name = screen.getByLabelText('Series name');
    expect(name).toHaveValue('Daily Sketch');

    await fireEvent.input(name, { target: { value: 'Renamed' } });
    await save();
    expect(onSave).toHaveBeenCalledWith({ name: 'Renamed' });
  });

  it('holds back a save the server would refuse and points at the field', async () => {
    const { onSave } = renderForm();
    const emoji = screen.getByLabelText('Reaction emoji');
    await fireEvent.input(emoji, { target: { value: 'two words' } });
    await save();

    expect(onSave).not.toHaveBeenCalled();
    expect(emoji).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByText(/single standard emoji/)).toBeInTheDocument();
    await waitFor(() => expect(emoji).toHaveFocus());

    await fireEvent.click(screen.getByRole('button', { name: '🍃' }));
    expect(emoji).toHaveValue('🍃');
    expect(emoji).not.toHaveAttribute('aria-invalid');
    await save();
    expect(onSave).toHaveBeenCalledWith({ emoji: '🍃' });
  });

  it('never chooses a role for the creator', async () => {
    const { onSave } = renderForm();
    await fireEvent.change(screen.getByLabelText('Who can see it'), {
      target: { value: 'role_gated' },
    });
    const role = screen.getByLabelText('Role');
    expect(role).toHaveValue('');
    await save();
    expect(onSave).not.toHaveBeenCalled();
    expect(role).toHaveAccessibleDescription('Choose the role that can view this series.');

    await fireEvent.change(role, { target: { value: 'r1' } });
    await save();
    expect(onSave).toHaveBeenCalledWith({ privacy: 'role_gated', privacy_role_id: 'r1' });
  });

  it('fills in a time and the device’s timezone when reminders are first turned on', async () => {
    const { onSave } = renderForm();
    expect(screen.queryByLabelText('Time')).not.toBeInTheDocument();

    await fireEvent.click(screen.getByLabelText('Remind me when I’m behind'));
    const time = screen.getByLabelText('Time');
    expect(time).toHaveValue('20:00');
    expect(time).toBeRequired();
    expect(screen.queryByText(/24h/)).not.toBeInTheDocument();
    expect(screen.getByLabelText('Timezone')).toHaveValue('Europe/Berlin');
    expect(screen.getByText(/It nudges once, then stays quiet/)).toHaveTextContent(
      'Europe/Berlin time',
    );

    await save();
    expect(onSave).toHaveBeenCalledWith({
      reminder_enabled: true,
      reminder_time: '20:00',
      reminder_timezone: 'Europe/Berlin',
    });
  });

  it('clears the timezone override when the server default is chosen', async () => {
    const { onSave } = renderForm(
      settings({ reminder_enabled: true, reminder_time: '21:00', reminder_timezone: 'UTC' }),
    );
    const zone = screen.getByLabelText('Timezone');
    expect(zone).toHaveValue('UTC');
    await fireEvent.change(zone, { target: { value: '' } });
    expect(screen.getByText(/It nudges once/)).toHaveTextContent('America/Chicago time');
    await save();
    expect(onSave).toHaveBeenCalledWith({ reminder_timezone: '' });
  });

  it('needs a time before reminders can be saved on', async () => {
    const { onSave } = renderForm();
    await fireEvent.click(screen.getByLabelText('Remind me when I’m behind'));
    const time = screen.getByLabelText('Time');
    await fireEvent.input(time, { target: { value: '' } });
    await save();
    expect(onSave).not.toHaveBeenCalled();
    expect(time).toHaveAccessibleDescription('Choose a reminder time before turning reminders on.');
  });

  it('says a channel ping leaves out the name of a series others can’t see', async () => {
    renderForm(
      settings({ privacy: 'creator_only', reminder_enabled: true, reminder_time: '20:00' }),
    );
    expect(screen.queryByText(/doesn’t name this series/)).not.toBeInTheDocument();
    await fireEvent.click(screen.getByLabelText('A ping in #art'));
    const hint = screen.getByText(/doesn’t name this series/);
    expect(hint).toHaveTextContent('The ping mentions you in #art and says “your series”.');
    expect(hint).not.toHaveClass('caution');
    expect(screen.queryByText(/A ping names this series/)).not.toBeInTheDocument();
  });

  it('says nothing about naming when everyone can see the series', async () => {
    renderForm(settings({ privacy: 'public', reminder_enabled: true, reminder_time: '20:00' }));
    await fireEvent.click(screen.getByLabelText('A ping in #art'));
    expect(screen.queryByText(/doesn’t name this series/)).not.toBeInTheDocument();
  });

  it('shows a freeform series’ reminder as off and says saving turns it off', async () => {
    const { onSave } = renderForm(settings({ reminder_enabled: true, reminder_time: '20:00' }));
    const remind = screen.getByLabelText('Remind me when I’m behind');
    expect(remind).toBeChecked();

    await fireEvent.change(screen.getByLabelText('How often you post'), {
      target: { value: 'freeform' },
    });
    expect(remind).not.toBeChecked();
    expect(remind).toBeDisabled();
    expect(screen.getByText(/Saving turns this series’ reminders off/)).toBeInTheDocument();
    await save();
    expect(onSave).toHaveBeenCalledWith({ cadence: 'freeform', reminder_enabled: false });
  });

  it('says why the last reminder did not arrive, while reminders are on', () => {
    const undelivered = {
      reminder_time: '20:00',
      reminder_error: 'dm_closed',
      reminder_error_at: 1_760_000_000,
    };
    const on = renderForm(settings({ ...undelivered, reminder_enabled: true }));
    expect(screen.getByText('leaf couldn’t deliver your last reminder')).toBeInTheDocument();
    expect(screen.getByText(/wouldn’t let leaf send you a DM/)).toBeInTheDocument();
    on.unmount();

    renderForm(settings({ ...undelivered, reminder_enabled: false }));
    expect(screen.queryByText('leaf couldn’t deliver your last reminder')).not.toBeInTheDocument();
  });

  it('is read-only, with the reason, for a revoked series', () => {
    renderForm(settings({ state: 'revoked' }));
    expect(screen.getByText('A server admin revoked this series')).toBeInTheDocument();
    expect(screen.getByLabelText('Description')).toBeDisabled();
    expect(screen.getByLabelText('Series name')).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Save changes' })).not.toBeInTheDocument();
  });

  it('says a sprout stays hidden whatever privacy is chosen', () => {
    renderForm(settings({ state: 'sprout' }));
    expect(screen.getByLabelText('Who can see it')).toHaveAccessibleDescription(
      /only you can see this series until 3 days are archived/,
    );
  });

  it('shows a server refusal under its field, and any other next to Save', async () => {
    const view = renderForm();
    await fireEvent.input(screen.getByLabelText('Series name'), { target: { value: 'Taken' } });
    await save();
    await view.rerender({ error: 'That name is already used.', errorCode: 'name_taken' });

    const name = screen.getByLabelText('Series name');
    expect(name).toHaveAccessibleDescription('That name is already used.');
    await waitFor(() => expect(name).toHaveFocus());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();

    await fireEvent.input(name, { target: { value: 'Other' } });
    expect(screen.queryByText('That name is already used.')).not.toBeInTheDocument();

    await save();
    await view.rerender({ error: 'Can’t reach leaf.', errorCode: null });
    expect(screen.getByRole('alert')).toHaveTextContent('Can’t reach leaf.');
  });

  it('shows a rename the server did not apply under the name, still unsaved', async () => {
    const view = renderForm();
    const name = screen.getByLabelText('Series name');
    await fireEvent.input(name, { target: { value: 'Renamed' } });
    await save();
    await view.rerender({ error: 'The name wasn’t changed.', errorCode: 'name_not_saved' });

    expect(name).toHaveAccessibleDescription('The name wasn’t changed.');
    await waitFor(() => expect(name).toHaveFocus());
    expect(name).toHaveValue('Renamed');
    expect(status()).toHaveTextContent('Unsaved changes');
  });

  it('announces a save and shows what the server stored', async () => {
    const stored = settings({ name: 'Renamed' });
    // The parent's part: store the save, then hand the form the result.
    let parent: (props: { settings: SeriesSettings; saved: boolean }) => Promise<void> = () =>
      Promise.resolve();
    const onSave = vi.fn(async () => {
      await parent({ settings: stored, saved: true });
      return true;
    });
    const view = renderForm(settings(), { onSave });
    parent = (props) => view.rerender(props);

    const name = screen.getByLabelText('Series name');
    await fireEvent.input(name, { target: { value: '  Renamed  ' } });
    await save();

    const bar = status().closest('.savebar');
    expect(bar).toHaveClass('pinned');

    await waitFor(() => expect(status()).toHaveTextContent('Saved'));
    expect(name).toHaveValue('Renamed');
    expect(saveButton()).toBeDisabled();
    // "Saved" stays where Save was pressed: the bar does not drop back to
    // the end of the form the moment there is nothing left to save.
    expect(bar).toHaveClass('pinned');

    // "Saved" does not linger over a new edit.
    await fireEvent.input(name, { target: { value: 'Renamed again' } });
    expect(status()).toHaveTextContent('Unsaved changes');
  });

  it('has no accessibility violations with reminders and errors showing', async () => {
    const { container } = renderForm(
      settings({
        privacy: 'role_gated',
        reminder_enabled: true,
        reminder_time: '20:00',
        reminder_error: 'no_permission',
      }),
    );
    await fireEvent.input(screen.getByLabelText('Description'), { target: { value: 'x' } });
    await save();
    await expectNoA11yViolations(container);
  });
});
