// The axe suite: the main screens' components, rendered with the mock
// screen viewer's fixtures (src/mock), so the content is as varied as the
// real thing: sprouts, a revoked series, dates holding several days, a long
// caption, reminders that are on. Each view also has its own tests, and
// mock/Screen.test.ts runs axe on every mock screen as a whole (the viewer
// over Home while a day loads and when it fails to, the boot screens).

import { render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  createMockAdminApi,
  dayIndex,
  eligibilityBlocked,
  eligibilityOk,
  guildDetail,
  homeSeries,
  options,
  series,
  seriesSettings,
  stats,
  TIMEZONE,
  viewerDay,
} from '../mock/fixtures';
import GuildPanel from '../views/admin/GuildPanel.svelte';
import Calendar from './components/calendar/Calendar.svelte';
import CreateWizard from './components/creator/CreateWizard.svelte';
import SeriesSettingsForm from './components/creator/SeriesSettingsForm.svelte';
import SeriesPicker from './components/picker/SeriesPicker.svelte';
import Stats from './components/stats/Stats.svelte';
import DayViewer from './components/viewer/DayViewer.svelte';
import { resetDraft } from './stores/createDraft.svelte';
import { expectNoA11yViolations } from './test/a11y';
import type * as Timezones from './utils/timezones';

// Pin the device's zone, and cut the zone list to a handful: the full one
// (400+ options) only slows the axe run down.
vi.mock('./utils/timezones', async (original) => ({
  ...(await original<typeof Timezones>()),
  deviceTimezone: () => 'Europe/Berlin',
  hasTimezoneList: () => true,
  timezoneOptions: (current?: string | null) => [
    ...new Set(['America/Chicago', 'Europe/Berlin', 'UTC', ...(current ? [current] : [])]),
  ],
}));

const noop = vi.fn();

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
  // The create form keeps its draft in a module; the admin panel keeps
  // unsaved settings in sessionStorage.
  resetDraft();
  vi.stubGlobal('sessionStorage', memoryStorage());
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe('accessibility (axe)', () => {
  it('series picker has no violations', async () => {
    const { container } = render(SeriesPicker, {
      props: {
        series,
        onSelect: noop,
        eligibility: eligibilityOk,
        eligibilityStatus: 'ready',
        ownsSeries: true,
        // Two series archive from the launch channel, so the list is grouped.
        inChannel: [7, 4],
        onCreate: noop,
        onManage: noop,
      },
    });
    expect(screen.getByRole('heading', { name: 'In this channel' })).toBeInTheDocument();
    expect(screen.getByText('Revoked')).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('series picker has none when the viewer can’t start a series', async () => {
    const { container } = render(SeriesPicker, {
      props: {
        series: series.filter((s) => !s.is_owner),
        onSelect: noop,
        eligibility: eligibilityBlocked,
        eligibilityStatus: 'ready',
      },
    });
    expect(screen.getByText('Want your own series?')).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('create form has no violations', async () => {
    const { container } = render(CreateWizard, {
      props: { options, submitting: false, error: null, onSubmit: noop },
    });
    await expectNoA11yViolations(container);
  });

  it('series settings form has no violations, reminders on', async () => {
    const { container } = render(SeriesSettingsForm, {
      props: {
        settings: seriesSettings,
        options,
        saving: false,
        saved: false,
        error: null,
        onSave: noop,
      },
    });
    expect(screen.getByLabelText('Remind me when I’m behind')).toBeChecked();
    await expectNoA11yViolations(container);
  });

  it('stats has no violations', async () => {
    const { container } = render(Stats, {
      props: { stats, lastPostedAt: homeSeries.last_posted_at },
    });
    await expectNoA11yViolations(container);
  });

  it('calendar has no violations, with two days on one date', async () => {
    const { container } = render(Calendar, {
      props: { index: dayIndex, onOpenDay: noop, timeZone: TIMEZONE },
    });
    expect(container.querySelector('[data-days="120 121"]')).not.toBeNull();
    await expectNoA11yViolations(container);
  });

  it('day viewer has no violations', async () => {
    const { container } = render(DayViewer, {
      props: {
        dayNumber: viewerDay.day,
        day: viewerDay,
        seriesName: homeSeries.name,
        timeZone: TIMEZONE,
        hasPrev: true,
        hasNext: true,
        onPrev: noop,
        onNext: noop,
        onRandom: noop,
        onClose: noop,
      },
    });
    await expectNoA11yViolations(container);
  });

  it('admin panel has no violations, and every setting’s label names its own control', async () => {
    const { container } = render(GuildPanel, {
      props: { api: createMockAdminApi(), guildId: guildDetail.guild_id },
    });
    await screen.findByRole('heading', { level: 2, name: 'Settings' });
    // The pickers' lists arrive separately.
    await waitFor(() => expect(screen.queryByText('Loading…')).toBeNull());

    // A label once named the help button beside it instead of the field
    // (activity-design-system-20): each must reach a form control.
    for (const [label, tag] of [
      ['Timezone', 'SELECT'],
      ['Creator role', 'SELECT'],
      ['Log channel', 'SELECT'],
      ['Series per member', 'INPUT'],
      ['Minimum Discord account age, in days', 'INPUT'],
      ['Minimum time in this server, in days', 'INPUT'],
      ['Sprout stage for new series', 'INPUT'],
      ['Days before a sprout is published', 'INPUT'],
    ] as const) {
      expect(screen.getByLabelText(label).tagName, label).toBe(tag);
    }
    expect(screen.getByLabelText('Timezone')).toHaveValue(guildDetail.settings.timezone);

    await expectNoA11yViolations(container);
  });
});
