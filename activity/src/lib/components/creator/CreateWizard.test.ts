import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import type { SeriesOptions } from '../../types/api';
import CreateWizard from './CreateWizard.svelte';

const options: SeriesOptions = {
  channels: [
    { id: 'c1', name: 'art' },
    { id: 'c2', name: 'daily' },
  ],
  roles: [{ id: 'r1', name: 'Member' }],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: true,
  sprout_threshold: 3,
};

function renderWizard() {
  const onSubmit = vi.fn();
  render(CreateWizard, { props: { options, submitting: false, error: null, onSubmit } });
  return onSubmit;
}

describe('CreateWizard', () => {
  it('blocks advancing past an empty name with an inline error', async () => {
    renderWizard();
    await fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('alert')).toHaveTextContent(/between 2 and 40/i);
  });

  it('walks the steps and submits the assembled payload', async () => {
    const onSubmit = renderWizard();

    await fireEvent.input(screen.getByPlaceholderText('Daily sketch'), {
      target: { value: 'Morning Pages' },
    });
    const next = () => fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await next(); // name → channel
    await next(); // channel (default c1) → cadence
    await next(); // cadence (default daily) → privacy
    await next(); // privacy (default public) → start day
    await next(); // start day (default 1) → confirm

    await fireEvent.click(screen.getByRole('button', { name: 'Start series' }));

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

  it('requires a role before finishing a role-gated series', async () => {
    renderWizard();
    await fireEvent.input(screen.getByPlaceholderText('Daily sketch'), {
      target: { value: 'Gated' },
    });
    const next = () => fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await next(); // → channel
    await next(); // → cadence
    await next(); // → privacy
    await fireEvent.click(screen.getByLabelText('Role-gated'));
    // A role is auto-selected from options, so advancing should succeed here.
    await next(); // → start day
    expect(screen.getByText('First day number')).toBeInTheDocument();
  });
});
