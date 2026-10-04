// Mounts every mock screen, waits for something only that screen shows, and
// runs axe on it. Nothing else runs them, and they lean on the real views'
// internals: MockGallery on how Gallery opens its first view and on the API
// client keeping the `fetch` it finds; api.ts on answering every request a
// view makes. A change elsewhere could leave a screen on a skeleton, on the
// wrong view or on an error state with the rest of the suite green.
//
// MockBoot and the admin chrome in Screen.svelte are copies of App.svelte's
// and Admin.svelte's markup; the last tests here hold them to the originals.

import type * as TestingLibrary from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as AdminClient from '../lib/admin/client';
import type * as SessionStore from '../lib/stores/session.svelte';
import { expectNoA11yViolations } from '../lib/test/a11y';
import { FULL_SIZE, resizeTo, TILE_SIZE } from '../lib/test/viewport';
import { bootScreens, GUILD_ID } from './fixtures';
import { SCREENS, TILE_SCREENS, type ScreenId } from './screens';

type Queries = (typeof TestingLibrary)['screen'];

/** Longer than the client's pause before it retries a failed read. */
const WAIT = { timeout: 3_000 };

/** A working Storage. Newer Node defines its own unusable `localStorage` global. */
function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => void items.delete(key),
    setItem: (key, value) => void items.set(key, String(value)),
  };
}

// Each screen is its own page in the screen viewer (an iframe), with module
// state of its own: the gallery store, its API client, the nav stack. So each
// test loads a fresh Screen. A component can only be mounted by the Svelte
// runtime it was compiled against, so the testing library is loaded again too.
let testing: typeof TestingLibrary;
let errors: string[];

/** Whether a screen is one of the minimised ones, which their viewport's size makes. */
const isTile = (id: string): boolean => TILE_SCREENS.some((screen) => screen.id === id);

async function show(id: string, longText = false): Promise<HTMLElement> {
  // As the screen viewer and the browser suites open them: in a viewport the
  // size of the tile Discord shrinks leaf to.
  resizeTo(isTile(id) ? TILE_SIZE : FULL_SIZE);
  const { default: Screen } = await import('./Screen.svelte');
  return testing.render(Screen, { props: { id, longText } }).container;
}

/**
 * The card a minimised screen shows: its name, the line under it (`null`
 * for none) and whether a picture is behind them. It waits for all three,
 * since the picture comes with the series' days.
 */
const tile =
  (name: string, detail: string | null, pictured = false) =>
  (s: Queries): Promise<HTMLElement> =>
    testing.waitFor(() => {
      // The only heading there is: the screen behind the card is put away.
      const card = s.getByRole('heading', { level: 1, name }).closest('main');
      if (!card) throw new Error('the card is not the page’s main landmark');
      expect(card).toHaveClass('tile');
      expect(card.querySelector('p')?.textContent ?? null).toBe(detail);
      expect(card.querySelector('img') !== null).toBe(pictured);
      return card;
    }, WAIT);

beforeEach(async () => {
  vi.resetModules();
  vi.stubGlobal('localStorage', memoryStorage());
  vi.stubGlobal('sessionStorage', memoryStorage());
  vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined);
  // The full zone list (400+ options in three screens) only slows axe down.
  vi.spyOn(Intl, 'supportedValuesOf').mockReturnValue(['America/Chicago', 'Europe/Berlin']);
  errors = [];
  vi.spyOn(console, 'error').mockImplementation(
    (first: unknown) => void errors.push(String(first)),
  );
  testing = await import('@testing-library/svelte');
});
afterEach(async () => {
  // Gallery fetches its lazy chunks as it mounts. One still loading when the
  // next test resets the modules would be evaluated into the new module graph.
  await vi.dynamicImportSettled();
  testing.cleanup();
  vi.doUnmock('../lib/stores/session.svelte');
  vi.doUnmock('../lib/admin/client');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  history.replaceState(null, '', '/');
  resizeTo(FULL_SIZE);
});

const heading =
  (name: string, level = 1) =>
  (s: Queries): Promise<HTMLElement> =>
    s.findByRole('heading', { level, name }, WAIT);
const button =
  (name: string | RegExp) =>
  (s: Queries): Promise<HTMLElement> =>
    s.findByRole('button', { name }, WAIT);
const text =
  (shown: string | RegExp) =>
  (s: Queries): Promise<HTMLElement> =>
    s.findByText(shown, undefined, WAIT);
const field =
  (label: string) =>
  (s: Queries): Promise<HTMLElement> =>
    s.findByLabelText(label, undefined, WAIT);

/**
 * What tells each screen apart once it has loaded: on the view the screen is
 * for, and where it can be, something that needs the mock API's answer.
 */
