import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { draft, resetDraft } from '../../stores/createDraft.svelte';
import { expectNoA11yViolations } from '../../test/a11y';
import type { SeriesOptions } from '../../types/api';
import CreateWizard from './CreateWizard.svelte';

const options: SeriesOptions = {
  channels: [
    { id: 'c1', name: 'art' },
    { id: 'c2', name: 'daily' },
  ],
  roles: [
    { id: 'r1', name: 'Member', held: true },
    { id: 'r2', name: 'Staff', held: false },
  ],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: true,
  sprout_threshold: 3,
};

interface Extra {
  options?: SeriesOptions;
  error?: string | null;
  errorCode?: string | null;
  channelId?: string | null;
  onReloadOptions?: () => void;
}

function renderForm(extra: Extra = {}) {
  const onSubmit = vi.fn();
  const view = render(CreateWizard, {
    props: { options, submitting: false, error: null, onSubmit, ...extra },
  });
  return { onSubmit, ...view };
}

const nameField = () => screen.getByLabelText('Series name');
const start = () => fireEvent.click(screen.getByRole('button', { name: 'Start a series' }));

beforeEach(resetDraft);

describe('CreateWizard', () => {
  it('is one form: a name and one press submit the defaults', async () => {
    const { onSubmit } = renderForm();
    expect(screen.queryByRole('button', { name: 'Next' })).not.toBeInTheDocument();

    await fireEvent.input(nameField(), { target: { value: 'Morning Pages' } });
    await start();

    expect(onSubmit).toHaveBeenCalledWith({
      name: 'Morning Pages',
      description: '',
      channel_id: 'c1',
      cadence: 'daily',
      privacy: 'public',
      privacy_role_id: null,
      start_day: 1,
    });
  });

  it('holds back an empty name, says why on the field and moves focus there', async () => {
    const { onSubmit } = renderForm();
    await start();

    expect(onSubmit).not.toHaveBeenCalled();
    const name = nameField();
    expect(name).toHaveAttribute('aria-invalid', 'true');
    expect(name).toHaveAccessibleDescription('Give the series a name.');
    await waitFor(() => expect(name).toHaveFocus());

    // The message goes as soon as the field is edited.
    await fireEvent.input(name, { target: { value: 'Ok' } });
    expect(name).not.toHaveAttribute('aria-invalid');
    expect(screen.queryByText('Give the series a name.')).not.toBeInTheDocument();
  });

  it('scrolls a refused field to the middle of the screen, then its message into view', async () => {
    // jsdom has no scrollIntoView; the form calls it where there is one.
    const scrollIntoView = vi.fn();
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      value: scrollIntoView,
      configurable: true,
    });
    try {
      renderForm();
      await start();
      await waitFor(() => expect(nameField()).toHaveFocus());
      expect(scrollIntoView.mock.calls).toEqual([[{ block: 'center' }], [{ block: 'nearest' }]]);
      expect(scrollIntoView.mock.instances).toEqual([
        nameField(),
        screen.getByText('Give the series a name.'),
      ]);
    } finally {
      Reflect.deleteProperty(Element.prototype, 'scrollIntoView');
    }
  });

  it('does not create the series on Return in the name field', async () => {
    const { onSubmit } = renderForm();
    await fireEvent.input(nameField(), { target: { value: 'Morning Pages' } });
    await fireEvent.keyDown(nameField(), { key: 'Enter' });

    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Start a series' })).toHaveFocus();
  });

  it('never chooses a role for the creator', async () => {
    const { onSubmit } = renderForm();
    await fireEvent.input(nameField(), { target: { value: 'Gated' } });
    await fireEvent.click(screen.getByLabelText('Only members with a role'));

    const role = screen.getByLabelText('Role');
    expect(role).toHaveValue('');
    expect(screen.getByRole('option', { name: '@Member (you have it)' })).toBeInTheDocument();

    await start();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(role).toHaveAccessibleDescription('Choose the role that can view this series.');
    await waitFor(() => expect(role).toHaveFocus());

    await fireEvent.change(role, { target: { value: 'r2' } });
    expect(role).not.toHaveAttribute('aria-invalid');
    expect(screen.getByText(/You don’t have this role yourself/)).toBeInTheDocument();
    await start();
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ privacy: 'role_gated', privacy_role_id: 'r2' }),
    );
  });

  it('preselects the channel leaf was opened in and keeps More options closed', () => {
    renderForm({ channelId: 'c2' });
    expect(screen.getByLabelText('Channel')).toHaveValue('c2');
    expect(screen.getByText('More options').closest('details')).not.toHaveAttribute('open');
    expect(screen.getByText('#daily · Daily · starts at Day 1')).toBeInTheDocument();
  });

  it('opens More options when the channel is only a guess', () => {
    renderForm({ channelId: 'somewhere-else' });
    expect(screen.getByLabelText('Channel')).toHaveValue('c1');
    expect(screen.getByText('More options').closest('details')).toHaveAttribute('open');
  });

  it('leaves the channel out when the server has only one', async () => {
    const { onSubmit } = renderForm({
      options: { ...options, channels: [{ id: 'only', name: 'series' }] },
    });
    expect(screen.queryByLabelText('Channel')).not.toBeInTheDocument();
    await fireEvent.input(nameField(), { target: { value: 'One channel' } });
    await start();
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ channel_id: 'only' }));
  });

  it('cannot be submitted, and says who can fix it, when no channel is set up', async () => {
    const { onSubmit } = renderForm({ options: { ...options, channels: [] } });
    expect(screen.getByText('No channel is set up for series yet')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start a series' })).toBeDisabled();
    await fireEvent.submit(nameField().closest('form')!);
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it('takes a whole day number only, and opens More options to show the problem', async () => {
    const { onSubmit } = renderForm({ channelId: 'c1' });
    await fireEvent.input(nameField(), { target: { value: 'Moved here' } });
    const day = screen.getByLabelText('First day number');
    expect(day).toHaveAttribute('inputmode', 'numeric');

    await fireEvent.input(day, { target: { value: '1.5' } });
    await start();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByText('Enter a whole number from 1 to 999999.')).toBeInTheDocument();
    await waitFor(() => expect(day).toHaveFocus());
    expect(day.closest('details')).toHaveAttribute('open');

    await fireEvent.input(day, { target: { value: '200' } });
    await start();
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ start_day: 200 }));
  });

  it('counts the description and refuses one that is too long', async () => {
    const { onSubmit } = renderForm();
    await fireEvent.input(nameField(), { target: { value: 'Wordy' } });
    const description = screen.getByLabelText(/^Description/);
    expect(description).not.toHaveAttribute('maxlength');
    await fireEvent.input(description, { target: { value: 'x'.repeat(201) } });

    expect(screen.getByText(/^201\/200/)).toBeInTheDocument();
    await start();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(
      screen.getByText('Keep the description to 200 characters or fewer.'),
    ).toBeInTheDocument();
  });

  it('shows a server refusal under the field it is about until that field is edited', async () => {
    const message = 'That name is already used in this server.';
    const { rerender } = renderForm();
    await fireEvent.input(nameField(), { target: { value: 'Taken' } });
    await start();
    await rerender({ error: message, errorCode: 'name_taken' });

    const name = nameField();
    expect(name).toHaveAccessibleDescription(message);
    expect(name).toHaveAttribute('aria-invalid', 'true');
    await waitFor(() => expect(name).toHaveFocus());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();

    await fireEvent.input(name, { target: { value: 'Not taken' } });
    expect(screen.queryByText(message)).not.toBeInTheDocument();
  });

  it('shows a refusal that belongs to no field next to the button, announced', async () => {
    const message = 'You have 3 of 3 series here, which is this server’s limit.';
    const { rerender } = renderForm();
    await fireEvent.input(nameField(), { target: { value: 'One too many' } });
    await start();
    await rerender({ error: message, errorCode: 'max_series' });

    expect(screen.getByRole('alert')).toHaveTextContent(message);
    expect(nameField()).not.toHaveAttribute('aria-invalid');
  });

  it('keeps the draft when the form is left and opened again, and can start over', async () => {
    const first = renderForm();
    await fireEvent.input(nameField(), { target: { value: 'Half typed' } });
    await fireEvent.change(screen.getByLabelText('How often you’ll post'), {
      target: { value: 'weekly' },
    });
    first.unmount();

    renderForm();
    expect(nameField()).toHaveValue('Half typed');
    expect(screen.getByLabelText('How often you’ll post')).toHaveValue('weekly');

    await fireEvent.click(screen.getByRole('button', { name: 'Start over' }));
    expect(nameField()).toHaveValue('');
    expect(draft.cadence).toBe('daily');
    expect(screen.queryByRole('button', { name: 'Start over' })).not.toBeInTheDocument();
    await waitFor(() => expect(nameField()).toHaveFocus());
  });

  it('explains the sprout stage by the privacy chosen', async () => {
    renderForm();
    expect(screen.getByText(/Then everyone in the server can\./)).toBeInTheDocument();

    await fireEvent.click(screen.getByLabelText('Only me'));
    expect(screen.queryByText(/New series start as sprouts/)).not.toBeInTheDocument();

    await fireEvent.click(screen.getByLabelText('Only members with a role'));
    await fireEvent.change(screen.getByLabelText('Role'), { target: { value: 'r1' } });
    expect(screen.getByText(/Then members with the @Member role can\./)).toBeInTheDocument();
  });

  it('says so, and offers a reload, when the role list did not load', async () => {
    const onReloadOptions = vi.fn();
    renderForm({ options: { ...options, roles: [], roles_unavailable: true }, onReloadOptions });

    expect(screen.getByLabelText('Only members with a role')).toBeDisabled();
    expect(screen.getByText(/couldn’t load this server’s roles/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Load roles again' }));
    expect(onReloadOptions).toHaveBeenCalled();
  });

  it('names a channel Discord did not name', () => {
    renderForm({
      options: {
        ...options,
        channels: [
          { id: 'c1', name: 'art' },
          { id: '9876543210', name: null },
        ],
      },
    });
    expect(
      screen.getByRole('option', { name: 'A channel leaf can’t see (…3210)' }),
    ).toBeInTheDocument();
  });

  it('has no accessibility violations, with errors showing', async () => {
    const { container } = renderForm();
    await fireEvent.click(screen.getByLabelText('Only members with a role'));
    await start();
    await expectNoA11yViolations(container);
  });
});
