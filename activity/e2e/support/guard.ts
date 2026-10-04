// Watches a page for what a healthy screen never produces: console errors
// and warnings, uncaught exceptions, requests that fail or are refused,
// pictures that are not pictures, and requests that leave the server under
// test. Those last are also cut off before they leave the browser: a test
// run reaches no other machine, whatever a screen asks for. A test names the
// output its screen produces on purpose with `allow`; whatever is left when
// the test ends fails it (see fixtures.ts).

import type { Page } from '@playwright/test';

/** Why a request must not be made, or `null` when it may. */
type Refusal = (url: URL) => string | null;

/** What a page loads besides documents and data. A server never answers one with a page. */
const ASSETS: ReadonlySet<string> = new Set(['image', 'font', 'stylesheet', 'script', 'media']);

/** `text` as a regex that matches exactly it. */
const literal = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Matches a URL of `schemes` (a regex alternation) that starts with none of `prefixes`. */
function outside(prefixes: string[], schemes: string): RegExp {
  const known = prefixes.map(literal).join('|');
  return new RegExp(`^(?!(?:${known})(?:[/?#]|$))(?:${schemes})://`, 'i');
}

export class PageGuard {
  readonly #page: Page;
  readonly #origins: readonly string[];
  readonly #seen: string[] = [];
  readonly #allowed: RegExp[] = [];
  readonly #refusals: Refusal[];

  /**
   * `origin` is the server under test, and `also` any others the project
   * itself runs on this machine: a request to any other origin is reported.
   */
  constructor(page: Page, origin: string, also: readonly string[] = []) {
    this.#page = page;
    this.#origins = [origin, ...also];
    this.#refusals = [
      (url) => (this.#origins.includes(url.origin) ? null : 'request left the server under test'),
    ];
    page.on('console', (message) => {
      const type = message.type();
      if (type === 'error' || type === 'warning') {
        this.#seen.push(`console ${type}: ${message.text()}`);
      }
    });
    page.on('pageerror', (error) => this.#seen.push(`uncaught: ${error.message}`));
    page.on('request', (request) => {
      const url = new URL(request.url());
      // Not requests at all: nothing leaves the page for these.
      if (['data:', 'blob:', 'about:'].includes(url.protocol)) return;
      for (const refuse of this.#refusals) {
        const why = refuse(url);
        if (why !== null) this.#seen.push(`${why}: ${request.method()} ${request.url()}`);
      }
    });
    page.on('requestfailed', (request) => {
      const why = request.failure()?.errorText ?? 'no reason given';
      this.#seen.push(`request failed: ${request.method()} ${request.url()} (${why})`);
    });
    page.on('response', (response) => {
      const request = response.request();
      const what = `${request.method()} ${response.url()}`;
      if (response.status() >= 400) {
        this.#seen.push(`HTTP ${response.status()}: ${what}`);
        return;
      }
      // A dev server answers a path it does not have with the app's page and
      // a 200 (Vite's fallback for client-side routes), so a missing picture
      // or font looks like a success to everything but its content type.
      const type = response.headers()['content-type'] ?? '';
      if (ASSETS.has(request.resourceType()) && type.startsWith('text/html')) {
        this.#seen.push(`a page answered for a missing ${request.resourceType()}: ${what}`);
      }
    });
  }

  /**
   * Cuts off every request and every WebSocket to an origin other than the
   * servers under test, for all pages of this page's context. The request is
   * still reported (as leaving, and then as failed); it just never arrives.
   */
  async isolate(): Promise<void> {
    const context = this.#page.context();
    // Patterns, not predicates: Playwright matches a pattern without a trip
    // to this process, so the requests that stay are not held up on the way.
    await context.route(outside([...this.#origins], 'https?'), (route) =>
      route.abort('blockedbyclient'),
    );
    const sockets = this.#origins.map((origin) => origin.replace(/^http/i, 'ws'));
    await context.routeWebSocket(outside(sockets, 'wss?'), (socket) => {
      this.#seen.push(`socket left the server under test: ${socket.url()}`);
      void socket.close({ code: 1008, reason: 'cut off by the test guard' });
    });
  }

  /** Output this test's screen produces on purpose. */
  allow(...patterns: RegExp[]): void {
    this.#allowed.push(...patterns);
  }

  /** Requests this kind of test must never make, on top of leaving the server. */
  refuse(refusal: Refusal): void {
    this.#refusals.push(refusal);
  }

  /**
   * Looks over the page as it stands for what no event reports: a picture
   * that finished loading and is not one (an empty or unreadable file, or a
   * page sent in its place). Frames are looked over too: a suite may show
   * the app inside one. Run for every test when it ends.
   */
  async inspect(): Promise<void> {
    if (this.#page.isClosed()) return;
    const main = this.#page.mainFrame();
    for (const frame of this.#page.frames()) {
      let broken: string[];
      try {
        broken = await frame.evaluate(() =>
          [...document.images]
            .filter((img) => img.complete && img.currentSrc !== '' && img.naturalWidth === 0)
            .map((img) => {
              const classes = [...img.classList].filter((c) => !c.startsWith('svelte-'));
              return `img${classes.map((c) => `.${c}`).join('')} ${img.currentSrc.slice(0, 120)}`;
            }),
        );
      } catch (error) {
        // A framed document can go away while it is being looked at. The
        // page itself cannot: that failure is the test's to hear about.
        if (frame === main) throw error;
        continue;
      }
      for (const image of broken) this.#seen.push(`broken image: ${image}`);
    }
  }

  /** Everything seen so far that no pattern allows. */
  unexpected(): string[] {
    return this.#seen.filter((line) => !this.#allowed.some((pattern) => pattern.test(line)));
  }
}
