// Fingers on the screen. Playwright's own touchscreen can only tap, and the
// viewer's gestures (swipe to turn the page, pinch to zoom) are sequences of
// pointer events from one or two fingers.
//
// - Chromium takes real touch input through the DevTools protocol: the
//   browser itself produces the pointer events (trusted, with its own pointer
//   ids), and applies `touch-action` as it would to a finger.
// - WebKit has no such input, so there the same sequence is dispatched as
//   PointerEvents from inside the page. Those are not trusted input: they
//   exercise the app's handlers in WebKit's engine, not WebKit's own touch
//   handling. `setPointerCapture` refuses a pointer the browser did not
//   create, so for these ids alone that refusal is swallowed.

import type { CDPSession, Page } from '@playwright/test';

export interface Point {
  x: number;
  y: number;
}

/** A finger, by number, at a place. */
interface Contact extends Point {
  id: number;
}

interface Driver {
  down(contact: Contact): Promise<void>;
  move(contacts: Contact[]): Promise<void>;
  up(id: number): Promise<void>;
}

/** Steps a moving finger takes between two points: enough to read as a drag. */
const STEPS = 6;

class ChromiumTouch implements Driver {
  readonly #cdp: CDPSession;
  readonly #down = new Map<number, Contact>();

  constructor(cdp: CDPSession) {
    this.#cdp = cdp;
  }

  async #send(
    type: 'touchStart' | 'touchMove' | 'touchEnd',
    touchPoints: Contact[],
  ): Promise<void> {
    await this.#cdp.send('Input.dispatchTouchEvent', { type, touchPoints });
  }

  async down(contact: Contact): Promise<void> {
    this.#down.set(contact.id, contact);
    // Every finger that is down is listed; the new one is the one pressed.
    await this.#send('touchStart', [...this.#down.values()]);
  }

  async move(contacts: Contact[]): Promise<void> {
    for (const contact of contacts) this.#down.set(contact.id, contact);
    await this.#send('touchMove', [...this.#down.values()]);
  }

  async up(id: number): Promise<void> {
    const contact = this.#down.get(id);
    if (!contact) throw new Error(`finger ${id} is not down`);
    this.#down.delete(id);
    // An end lists the fingers that lift; the others stay down.
    await this.#send('touchEnd', [contact]);
  }
}

/** Pointer ids well away from the ones a browser gives its own pointers. */
const SYNTHETIC_ID = 9_000;

class SyntheticTouch implements Driver {
  readonly #page: Page;
  readonly #down = new Map<number, Contact>();

  constructor(page: Page) {
    this.#page = page;
  }

  /** Lets the page capture the pointers this driver makes up. Call before navigating. */
  static async prepare(page: Page): Promise<void> {
    await page.addInitScript((firstId) => {
      const capture = Element.prototype.setPointerCapture;
      Element.prototype.setPointerCapture = function (pointerId: number): void {
        if (pointerId < firstId) capture.call(this, pointerId);
      };
    }, SYNTHETIC_ID);
  }

  async #dispatch(type: string, contact: Contact): Promise<void> {
    await this.#page.evaluate(
      ({ type, contact, firstId, primary }) => {
        const target = document.elementFromPoint(contact.x, contact.y);
        if (!target) throw new Error(`nothing is at ${contact.x}, ${contact.y}`);
        target.dispatchEvent(
          new PointerEvent(type, {
            pointerId: firstId + contact.id,
            pointerType: 'touch',
            isPrimary: primary,
            clientX: contact.x,
            clientY: contact.y,
            button: type === 'pointermove' ? -1 : 0,
            buttons: type === 'pointerup' ? 0 : 1,
            bubbles: true,
            cancelable: true,
            composed: true,
          }),
        );
      },
      { type, contact, firstId: SYNTHETIC_ID, primary: this.#primary(contact.id) },
    );
  }

  /** The first finger down is the primary pointer for as long as it stays. */
  #primary(id: number): boolean {
    return [...this.#down.keys()][0] === id;
  }

  async down(contact: Contact): Promise<void> {
    this.#down.set(contact.id, contact);
    await this.#dispatch('pointerdown', contact);
  }

  async move(contacts: Contact[]): Promise<void> {
    for (const contact of contacts) {
      this.#down.set(contact.id, contact);
      await this.#dispatch('pointermove', contact);
    }
  }

  async up(id: number): Promise<void> {
    const contact = this.#down.get(id);
    if (!contact) throw new Error(`finger ${id} is not down`);
    await this.#dispatch('pointerup', contact);
    this.#down.delete(id);
  }
}

function between(from: Point, to: Point, step: number): Point {
  const t = step / STEPS;
  return { x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t };
}

/** Where a finger that is down goes next. */
export interface Glide {
  finger: number;
  from: Point;
  to: Point;
}

export class Touch {
  readonly #driver: Driver;

  private constructor(driver: Driver) {
    this.#driver = driver;
  }

  /**
   * The touch input for this page's engine. In WebKit, call it before the
   * page is opened: it installs a script that runs as the page loads.
   */
  static async on(page: Page, browserName: string): Promise<Touch> {
    if (browserName === 'chromium') {
      return new Touch(new ChromiumTouch(await page.context().newCDPSession(page)));
    }
    await SyntheticTouch.prepare(page);
    return new Touch(new SyntheticTouch(page));
  }

  /** Puts a finger down. */
  down(finger: number, at: Point): Promise<void> {
    return this.#driver.down({ id: finger, ...at });
  }

  /** Moves the fingers that are down, together, in small steps. */
  async glide(...glides: Glide[]): Promise<void> {
    for (let step = 1; step <= STEPS; step += 1) {
      await this.#driver.move(
        glides.map((g) => ({ id: g.finger, ...between(g.from, g.to, step) })),
      );
    }
  }

  /** Lifts a finger. */
  up(finger: number): Promise<void> {
    return this.#driver.up(finger);
  }

  /** One finger dragged from one point to another, then lifted. */
  async swipe(from: Point, to: Point): Promise<void> {
    await this.down(0, from);
    await this.glide({ finger: 0, from, to });
    await this.up(0);
  }
}
