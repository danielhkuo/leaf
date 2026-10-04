// The `test` every suite builds on. On top of Playwright's own it gives each
// test a fixed clock and a guard on the page's console and requests, both
// without the spec asking: a spec that forgets them still gets them.
//
// A suite adds what is its own by extending this one (support/mock.ts adds
// the mock screens), and its specs import `test` from there.

import { test as base, expect } from '@playwright/test';

import { FIXED_NOW } from './env';
import { PageGuard } from './guard';

interface Options {
  /**
   * The instant the page is told it is. `Date` answers this for the whole
   * test while timers run as usual. `null` leaves the real clock, for a
   * project whose server keeps its own time: `use: { now: null }`.
   */
  now: Date | null;
  /**
   * Origins besides `baseURL`'s that a page may reach: a second server the
   * project itself starts on this machine (a stand-in for Discord's side,
   * say). Anything else is cut off before it leaves the browser.
   */
  alsoReachable: string[];
}

interface Fixtures {
  /**
   * Fails the test for console errors, warnings, failed requests and broken
   * pictures it did not allow, and keeps the page from reaching any server
   * but the one under test.
   */
  guard: PageGuard;
  fixedClock: undefined;
}

export const test = base.extend<Options & Fixtures>({
  now: [FIXED_NOW, { option: true }],
  alsoReachable: [[], { option: true }],

  fixedClock: [
    async ({ page, now }, use) => {
      if (now) await page.clock.setFixedTime(now);
      await use(undefined);
    },
    { auto: true },
  ],

  guard: [
    async ({ page, baseURL, alsoReachable }, use) => {
      if (!baseURL) throw new Error('the project sets no baseURL');
      const origins = alsoReachable.map((url) => new URL(url).origin);
      const guard = new PageGuard(page, new URL(baseURL).origin, origins);
      await guard.isolate();
      await use(guard);
      // A trip to the page and back, so what it logged last has arrived too.
      await guard.inspect();
      expect(guard.unexpected(), 'console errors, warnings and failed requests').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
