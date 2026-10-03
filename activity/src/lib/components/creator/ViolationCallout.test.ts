import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import ViolationCallout from './ViolationCallout.svelte';

describe('ViolationCallout', () => {
  it('writes the reason from the code and its details, not the server’s fragment', () => {
    render(ViolationCallout, {
      props: {
        violations: [
          {
            code: 'missing_creator_role',
            message: 'starting a series here requires the creator role',
            params: { role_name: 'Artists' },
          },
        ],
      },
    });
    expect(screen.getByText('You can’t start a series here yet')).toBeInTheDocument();
    expect(
      screen.getByText(
        'Starting a series here needs the @Artists role. Ask a server admin for it.',
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/requires the creator role/)).not.toBeInTheDocument();
  });

  it('lists every rule in the way', () => {
    render(ViolationCallout, {
      props: {
        violations: [
          { code: 'max_series', message: '', params: { limit: 3, current: 3 } },
          { code: 'membership_too_new', message: '', params: { days: 7 } },
        ],
      },
    });
    const items = screen.getAllByRole('listitem');
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent('You have 3 of 3 series here');
    expect(items[1]).toHaveTextContent('at least 7 days');
  });

  it('names /setup and who runs it when the server is not set up', () => {
    render(ViolationCallout, {
      props: {
        violations: [
          { code: 'guild_not_setup', message: 'leaf isn’t set up in this server yet' },
          { code: 'missing_creator_role', message: '' },
        ],
      },
    });
    expect(screen.getByText('leaf isn’t set up in this server yet')).toBeInTheDocument();
    expect(
      screen.getByText(/A server admin needs to run \/setup in chat first/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/needs a role/)).not.toBeInTheDocument();
  });

  it('falls back to the server’s sentence for a rule this build does not know', () => {
    render(ViolationCallout, {
      props: { violations: [{ code: 'new_rule', message: 'A new rule applies.' }] },
    });
    expect(screen.getByText('A new rule applies.')).toBeInTheDocument();
  });
});