const LANDMARKS: Record<ScreenId, (s: Queries) => Promise<HTMLElement>> = {
  picker: button('Manage my series'),
  'picker-empty': text('No series here yet'),
  'picker-blocked': text('Want your own series?'),
  home: button('Latest: Day 128'),
  'home-empty': heading('Archive your first post', 2),
  'home-sprout': text(/Only you can see this series for now/),
  'home-channel-gone': button('Choose another channel'),
  viewer: (s) => s.findByRole('dialog', { name: 'Day 125, Daily Sketch' }, WAIT),
  // The day's one file is a video, under its poster until it plays. (The
  // dialog is asked for each time: Gallery's stand-in has the same name.)
  'viewer-video': (s) =>
    testing.waitFor(() => {
      const dialog = s.getByRole('dialog', { name: 'Day 118, Daily Sketch' });
      expect(dialog.querySelector('video[poster]')).not.toBeNull();
      return dialog;
    }, WAIT),
  // The viewer's own loading state: its shell is up, the day is not.
  'viewer-loading': (s) => s.findByRole('status', { name: 'Loading Day 125' }, WAIT),
  'viewer-failed': async (s) => {
    await s.findByText('Couldn’t load this day', undefined, WAIT);
    return s.getByRole('button', { name: 'Close' });
  },
  create: button('Start a series'),
  'create-blocked': text('You can’t start a series here yet'),
  myseries: text(/124 days archived/),
  settings: field('Remind me when I’m behind'),
  'settings-channel-unseen': text(/this server has no other series channel it can see/),
  'admin-login': (s) => s.findByRole('link', { name: 'Sign in with Discord' }, WAIT),
  'admin-panel': heading('Settings', 2),
  loading: text('Opening leaf…'),
  'loading-consent': text('Waiting for your OK in Discord…'),
  error: heading('Discord didn’t respond'),
  'error-retry': button('Try again'),
  unavailable: text('This series isn’t available'),
  expired: text('Your session has ended'),
  'load-error': text('Couldn’t load the gallery'),
  landing: heading('leaf is running'),
  'tile-day': tile('Daily Sketch', 'Day 125', true),
  'tile-series': tile('Daily Sketch', 'Day 128', true),
  'tile-empty': tile('Pressed Flowers', 'No days yet'),
  'tile-list': tile('leaf', null),
  'tile-boot': tile('leaf', 'Opening…'),
  'tile-error': tile('leaf', 'Didn’t open'),
  'tile-expired': tile('leaf', 'Session ended'),
};

/** The screens whose scenario fails a request on purpose, and what that logs. */
const LOGGED: Partial<Record<ScreenId, string[]>> = {
  expired: ['leaf: loading the gallery failed'],
  'load-error': ['leaf: loading the gallery failed'],
  'viewer-failed': ['leaf: loading Day 124 failed'],
  'tile-expired': ['leaf: loading the gallery failed'],
};

describe('mock screens', () => {
  it.each(SCREENS.map((s) => s.id))('%s mounts, shows its view and passes axe', async (id) => {
    const container = await show(id);

    expect(await LANDMARKS[id](testing.screen)).toBeInTheDocument();
    expect(testing.screen.queryByText(/There is no mock screen called/)).toBeNull();
    // The mock API logs a request it has no answer for, and most views log a
    // load that failed: either shows here even when the landmark still renders.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(errors).toEqual(LOGGED[id] ?? []);
    await expectNoA11yViolations(container);
    // axe only checks these on a whole page (the Playwright suite in
    // e2e/mock does): every screen is one main landmark with a heading. A
    // minimised screen's is the card; the screen put away behind it is inert.
    const mains = [...container.querySelectorAll('main')].filter(
      (main) => !main.closest('[hidden]'),
    );
    expect(mains).toHaveLength(1);
    expect(mains[0]?.querySelector('h1')).not.toBeNull();
    expect(container.querySelectorAll('.screens[hidden]')).toHaveLength(isTile(id) ? 1 : 0);
  });

  it('viewer-loading can be left: a dialog with a Close button while the day loads', async () => {
    // jsdom has no scrolling; Home brings the day's cell into view on close.
    const scrolled = vi.fn();
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      value: scrolled,
      configurable: true,
    });
    try {
      await leaveLoadingViewer(scrolled);
    } finally {
      Reflect.deleteProperty(Element.prototype, 'scrollIntoView');
    }
  });

  async function leaveLoadingViewer(scrolled: ReturnType<typeof vi.fn>): Promise<void> {
    const container = await show('viewer-loading');
    // Gallery's stand-in for the viewer's chunk is a dialog of the same name:
    // wait for the viewer's own loading state before taking hold of it.
    await testing.screen.findByRole('status', { name: 'Loading Day 125' }, WAIT);
    const dialog = testing.screen.getByRole('dialog', { name: 'Day 125, Daily Sketch' });
    // The date and the way to the neighbouring days come from the calendar's
    // index, which has loaded.
    await testing.waitFor(() => expect(dialog.querySelector('time')).not.toBeNull());
    expect(testing.screen.getByRole('button', { name: 'Previous day' })).toBeInTheDocument();

    await testing.fireEvent.click(testing.screen.getByRole('button', { name: 'Close' }));
    await testing.waitFor(() => expect(testing.screen.queryByRole('dialog')).toBeNull());
    expect(testing.screen.getByRole('button', { name: 'Latest: Day 128' })).toBeInTheDocument();
    // Home is told which day was open: its cell is scrolled to and focused.
    const cell = container.querySelector('[data-days~="125"]');
    await testing.waitFor(() => expect(cell).toHaveFocus());
    expect(scrolled).toHaveBeenCalled();
  }

  it('says so for an id it does not know', async () => {
    await show('nope');
    expect(testing.screen.getByText('There is no mock screen called “nope”.')).toBeInTheDocument();
  });

  it.each(['home', 'admin-panel'])('%s takes worst-case text when asked', async (id) => {
    await show(id, true);
    // A series name and a server name, at 40 characters with no spaces.
    expect(
      await testing.screen.findByRole('heading', { level: 1, name: 'W'.repeat(40) }, WAIT),
    ).toBeInTheDocument();
  });
});

