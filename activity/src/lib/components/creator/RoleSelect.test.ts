import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import type { RoleOption } from '../../types/api';
import RoleSelect from './RoleSelect.svelte';

function roles(count: number): RoleOption[] {
  return Array.from({ length: count }, (_, i) => ({ id: `r${i}`, name: `Role ${i}` }));
}

describe('RoleSelect', () => {
  it('starts on a placeholder that cannot itself be chosen', () => {
    render(RoleSelect, { props: { id: 'role', roles: roles(3), value: '' } });
    const select = screen.getByRole('combobox');
    expect(select).toHaveValue('');
    expect(screen.getByRole('option', { name: 'Choose a role…' })).toBeDisabled();
    expect(screen.queryByLabelText('Filter roles')).not.toBeInTheDocument();
  });

  it('keeps the server’s order and marks the roles the creator has', () => {
    render(RoleSelect, {
      props: {
        id: 'role',
        value: '',
        roles: [
          { id: 'a', name: 'Staff', held: false },
          { id: 'b', name: 'Artists', held: true },
        ],
      },
    });
    const names = screen.getAllByRole('option').map((o) => o.textContent);
    expect(names).toEqual(['Choose a role…', '@Staff', '@Artists (you have it)']);
  });

  it('adds a filter for a long list, which never submits the form', async () => {
    render(RoleSelect, { props: { id: 'role', roles: roles(20), value: 'r3' } });
    const filter = screen.getByLabelText('Filter roles');
    await fireEvent.input(filter, { target: { value: 'role 1' } });

    // Role 1 and Role 10 to 19, plus the one already chosen.
    const names = screen.getAllByRole('option').map((o) => o.textContent);
    expect(names).toHaveLength(1 + 11 + 1);
    expect(names).toContain('@Role 3');
    expect(screen.getByText('11 of 20 roles match. Choose one below.')).toBeInTheDocument();

    const enter = new KeyboardEvent('keydown', { key: 'Enter', cancelable: true, bubbles: true });
    filter.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);

    await fireEvent.input(filter, { target: { value: 'zzz' } });
    expect(screen.getByText('No role matches. Check the spelling.')).toBeInTheDocument();
  });

  it('still shows a saved role that Discord no longer lists', () => {
    render(RoleSelect, { props: { id: 'role', roles: roles(2), value: 'deleted' } });
    expect(screen.getByRole('combobox')).toHaveValue('deleted');
    expect(
      screen.getByRole('option', { name: 'A role that is no longer in this server' }),
    ).toBeInTheDocument();
  });
});
