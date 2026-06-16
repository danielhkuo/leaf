import { render, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { installMockIO, MockIntersectionObserver } from '../../test/io';
import type { Block } from '../../utils/heatmap';
import MonthBlock from './MonthBlock.svelte';

const BLOCK: Block = { index: 0, fromDay: 1, toDay: 28, year: 1 };

describe('MonthBlock windowing', () => {
  let restore: () => void;
  afterEach(() => restore());

  it('mounts no cells and does not fetch until scrolled into view', async () => {
    restore = installMockIO();
    const load = vi.fn(() =>
      Promise.resolve([
        { day: 1, thumb_url: null },
        { day: 2, thumb_url: null },
      ]),
    );

    const { container } = render(MonthBlock, {
      props: { block: BLOCK, maxDay: 28, startDay: 1, load, onOpenDay: vi.fn() },
    });

    // Windowed out: the block holds its height but renders zero day cells and
    // never fetches — this is what keeps the DOM bounded over a long archive.
    expect(container.querySelectorAll('.cell')).toHaveLength(0);
    expect(load).not.toHaveBeenCalled();

    // Scroll it into view → cells mount and the block fetches exactly once.
    MockIntersectionObserver.instances[0]!.enter();
    await waitFor(() => {
      expect(load).toHaveBeenCalledOnce();
    });
    await waitFor(() => {
      expect(container.querySelectorAll('.cell')).toHaveLength(28);
    });
  });
});
