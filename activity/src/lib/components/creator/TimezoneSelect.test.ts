import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type * as Timezones from '../../utils/timezones';
import TimezoneSelect from './TimezoneSelect.svelte';

const zones = vi.hoisted(() => ({ listed: true }));
vi.mock('../../utils/timezones', async (original) => {
  const real = await original<typeof Timezones>();
  return {
    ...real,
    deviceTimezone: () => 'Europe/Berlin',
    hasTimezoneList: () => zones.listed,
    timezoneOptions: (current?: string | null) =>
      zones.listed
        ? real.timezoneOptions(current)
        : [...new Set(['UTC', 'Europe/Berlin', ...(current ? [current] : [])])],
  };
});

afterEach(() => {
  zones.listed = true;
});

describe('TimezoneSelect', () => {
  it('puts the server’s zone and the device’s first, then every zone once', () => {
    render(TimezoneSelect, {
      props: { id: 'tz', value: 'Asia/Tokyo', serverZone: 'America/Chicago' },
    });
    const select = screen.getByRole('combobox');
    expect(select).toHaveValue('Asia/Tokyo');

    const options = screen.getAllByRole<HTMLOptionElement>('option');
    expect(options[0]).toHaveTextContent('America/Chicago (server default)');
    expect(options[0]).toHaveValue('');
    expect(options[1]).toHaveTextContent('Europe/Berlin (this device)');
    expect(options[1]).toHaveValue('Europe/Berlin');
    const values = options.map((o) => o.value);
    expect(new Set(values).size).toBe(values.length);
    expect(values).toContain('UTC');
    expect(values.length).toBeGreaterThan(100);
  });

  it('offers a text box where the webview cannot list zones', async () => {
    zones.listed = false;
    render(TimezoneSelect, { props: { id: 'tz', value: '', serverZone: 'America/Chicago' } });
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();

    await fireEvent.change(screen.getByRole('combobox'), { target: { value: '__other__' } });
    const typed = screen.getByRole('textbox', { name: /Timezone name/ });
    expect(typed).toHaveAttribute('autocapitalize', 'off');
    expect(typed).toHaveAttribute('spellcheck', 'false');

    await fireEvent.change(screen.getByRole('combobox'), { target: { value: 'UTC' } });
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  });
});
