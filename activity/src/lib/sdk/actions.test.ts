import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { closeActivity, onForeground, openExternalLink } from './actions';

type LayoutListener = (update: { layout_mode: number }) => void;

// The SDK module is mocked at its boundary: `getSdk` hands out this fake.
const sdk = vi.hoisted(() => ({
  available: true,
  openExternalLink: vi.fn(),
  subscribe: vi.fn(),
  unsubscribe: vi.fn(),
  closeSdk: vi.fn(),
}));

vi.mock('./discord', () => ({
  getSdk: () => {
    if (!sdk.available) throw new Error('SDK used before boot()');
    return {
      commands: { openExternalLink: sdk.openExternalLink },
      subscribe: sdk.subscribe,
      unsubscribe: sdk.unsubscribe,
    };
  },
  closeSdk: sdk.closeSdk,
}));

/** Lets the dynamic import and the subscribe inside `onForeground` settle. */
async function settle(): Promise<void> {
  await vi.dynamicImportSettled();
  await Promise.resolve();
}

function setVisibility(state: 'visible' | 'hidden'): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  document.dispatchEvent(new Event('visibilitychange'));
}

beforeEach(() => {
  sdk.available = true;
  sdk.openExternalLink.mockReset();
  sdk.subscribe.mockReset().mockResolvedValue(undefined);
  sdk.unsubscribe.mockReset().mockResolvedValue(undefined);
  sdk.closeSdk.mockReset().mockReturnValue(true);
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('openExternalLink', () => {
  it('reports a link Discord opened', async () => {
    sdk.openExternalLink.mockResolvedValue({ opened: true });

    await expect(openExternalLink('https://discord.com/channels/g/c/m')).resolves.toBe('opened');
    expect(sdk.openExternalLink).toHaveBeenCalledWith({
      url: 'https://discord.com/channels/g/c/m',
    });
  });

  it('treats "stay here" on Discord’s prompt as a cancel, not a failure', async () => {
    sdk.openExternalLink.mockResolvedValue({ opened: false });

    await expect(openExternalLink('https://example.com')).resolves.toBe('cancelled');
  });

  it('assumes an older client that cannot say did open it', async () => {
    sdk.openExternalLink.mockResolvedValue({ opened: null });

    await expect(openExternalLink('https://example.com')).resolves.toBe('opened');
  });

  it('resolves to failed instead of rejecting when the command is refused', async () => {
    sdk.openExternalLink.mockRejectedValue({ code: 4002, message: 'Invalid command' });

    await expect(openExternalLink('https://example.com')).resolves.toBe('failed');
  });

  it('resolves to failed when there is no SDK', async () => {
    sdk.available = false;

    await expect(openExternalLink('https://example.com')).resolves.toBe('failed');
  });
});

describe('closeActivity', () => {
  it('says whether the close request went out', async () => {
    await expect(closeActivity()).resolves.toBe(true);

    sdk.closeSdk.mockReturnValue(false);
    await expect(closeActivity()).resolves.toBe(false);
  });

  it('never rejects', async () => {
    sdk.closeSdk.mockImplementation(() => {
      throw new Error('postMessage failed');
    });

    await expect(closeActivity()).resolves.toBe(false);
  });
});

describe('onForeground', () => {
  /** The listener `onForeground` registered for layout-mode updates. */
  function layoutListener(): LayoutListener {
    const call = sdk.subscribe.mock.calls[0] as [string, LayoutListener] | undefined;
    if (!call) throw new Error('onForeground did not subscribe');
    expect(call[0]).toBe('ACTIVITY_LAYOUT_MODE_UPDATE');
    return call[1];
  }

  it('fires when the page becomes visible again, not when it is hidden', async () => {
    const callback = vi.fn();
    const stop = onForeground(callback);
    await settle();

    setVisibility('hidden');
    expect(callback).not.toHaveBeenCalled();

    setVisibility('visible');
    expect(callback).toHaveBeenCalledOnce();
    stop();
  });

  it('fires when the layout returns to focused, but not for the mode at subscribe', async () => {
    const callback = vi.fn();
    const stop = onForeground(callback);
    await settle();
    vi.useFakeTimers();
    const layout = layoutListener();

    // Discord publishes the current mode straight away.
    layout({ layout_mode: 0 });
    expect(callback).not.toHaveBeenCalled();

    layout({ layout_mode: 1 });
    expect(callback).not.toHaveBeenCalled();

    layout({ layout_mode: 0 });
    expect(callback).toHaveBeenCalledOnce();

    // Grid to focused counts as well.
    vi.advanceTimersByTime(5_000);
    layout({ layout_mode: 2 });
    layout({ layout_mode: 0 });
    expect(callback).toHaveBeenCalledTimes(2);
    stop();
  });

  it('counts signals that arrive together once', async () => {
    const callback = vi.fn();
    const stop = onForeground(callback);
    await settle();
    vi.useFakeTimers();
    const layout = layoutListener();
    layout({ layout_mode: 1 });

    layout({ layout_mode: 0 });
    setVisibility('visible');
    window.dispatchEvent(new Event('focus'));
    expect(callback).toHaveBeenCalledOnce();

    vi.advanceTimersByTime(1_000);
    window.dispatchEvent(new Event('focus'));
    expect(callback).toHaveBeenCalledTimes(2);
    stop();
  });

  it('stops listening everywhere when the returned function is called', async () => {
    const callback = vi.fn();
    const stop = onForeground(callback);
    await settle();
    const layout = layoutListener();

    stop();

    expect(sdk.unsubscribe).toHaveBeenCalledWith('ACTIVITY_LAYOUT_MODE_UPDATE', layout);
    layout({ layout_mode: 1 });
    layout({ layout_mode: 0 });
    setVisibility('visible');
    window.dispatchEvent(new Event('focus'));
    expect(callback).not.toHaveBeenCalled();
  });

  it('unsubscribes when stopped before the subscription finished', async () => {
    const stop = onForeground(vi.fn());
    stop();
    await settle();

    expect(sdk.unsubscribe).toHaveBeenCalledOnce();
  });

  it('still works from page signals when Discord refuses the subscription', async () => {
    sdk.subscribe.mockRejectedValue({ code: 4004, message: 'Invalid event' });
    const callback = vi.fn();
    const stop = onForeground(callback);
    await settle();

    setVisibility('visible');
    expect(callback).toHaveBeenCalledOnce();
    stop();
  });

  it('still works from page signals when there is no SDK', async () => {
    sdk.available = false;
    const callback = vi.fn();
    const stop = onForeground(callback);
    await settle();

    window.dispatchEvent(new Event('focus'));
    expect(callback).toHaveBeenCalledOnce();
    expect(sdk.subscribe).not.toHaveBeenCalled();
    stop();
  });
});