/**
 * An element's tags, attributes and text, without what differs between two
 * components that render the same markup: Svelte's scoped class names and
 * comment anchors, and how the source happens to be wrapped.
 */
function outline(el: Element): unknown[] {
  const attributes = [...el.attributes]
    .map(({ name, value }) =>
      name === 'class' ? `class=${value.replace(/\bsvelte-\w+/g, '').trim()}` : `${name}=${value}`,
    )
    .sort();
  const children: unknown[] = [];
  let run = '';
  const endRun = (): void => {
    const words = run.replace(/\s+/g, ' ').trim();
    if (words) children.push(words);
    run = '';
  };
  for (const child of el.childNodes) {
    if (child instanceof Element) {
      endRun();
      children.push(outline(child));
    } else if (child.nodeType === Node.TEXT_NODE) {
      run += child.textContent ?? '';
    }
  }
  endRun();
  return [el.tagName.toLowerCase(), attributes, children];
}

function outlineOf(container: HTMLElement, selector: string): unknown[] {
  const el = container.querySelector(selector);
  if (!el) throw new Error(`nothing matches ${selector}`);
  return outline(el);
}

/** Every `<main>` in the container, outlined: the screen, and the card over it when minimised. */
function mainsOf(container: HTMLElement): unknown[] {
  return [...container.querySelectorAll('main')].map(outline);
}

describe('mock screens copied from a real one', () => {
  it.each([...bootScreens])('%s is what App renders in that boot state', async (id, state) => {
    vi.doMock('../lib/stores/session.svelte', async (original) => ({
      ...(await original<typeof SessionStore>()),
      // The real one starts the Discord handshake, which nothing here answers.
      bootSession: () => Promise.resolve(),
    }));
    const { session } = await import('../lib/stores/session.svelte');
    session.value = state;
    // In the viewport the mock screen is for: a minimised one is the card
    // over the boot screen, and both are held to App's.
    resizeTo(isTile(id) ? TILE_SIZE : FULL_SIZE);
    const { default: App } = await import('../App.svelte');
    const real = mainsOf(testing.render(App).container);
    testing.cleanup();

    expect(real).toHaveLength(isTile(id) ? 2 : 1);
    expect(mainsOf(await show(id))).toEqual(real);
  });

  it('admin-login is what Admin renders before there is a session', async () => {
    history.replaceState(null, '', '/admin');
    const { default: Admin } = await import('../views/admin/Admin.svelte');
    const { container } = testing.render(Admin);
    await testing.screen.findByRole('link', { name: 'Sign in with Discord' });
    const real = outlineOf(container, 'main');
    testing.cleanup();

    expect(outlineOf(await show('admin-login'), 'main')).toEqual(real);
  });

  it('admin-panel has the header Admin puts over a server’s panel', async () => {
    // The page builds its own client from the stored token: stand in for the
    // class with the mock's client, and a second server so that Admin offers
    // "Switch server", as the mock header does.
    vi.doMock('../lib/admin/client', async (original) => {
      const { createMockAdminApi } = await import('./fixtures');
      const stand = createMockAdminApi();
      return {
        ...(await original<typeof AdminClient>()),
        AdminApi: class {
          listGuilds = async () => [
            ...(await stand.listGuilds()),
            { guild_id: '900000000000000010', series_count: 0 },
          ];
          guild = stand.guild;
          options = stand.options;
        },
      };
    });
    const { storeToken } = await import('../lib/admin/session');
    storeToken('stored');
    history.replaceState(null, '', `/admin?guild=${GUILD_ID}`);
    const { default: Admin } = await import('../views/admin/Admin.svelte');
    const { container } = testing.render(Admin);
    await testing.screen.findByRole('heading', { level: 2, name: 'Settings' }, WAIT);
    const real = outlineOf(container, 'header');
    testing.cleanup();

    const mock = await show('admin-panel');
    await testing.screen.findByRole('heading', { level: 2, name: 'Settings' }, WAIT);
    expect(outlineOf(mock, 'header')).toEqual(real);
  });
});
