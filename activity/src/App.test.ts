import type * as TestingLibrary from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as BootErrorModule from './lib/sdk/bootError';
import type { BootHooks, Session } from './lib/sdk/handshake';
import { expectNoA11yViolations } from './lib/test/a11y';

// The session store keeps module state, so each test loads a fresh App. The
// SDK module and the gallery are mocked at their boundaries.
const sdk = vi.hoisted(() => ({
  boot: vi.fn(),
  closeSdk: vi.fn(),
  /** What the stand-in gallery does when it renders. */
  gallery: vi.fn(),
}));

const SESSION: Session = {
  user: { id: 'u1', username: 'ann' },
  guildId: 'g1',
  channelId: 'c1',
  platform: 'mobile',
  appName: 'leaf',
  customId: null,
  token: 'tok',
  expiresAt: 1_000_000,
};

let BootError: (typeof BootErrorModule)['BootError'];
// Resetting modules also gives App a fresh Svelte runtime, and a component
// can only be mounted by the runtime it was compiled against: the testing
// library is loaded again after each reset so both share one.
let testing: typeof TestingLibrary;
let screen: (typeof TestingLibrary)['screen'];
let fireEvent: (typeof TestingLibrary)['fireEvent'];
let waitFor: (typeof TestingLibrary)['waitFor'];

async function renderApp(): Promise<HTMLElement> {
  const { default: App } = await import('./App.svelte');
  return testing.render(App).container;
}

function unhandled(reason: unknown): void {
  window.dispatchEvent(Object.assign(new Event('unhandledrejection'), { reason }));
}

beforeEach(async () => {
  vi.resetModules();
  sdk.boot.mockReset().mockResolvedValue(SESSION);
  sdk.closeSdk.mockReset().mockReturnValue(true);
  sdk.gallery.mockReset();
  vi.doMock('./lib/sdk/discord', () => ({
    boot: sdk.boot,
    closeSdk: sdk.closeSdk,
    whenReady: () => new Promise<void>(() => undefined),
  }));
  vi.doMock('./views/Gallery.svelte', () => ({ default: sdk.gallery }));
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  ({ BootError } = await import('./lib/sdk/bootError'));
  testing = await import('@testing-library/svelte');
  ({ screen, fireEvent, waitFor } = testing);
});

