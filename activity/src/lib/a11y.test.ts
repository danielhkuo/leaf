import { render } from '@testing-library/svelte';
import { describe, it, vi } from 'vitest';

import SeriesPicker from './components/picker/SeriesPicker.svelte';
import Stats from './components/stats/Stats.svelte';
import DayViewer from './components/viewer/DayViewer.svelte';
import { expectNoA11yViolations } from './test/a11y';
import type { Day, Series } from './types/api';

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

const noop = vi.fn();

describe('accessibility (axe)', () => {
  it('series picker has no violations', async () => {
    const { container } = render(SeriesPicker, { props: { series, onSelect: noop } });
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
