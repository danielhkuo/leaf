// What every suite and both configs (Playwright's and the mock server's) have
// to agree on: where the servers listen and what the pages are told about
// time and place. Nothing here reads the network or a secret.

/** Set by CI providers. Turns off server reuse and pixel comparison. */
export const CI = Boolean(process.env.CI);

/**
 * The mock screens' Vite server. Not 5173, which is `npm run dev`: a
 * developer's own preview keeps running while the suite does.
 */
export const MOCK_PORT = Number(process.env.LEAF_E2E_MOCK_PORT ?? 5183);
export const MOCK_ORIGIN = `http://127.0.0.1:${MOCK_PORT}`;

/**
 * The real leaf API with the built Activity (the `e2e_server` example of
 * leaf-server), for the suites under e2e/app. A server holds one seeded
 * world that every client sees, so each engine's project gets a server of
 * its own, on the port after the last: the two projects never share state
 * and can run side by side.
 */
export const APP_PORT = Number(process.env.LEAF_E2E_APP_PORT ?? 3811);
export const APP_ORIGINS = {
  chromium: `http://127.0.0.1:${APP_PORT}`,
  webkit: `http://127.0.0.1:${APP_PORT + 1}`,
} as const;
export type AppEngine = keyof typeof APP_ORIGINS;

/**
 * The Discord application id the e2e server is seeded with (`CLIENT_ID` in
 * its seed.rs). Off `<id>.discordsays.com` the Activity learns its id at
 * build time, so the suite's build is made with this one.
 */
export const APP_CLIENT_ID = '800000000000000001';

/**
 * What the stand-in Discord says the application is called. Not "leaf": the
 * gallery names the app in its archive steps as Discord lists it, and a
 * name of its own shows the steps took it from the handshake.
 */
export const APP_NAME = 'Sketchbook Bot';

/**
 * The instant every page is told it is. The mock fixtures date their days
 * back from "now", so the calendar, "last post" and "today" would otherwise
 * differ from one run to the next. Mid-month and midday in both zones below,
 * so nothing sits on a month, day or clock-change boundary.
 */
export const FIXED_NOW = new Date('2026-05-20T15:00:00Z');

/** The device's locale and zone. The fixtures' server is in America/Chicago. */
export const LOCALE = 'en-US';
export const DEVICE_TIMEZONE = 'Europe/Berlin';