afterEach(() => {
  testing.cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('App boot screens', () => {
  it('shows progress while the handshake runs', async () => {
    sdk.boot.mockReturnValue(new Promise<Session>(() => undefined));

    await renderApp();

    expect(screen.getByText('Opening leaf…').closest('[role="status"]')).not.toBeNull();
  });

  it('explains a declined permission and asks again on the same boot', async () => {
    sdk.boot.mockRejectedValueOnce(new BootError('consent_declined', 'authorize: 5000 denied'));
    await renderApp();

    const grant = await screen.findByRole('button', { name: 'Grant access' });
    expect(screen.getByRole('heading', { name: 'leaf needs your OK' })).toBeInTheDocument();
    expect(screen.queryByText(/object Object/)).not.toBeInTheDocument();
    // Declining is not a failure: nothing is announced as an alert.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();

    await fireEvent.click(grant);

    await waitFor(() => expect(sdk.gallery).toHaveBeenCalled());
    expect(sdk.boot).toHaveBeenCalledTimes(2);
  });

  it('keeps the detail of a declined permission reachable, since a broken setup reads the same', async () => {
    sdk.boot.mockRejectedValue(new BootError('consent_declined', 'authorize: 5000 OAuth2 Error'));
    const container = await renderApp();
    await screen.findByRole('heading', { name: 'leaf needs your OK' });

    const details = container.querySelector('details');
    expect(details).not.toHaveAttribute('open');
    expect(details).toHaveTextContent('authorize: 5000 OAuth2 Error');
  });

  it('offers a way out when the permission prompt never answers', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    sdk.boot.mockImplementation((hooks: BootHooks) => {
      hooks.onStep?.('authorize');
      return new Promise<Session>(() => undefined);
    });
    await renderApp();
    await screen.findByText('Signing you in…');
    expect(screen.queryByRole('button', { name: 'Close leaf' })).not.toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(4_000);

    expect(screen.getByText('Waiting for your OK in Discord…')).toBeInTheDocument();
    expect(screen.getByText(/If Discord isn’t asking/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Close leaf' }));
    await waitFor(() => expect(sdk.closeSdk).toHaveBeenCalledOnce());
    await vi.advanceTimersByTimeAsync(1_500);
    expect(screen.getByText(/close it with Discord’s own controls/)).toBeInTheDocument();
  });

  it('shows no detail on the screen about where leaf was opened', async () => {
    sdk.boot.mockRejectedValue(new BootError('no_guild', 'launched without a guild_id'));
    const container = await renderApp();
    await screen.findByRole('heading', { name: 'leaf opens from a server' });

    expect(container.querySelector('details')).toBeNull();
  });

  it('sends a DM launch back to a server, with Close and no retry', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    sdk.boot.mockRejectedValue(new BootError('no_guild', 'launched without a guild_id'));
    await renderApp();

    expect(
      await screen.findByRole('heading', { name: 'leaf opens from a server' }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /again|Grant/ })).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Close leaf' }));
    await waitFor(() => expect(sdk.closeSdk).toHaveBeenCalledOnce());
    // The SDK stops listening once it has asked to close: no second press.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Close leaf' })).toBeDisabled());

    // Discord does not confirm a close; still here, so say how to leave.
    await vi.advanceTimersByTimeAsync(1_500);
    expect(screen.getByText(/close it with Discord’s own controls/)).toBeInTheDocument();
  });

  it('offers no retry when Discord refused the sign-in, and keeps the detail collapsed', async () => {
    sdk.boot.mockRejectedValue(
      new BootError('sign_in_rejected', 'POST /token → 400 (invalid_client)'),
    );
    const container = await renderApp();

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Couldn’t sign you in');
    expect(alert).toHaveTextContent('Tell a server admin.');
    expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Close leaf' })).toBeInTheDocument();

    const details = container.querySelector('details');
    expect(details).not.toHaveAttribute('open');
    expect(details).toHaveTextContent('POST /token → 400 (invalid_client)');
  });

  it('has no axe violations on an error screen', async () => {
    sdk.boot.mockRejectedValue(new BootError('network', 'POST /token → no response'));
    const container = await renderApp();
    await screen.findByRole('alert');

    await expectNoA11yViolations(container);
  });
});

describe('App once signed in', () => {
  it('hands the session to the gallery', async () => {
    await renderApp();

    await waitFor(() => expect(sdk.gallery).toHaveBeenCalled());
    const props = sdk.gallery.mock.calls[0]?.[1] as { session: Session };
    expect(props.session).toEqual(SESSION);
  });

  it('shows a passing notice for an error nothing else handled', async () => {
    await renderApp();
    await waitFor(() => expect(sdk.gallery).toHaveBeenCalled());

    unhandled(new DOMException('The play() request was interrupted', 'AbortError'));
    expect(screen.queryByText(/Something didn’t work/)).not.toBeInTheDocument();

    unhandled(new Error('jump failed'));
    expect(await screen.findByText('Something didn’t work. Try that again.')).toBeInTheDocument();
    expect(screen.queryByText('jump failed')).not.toBeInTheDocument();
  });

  it('gives the same notice for a script error, but not for a ResizeObserver loop', async () => {
    await renderApp();
    await waitFor(() => expect(sdk.gallery).toHaveBeenCalled());
    const thrown = (message: string): void => {
      window.dispatchEvent(new ErrorEvent('error', { message, error: new Error(message) }));
    };

    thrown('ResizeObserver loop completed with undelivered notifications.');
    expect(screen.queryByText(/Something didn’t work/)).not.toBeInTheDocument();

    thrown('effect failed');
    expect(await screen.findByText('Something didn’t work. Try that again.')).toBeInTheDocument();
  });

  it('does not log again for an error raised by reporting one', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    await renderApp();
    await waitFor(() => expect(sdk.gallery).toHaveBeenCalled());
    // Inside Discord the console is forwarded to the client; a line it
    // refuses would come back as another unhandled rejection.
    const error = vi.mocked(console.error);
    error.mockImplementation(() => unhandled(new Error('CAPTURE_LOG refused')));
    const reports = (): number =>
      error.mock.calls.filter(([first]) => first === 'leaf: unhandled error').length;

    unhandled(new Error('jump failed'));
    expect(reports()).toBe(1);
    expect(await screen.findByText('Something didn’t work. Try that again.')).toBeInTheDocument();

    // A later, separate error is still reported.
    vi.setSystemTime(Date.now() + 1_000);
    unhandled(new Error('save failed'));
    expect(reports()).toBe(2);
  });
});
