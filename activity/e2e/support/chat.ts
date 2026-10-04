// The chat side of the mock screens: an "Open gallery" button pressed on a
// post while leaf is open. The real server keeps the press as a launch
// intent and hands it to the next request that asks for one, once. The mock
// answers `fetch` inside the page (src/mock/api.ts) and always says nothing
// was pressed, so this stands in front of it for that one route: it counts
// each time the page asks, and answers with a press when a test has made one.

import { expect, type Page } from '@playwright/test';

/** How often a minimised leaf asks for a press (TILE_INTENT_EVERY_MS in Gallery.svelte). */
const ASKS_EVERY_MS = 4_000;

/** What `GET .../launch-intent` answers after a press. */
interface Pressed {
  series_id: number;
  day: number | null;
}

interface ChatState {
  asks: number;
  pressed: Pressed | null;
}

/** Where the page keeps the state, for the functions below that run inside it. */
type ChatWindow = Window & { leafChat: ChatState };

export interface Chat {
  /** How many times the page has asked for a launch intent since it loaded. */
  asks(): Promise<number>;
  /** "Open gallery" pressed on a post: the next ask is answered with it, and only that one. */
  pressOpenGallery(seriesId: number, day?: number): Promise<void>;
  /**
   * Time passes until a minimised leaf has asked once more: one interval, as
   * a rule. The page's clock can be run faster than an answer travels, and
   * leaf does not ask again while one is on its way; then it is the next.
   * A page that does not ask at all fails the test here.
   */
  nextAsk(): Promise<void>;
}

/**
 * Starts counting the page's launch-intent requests. Call it before the
 * screen is opened: the gallery's API client keeps the `fetch` it finds when
 * it is built.
 */
export async function watchChat(page: Page): Promise<Chat> {
  await page.addInitScript(() => {
    const chat: ChatState = { asks: 0, pressed: null };
    (window as unknown as ChatWindow).leafChat = chat;
    // Whatever function the page puts on `window.fetch` (the mock API does),
    // a launch-intent request made through it comes here first.
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
          if (!new URL(href, location.href).pathname.endsWith('/launch-intent')) {
            return answer.call(window, input, init);
          }
          chat.asks += 1;
          const { pressed } = chat;
          chat.pressed = null;
          if (!pressed) return answer.call(window, input, init);
          return Promise.resolve(
            new Response(JSON.stringify(pressed), {
              headers: { 'content-type': 'application/json' },
            }),
          );
        };
      },
    });
  });
  const asks = (): Promise<number> =>
    page.evaluate(() => (window as unknown as ChatWindow).leafChat.asks);
  return {
    asks,
    async nextAsk() {
      const before = await asks();
      await expect
        .poll(async () => {
          await page.clock.runFor(ASKS_EVERY_MS);
          return asks();
        })
        .toBeGreaterThan(before);
    },
    async pressOpenGallery(seriesId, day) {
      await page.evaluate(
        (pressed) => {
          (window as unknown as ChatWindow).leafChat.pressed = pressed;
        },
        { series_id: seriesId, day: day ?? null },
      );
    },
  };
}
