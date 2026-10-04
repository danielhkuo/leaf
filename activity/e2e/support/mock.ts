// Opening a mock screen (`/mock.html?embed=1&screen=<id>`) and knowing when
// it has finished loading. The ids come from the screen viewer's own list
// (src/mock/screens.ts), so a screen added there is opened by every suite
// that loops over FULL_SCREENS, with nothing to add here. The minimised
// screens (TILE_SCREENS) are what they are in a viewport the size of
// Discord's tile, so they have a suite of their own (tile.spec.ts).
//
// Specs under e2e/mock import `test` from this file: it is the shared one
// (fixtures.ts) plus the `mock` fixture.

import type { Page } from '@playwright/test';

import { FULL_SCREENS, TILE_SCREENS, TILE_SIZES, type ScreenId } from '../../src/mock/screens';
import { test as base, expect } from './fixtures';

export { expect, FULL_SCREENS, TILE_SCREENS, TILE_SIZES, type ScreenId };

export function screenUrl(id: string, longText = false): string {
  return `/mock.html?embed=1&screen=${id}${longText ? '&long=1' : ''}`;
}

/** Playwright's WebKit lacks some of its own video player's artwork, and says so. */
export const WEBKIT_PLAYER_ARTWORK = /^console error: Button failed to load, iconName = /;

/**
 * What a screen logs on purpose: its scenario fails a request, and the view
 * logs the failure. Or, for a screen with a video on it, what WebKit's
 * player logs about itself. Anything else in the console fails the test.
 */
const EXPECTED_OUTPUT: Partial<Record<ScreenId, RegExp[]>> = {
  expired: [/^console error: leaf: loading the gallery failed/],
  'tile-expired': [/^console error: leaf: loading the gallery failed/],
  'load-error': [/^console error: leaf: loading the gallery failed/],
  'viewer-failed': [/^console error: leaf: loading Day 124 failed/],
  'viewer-video': [WEBKIT_PLAYER_ARTWORK],
};

/**
 * Screens that show a loading state for good, which is what they are for.
 * Every other screen is ready only once no placeholder is left.
 */
const HELD_LOADING: ReadonlySet<string> = new Set<ScreenId>(['viewer-loading']);

/**
 * The app's two loading placeholders (Skeleton.svelte, Spinner.svelte), by
 * the class each is styled with. Readiness hangs on these names, so both are
 * held to them: `viewer-loading` is not ready until a spinner is found, and
 * the tests that stop a screen while it loads look for a skeleton.
 */
export const SKELETON = '.skeleton';
const SPINNER = '.spinner';

/** The typefaces app.css declares; a screen is measured in them, not in the fallbacks. */
const FONTS = ['400 16px "DM Sans"', '600 16px "DM Sans"', '600 16px Fraunces'];

/**
 * A phone draws its scrollbar over the page. The desktop engines standing in
 * for one do not always (WebKit on Linux, a Mac set to always show scroll
 * bars), and a scrollbar that takes 15px from the layout would turn a 375px
 * screen into a 360px one.
 */
const PHONE_SCROLLBARS = 'html { scrollbar-width: none } ::-webkit-scrollbar { display: none }';

/**
 * Waits until the screen has what it will show: mounted, no placeholder
 * left (or, for a screen held on its loading state, the spinner up), every
 * picture that is due decoded, a video's size and length known, the web
 * fonts in, and nothing still on its way to its resting look.
 */
async function settled(page: Page, id: string): Promise<void> {
  await page.waitForFunction(
    ({ skeleton, spinner, held }) => {
      const app = document.getElementById('app');
      if (!app || app.childElementCount === 0) return false;
      if (document.querySelector(skeleton)) return false;
      if ((document.querySelector(spinner) !== null) !== held) return false;
      // A video is ready once it knows its size and length (or that it has
      // none to show): its controls are drawn from those.
      const videos = [...document.querySelectorAll('video')];
      if (!videos.every((video) => video.readyState >= 1 || video.error !== null)) return false;
      // Lazy pictures further down the page load when they are scrolled to,
      // and a picture with no box (in a screen put away behind the
      // minimised tile) is not due at all.
      return [...document.images].every(
        (img) =>
          img.complete ||
          img.getClientRects().length === 0 ||
          (img.loading === 'lazy' && img.getBoundingClientRect().top > window.innerHeight),
      );
    },
    { skeleton: SKELETON, spinner: SPINNER, held: HELD_LOADING.has(id) },
  );
  await page.evaluate(async (fonts) => {
    await Promise.all(fonts.map((font) => document.fonts.load(font)));
    await document.fonts.ready;
    // Two frames: the layout in the real fonts, then whatever reacts to it
    // (a caption that now needs its "More" button, a photo sized to its frame).
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, FONTS);
  // A fade or a move that ends: a photo coming in over its preview, a button
  // changing as it is enabled. Measured halfway, a colour is neither the one
  // it was nor the one it will be. What runs without end (a spinner) is left.
  await page.waitForFunction(() =>
    document
      .getAnimations()
      .every(
        (animation) =>
          animation.playState !== 'running' ||
          animation.effect?.getComputedTiming().endTime === Infinity,
      ),
  );
}

/**
 * Keeps the gallery API from ever answering, so a screen stays on the
 * skeletons it shows while it loads. The mock answers `fetch` inside the
 * page (src/mock/api.ts puts its own function on `window.fetch`), so there
 * is no request to hold on the network: whatever function the page puts
 * there, an API call made through it stays pending.
 */
async function holdApi(page: Page): Promise<void> {
  await page.addInitScript(() => {
    let installed = window.fetch;
    Object.defineProperty(window, 'fetch', {
      configurable: true,
      set(next: typeof fetch) {
        installed = next;
      },
      get(): typeof fetch {
        const answer = installed;
        return (input, init) => {
          const href = input instanceof Request ? input.url : String(input);
          return new URL(href, location.href).pathname.startsWith('/api/')
            ? new Promise<Response>(() => undefined)
            : answer.call(window, input, init);
        };
      },
    });
  });
}

export interface MockScreens {
  /**
   * Opens a mock screen and waits for it to settle. `unsettled` returns as
   * soon as the page is there, for a test that holds back something the
   * screen would wait for.
   */
  open(id: ScreenId, options?: { longText?: boolean; unsettled?: boolean }): Promise<void>;
  /**
   * Opens a gallery or creator screen and leaves it loading, on its
   * skeletons: the API never answers. No mock screen shows that state.
   */
  openLoading(id: ScreenId): Promise<void>;
}

export const test = base.extend<{ mock: MockScreens }>({
  mock: async ({ page, guard, isMobile }, use) => {
    // The mock answers the API inside the page: a call that reaches the
    // network is one it has no answer for.
    guard.refuse((url) =>
      url.pathname.startsWith('/api/') ? 'API request reached the network' : null,
    );
    await use({
      async open(id, { longText = false, unsettled = false } = {}) {
        guard.allow(...(EXPECTED_OUTPUT[id] ?? []));
        await page.goto(screenUrl(id, longText));
        if (isMobile) await page.addStyleTag({ content: PHONE_SCROLLBARS });
        if (!unsettled) await settled(page, id);
      },
      async openLoading(id) {
        await holdApi(page);
        await page.goto(screenUrl(id));
        if (isMobile) await page.addStyleTag({ content: PHONE_SCROLLBARS });
        await expect(page.locator(SKELETON).first()).toBeVisible();
      },
    });
  },
});
