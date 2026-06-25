import { render } from '@testing-library/svelte';
import { describe, it, vi } from 'vitest';

import CreateWizard from './components/creator/CreateWizard.svelte';
import SeriesSettingsForm from './components/creator/SeriesSettingsForm.svelte';
import SeriesPicker from './components/picker/SeriesPicker.svelte';
import Stats from './components/stats/Stats.svelte';
import DayViewer from './components/viewer/DayViewer.svelte';
import { expectNoA11yViolations } from './test/a11y';
import type { Day, Series, SeriesOptions, SeriesSettings } from './types/api';

const series: Series[] = [
  {
    id: 1,
    name: 'Daily Johan',
    description: 'a daily thing',
    creator_id: 'u',
    cadence: 'daily',
    emoji: '🍃',
    start_day: 1,
    max_day: 5,
  },
];

const day: Day = {
  day: 5,
  caption: 'A nice day',
  posted_at: 1_700_000_000,
  jump_url: 'https://discord.com/channels/g/c/m',
  media: [
    {
      url: '/api/media/a',
      thumb_url: '/api/media/a?thumb',
      content_type: 'image/png',
      missing: false,
    },
  ],
};

const options: SeriesOptions = {
  channels: [{ id: 'c1', name: 'art' }],
  roles: [{ id: 'r1', name: 'Member' }],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: true,
  sprout_threshold: 3,
};

const settings: SeriesSettings = {
  id: 1,
  name: 'Daily Johan',
  description: 'a daily thing',
  emoji: '🍃',
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
};

const noop = vi.fn();

describe('accessibility (axe)', () => {
  it('series picker has no violations', async () => {
    const { container } = render(SeriesPicker, { props: { series, onSelect: noop } });
    await expectNoA11yViolations(container);
  });

  it('create wizard has no violations', async () => {
    const { container } = render(CreateWizard, {
      props: { options, submitting: false, error: null, onSubmit: noop },
    });
    await expectNoA11yViolations(container);
  });

  it('series settings form has no violations', async () => {
    const { container } = render(SeriesSettingsForm, {
      props: { settings, options, saving: false, saved: false, error: null, onSave: noop },
    });
    await expectNoA11yViolations(container);
  });

  it('stats has no violations', async () => {
    const { container } = render(Stats, {
      props: { stats: { total: 10, current_streak: 3, longest_streak: 7, missed: 2, max_day: 10 } },
    });
    await expectNoA11yViolations(container);
  });

  it('day viewer has no violations', async () => {
    const { container } = render(DayViewer, {
      props: {
        day,
        seriesName: 'Daily Johan',
        hasPrev: true,
        hasNext: true,
        onPrev: noop,
        onNext: noop,
        onRandom: noop,
        onClose: noop,
        onJump: noop,
      },
    });
    await expectNoA11yViolations(container);
  });
});
