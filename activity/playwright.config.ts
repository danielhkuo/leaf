// Browser suites (`npm run e2e`). Everything under e2e/mock runs against the
// dev-only mock screens (`/mock.html`): fixture data, no Discord, no
// leaf-server, no network. Everything under e2e/app runs the built Activity
// against the real leaf API (leaf-server's `e2e_server` example) inside a
// stand-in for the Discord client. See the Testing section of README.md.
import { defineConfig } from '@playwright/test';

import { APP_ORIGINS, CI, DEVICE_TIMEZONE, LOCALE, MOCK_ORIGIN } from './e2e/support/env';
import { serversFor } from './e2e/support/servers';
import { viewport, WIDE_TAG } from './e2e/support/viewports';

/** The pixel comparison, kept apart from the assertions: its baselines are per OS. */
const VISUAL = /visual\.spec\.ts$/;

/**
 * What an app project needs besides its engine and origin. A leaf server
 * holds one seeded world, reset before each test, so the tests of a project
 * take turns at it.
 */
const APP = {
  testDir: './e2e/app',
  fullyParallel: false,
  workers: 1,
} as const;
/** The common iPhone width, with a phone's touch and user agent. */
const PHONE = viewport(375).use;

/**
 * Nothing but this machine is reachable from an app project's browser. The
 * guard's routes cannot promise that alone: a redirect is followed inside
 * the browser, where no route sees it, and leaf's own `/admin/login`
 * redirects to discord.com. So every address but loopback goes to a proxy
 * that is not there (port 9, discard) and fails at once.
 */
const LOOPBACK_ONLY = { proxy: { server: 'http://127.0.0.1:9', bypass: '127.0.0.1' } } as const;

/**
 * The specs WebKit, the nearest stand-in for the iOS webview, runs too: what
 * a phone shows, and the check that `LOOPBACK_ONLY` holds, which each engine
 * has to pass for itself.
 */
const APP_ON_WEBKIT = /(boot|calendar|viewer|isolation)\.spec\.ts$/;

/** The server each project's pages are served by (see `serversFor`). */
const SERVERS = {
  'mock-warmup': 'mock',
  'mock-chromium': 'mock',
  'mock-webkit': 'mock',
  visual: 'mock',
  'app-chromium': 'chromium',
  'app-webkit': 'webkit',
} as const;

export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  // One place for every baseline, with the OS in the path: only the macOS
  // ones are committed (see .gitignore).
  snapshotPathTemplate: '{testDir}/__screenshots__/{platform}/{arg}{ext}',
  fullyParallel: true,
  forbidOnly: CI,
  // A test that passes on the second go is still a broken test.
  retries: 0,
  // A CI runner has nothing else to do; a laptop keeps half its cores.
  workers: CI ? '100%' : '50%',
  timeout: 30_000,
  // Every wait is on a condition; a shared CI runner just gets longer to meet it.
  expect: { timeout: CI ? 10_000 : 5_000 },
  reporter: [[CI ? 'github' : 'list'], ['html', { open: 'never' }]],
  use: {
    locale: LOCALE,
    timezoneId: DEVICE_TIMEZONE,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    // Loads the mock once before the parallel projects start, so Vite has
    // compiled every module (and settled its dependency cache) by then.
    { name: 'mock-warmup', testMatch: /mock\/warmup\.setup\.ts$/, use: { baseURL: MOCK_ORIGIN } },
    {
      name: 'mock-chromium',
      testDir: './e2e/mock',
      testIgnore: VISUAL,
      dependencies: ['mock-warmup'],
      use: { browserName: 'chromium', baseURL: MOCK_ORIGIN },
    },
    {
      // The nearest CI stand-in for iOS: the phone widths only.
      name: 'mock-webkit',
      testDir: './e2e/mock',
      testIgnore: VISUAL,
      grepInvert: new RegExp(WIDE_TAG),
      dependencies: ['mock-warmup'],
      use: { browserName: 'webkit', baseURL: MOCK_ORIGIN },
    },
    {
      name: 'visual',
      testDir: './e2e/mock',
      testMatch: VISUAL,
      dependencies: ['mock-warmup'],
      use: { browserName: 'chromium', baseURL: MOCK_ORIGIN },
    },
    {
      name: 'app-chromium',
      ...APP,
      use: { browserName: 'chromium', baseURL: APP_ORIGINS.chromium, ...PHONE, ...LOOPBACK_ONLY },
    },
    {
      name: 'app-webkit',
      ...APP,
      testMatch: APP_ON_WEBKIT,
      use: { browserName: 'webkit', baseURL: APP_ORIGINS.webkit, ...PHONE, ...LOOPBACK_ONLY },
    },
  ],
  webServer: serversFor(process.argv, SERVERS),
});
