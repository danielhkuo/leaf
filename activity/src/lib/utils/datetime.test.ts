import { describe, expect, it } from 'vitest';

import { formatPostedAt } from './datetime';

describe('formatPostedAt', () => {
  it('formats a unix-seconds timestamp into a short date', () => {
    // 2023-11-14T22:13:20Z — a midday-ish time, so the date does not shift
    // across a year boundary in any timezone the test might run in.
    const out = formatPostedAt(1_700_000_000, 'en-US');
    expect(out).toMatch(/Nov 1[45], 2023/);
  });
});
