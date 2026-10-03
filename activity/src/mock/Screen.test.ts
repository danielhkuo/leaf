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
import { bootScreens, GUILD_ID } from './fixtures';
import { SCREENS, type ScreenId } from './screens';

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

async function show(id: string, longText = false): Promise<HTMLElement> {
  const { default: Screen } = await import('./Screen.svelte');
  return testing.render(Screen, { props: { id, longText } }).container;
}

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
  viewer: (s) => s.findByRole('dialog', { name: 'Day 125, Daily Sketch' }, WAIT),
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
};

/** The screens whose scenario fails a request on purpose, and what that logs. */
const LOGGED: Partial<Record<ScreenId, string[]>> = {
  expired: ['leaf: loading the gallery failed'],
  'load-error': ['leaf: loading the gallery failed'],
  'viewer-failed': ['leaf: loading Day 124 failed'],
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

describe('mock screens copied from a real one', () => {
  it.each([...bootScreens])('%s is what App renders in that boot state', async (id, state) => {
    vi.doMock('../lib/stores/session.svelte', async (original) => ({
      ...(await original<typeof SessionStore>()),
      // The real one starts the Discord handshake, which nothing here answers.
      bootSession: () => Promise.resolve(),
    }));
    const { session } = await import('../lib/stores/session.svelte');
    session.value = state;
    const { default: App } = await import('../App.svelte');
    const real = outlineOf(testing.render(App).container, 'main');
    testing.cleanup();

    expect(outlineOf(await show(id), 'main')).toEqual(real);
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
