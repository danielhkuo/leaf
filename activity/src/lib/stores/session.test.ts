import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as BootErrorModule from '../sdk/bootError';
import type { BootHooks, Session } from '../sdk/handshake';
import type * as StoreModule from './session.svelte';

// The store keeps module state (the SDK import, the run in flight), so each
// test loads a fresh copy. The SDK module is mocked at its boundary.
const sdk = vi.hoisted(() => ({
  boot: vi.fn(),
  whenReady: vi.fn(),
}));

const SESSION: Session = {
  user: { id: 'u1', username: 'ann' },
  guildId: 'g1',
  channelId: 'c1',
  platform: 'mobile',
  customId: null,
  token: 'tok',
  expiresAt: 1_000_000,
};

let store: typeof StoreModule;
// Loaded after each module reset so `instanceof` matches the store's copy.
let BootError: (typeof BootErrorModule)['BootError'];

beforeEach(async () => {
  vi.resetModules();
  sdk.boot.mockReset().mockResolvedValue(SESSION);
  sdk.whenReady.mockReset().mockReturnValue(new Promise<void>(() => undefined));
  vi.doMock('../sdk/discord', () => ({ boot: sdk.boot, whenReady: sdk.whenReady }));
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  store = await import('./session.svelte');
  ({ BootError } = await import('../sdk/bootError'));
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('bootStatusText', () => {
  it('names the stage, and what it is waiting on once it is slow', () => {
    const { bootStatusText } = store;
    expect(bootStatusText('load', false)).toBe('Opening leaf…');
    expect(bootStatusText('ready', false)).toBe('Opening leaf…');
    expect(bootStatusText('ready', true)).toBe('Still connecting to Discord…');
    expect(bootStatusText('authorize', false)).toBe('Signing you in…');
    expect(bootStatusText('authorize', true)).toBe('Waiting for your OK in Discord…');
    expect(bootStatusText('exchange', false)).toBe('Signing you in…');
    expect(bootStatusText('authenticate', true)).toBe('Still signing you in…');
  });
});

describe('bootSession', () => {
  it('moves from loading to authed', async () => {
    expect(store.session.value).toEqual({ status: 'loading', step: 'load', slow: false });

    await store.bootSession();

    expect(store.session.value).toEqual({ status: 'authed', session: SESSION });
  });

  it('follows the handshake step and passes the token hook through', async () => {
    const onToken = vi.fn();
    let finish: (s: Session) => void = () => undefined;
    sdk.boot.mockImplementation((hooks: BootHooks) => {
      hooks.onStep?.('authorize');
      hooks.onToken?.({ guildId: 'g1', token: 'tok', expiresAt: 1 });
      return new Promise<Session>((resolve) => (finish = resolve));
    });

    const done = store.bootSession({ onToken });
    await vi.waitFor(() => expect(sdk.boot).toHaveBeenCalled());

    expect(store.session.value).toEqual({ status: 'loading', step: 'authorize', slow: false });
    expect(onToken).toHaveBeenCalledWith({ guildId: 'g1', token: 'tok', expiresAt: 1 });

    finish(SESSION);
    await done;
    expect(store.session.value.status).toBe('authed');
  });

  it('admits a step is slow after four seconds, and starts over on the next step', async () => {
    vi.useFakeTimers();
    let step: (s: 'ready' | 'authorize') => void = () => undefined;
    sdk.boot.mockImplementation((hooks: BootHooks) => {
      step = (s) => hooks.onStep?.(s);
      return new Promise<Session>(() => undefined);
    });

    void store.bootSession();
    await vi.waitFor(() => expect(sdk.boot).toHaveBeenCalled());
    step('ready');

    await vi.advanceTimersByTimeAsync(4_000);
    expect(store.session.value).toEqual({ status: 'loading', step: 'ready', slow: true });

    step('authorize');
    expect(store.session.value).toEqual({ status: 'loading', step: 'authorize', slow: false });
  });

  it('keeps the typed error for the screen to describe', async () => {
    const declined = new BootError('consent_declined', 'authorize: 5000 denied');
    sdk.boot.mockRejectedValue(declined);

    await store.bootSession();

    expect(store.session.value).toEqual({ status: 'error', error: declined });
  });

  it('never shows "[object Object]" for a plain rejection', async () => {
    sdk.boot.mockRejectedValue({ code: 1000, message: 'Unknown error' });

    await store.bootSession();

    const state = store.session.value;
    if (state.status !== 'error') throw new Error('expected an error state');
    expect(state.error.kind).toBe('unknown');
    expect(state.error.detail).toBe('boot: 1000 Unknown error');
  });

  it('runs the boot again on retry, and resolves at once when already signed in', async () => {
    sdk.boot.mockRejectedValueOnce(new BootError('network', 'POST /token → no response'));

    await store.bootSession();
    expect(store.session.value.status).toBe('error');

    await store.bootSession();
    expect(store.session.value.status).toBe('authed');
    expect(sdk.boot).toHaveBeenCalledTimes(2);

    await store.bootSession();
    expect(sdk.boot).toHaveBeenCalledTimes(2);
  });

  it('joins a run already in flight instead of starting another', async () => {
    await Promise.all([store.bootSession(), store.bootSession()]);

    expect(sdk.boot).toHaveBeenCalledOnce();
  });

  it('carries on by itself when Discord answers after the ready timeout', async () => {
    let answer: () => void = () => undefined;
    sdk.whenReady.mockReturnValue(new Promise<void>((resolve) => (answer = resolve)));
    sdk.boot.mockRejectedValueOnce(new BootError('ready_timeout', 'ready: no answer'));

    await store.bootSession();
    expect(store.session.value.status).toBe('error');

    answer();
    await vi.waitFor(() => expect(store.session.value.status).toBe('authed'));
    expect(sdk.boot).toHaveBeenCalledTimes(2);
  });

  it('reports an SDK chunk that did not download, and loads it again on retry', async () => {
    vi.resetModules();
    vi.doMock('../sdk/discord', () => {
      throw new TypeError('Failed to fetch dynamically imported module');
    });
    store = await import('./session.svelte');

    await store.bootSession();

    const state = store.session.value;
    if (state.status !== 'error') throw new Error('expected an error state');
    expect(state.error.kind).toBe('load_failed');
    expect(sdk.boot).not.toHaveBeenCalled();

    vi.doMock('../sdk/discord', () => ({ boot: sdk.boot, whenReady: sdk.whenReady }));
    await store.bootSession();
    expect(store.session.value.status).toBe('authed');
  });

  it('stops waiting on a stalled SDK download, and carries on if it arrives', async () => {
    vi.useFakeTimers();
    const arrive = await slowSdkChunk();

    const done = store.bootSession();
    await vi.advanceTimersByTimeAsync(4_000);
    expect(store.session.value).toEqual({ status: 'loading', step: 'load', slow: true });

    await vi.advanceTimersByTimeAsync(26_000);
    await done;
    const state = store.session.value;
    if (state.status !== 'error') throw new Error('expected an error state');
    expect(state.error.kind).toBe('load_failed');

    arrive();
    await vi.waitFor(() => expect(store.session.value.status).toBe('authed'));
    expect(sdk.boot).toHaveBeenCalledOnce();
  });

  it('runs one slow clock across the download and the wait for Discord', async () => {
    vi.useFakeTimers();
    const arrive = await slowSdkChunk();
    sdk.boot.mockImplementation((hooks: BootHooks) => {
      hooks.onStep?.('ready');
      return new Promise<Session>(() => undefined);
    });

    void store.bootSession();
    await vi.advanceTimersByTimeAsync(3_000);
    arrive();
    await vi.waitFor(() => expect(sdk.boot).toHaveBeenCalled());
    expect(store.session.value).toEqual({ status: 'loading', step: 'ready', slow: false });

    // Both read "Opening leaf…", so four seconds in all is slow.
    await vi.advanceTimersByTimeAsync(1_000);
    expect(store.session.value).toEqual({ status: 'loading', step: 'ready', slow: true });
  });
});

/**
 * Reloads the store with an SDK chunk that downloads only when the returned
 * function is called.
 */
async function slowSdkChunk(): Promise<() => void> {
  let arrive: () => void = () => undefined;
  const arrived = new Promise<void>((resolve) => (arrive = resolve));
  vi.resetModules();
  vi.doMock('../sdk/discord', async () => {
    await arrived;
    return { boot: sdk.boot, whenReady: sdk.whenReady };
  });
  store = await import('./session.svelte');
  return () => arrive();
}
