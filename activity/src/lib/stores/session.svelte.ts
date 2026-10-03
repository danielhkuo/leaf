// Boot/session state as a runes module (Svelte 5 universal reactivity).
// Components read `session.value`; `bootSession()` drives the handshake and
// is also the Retry: the SDK module keeps what each step produced, so a
// second call resumes at the step that failed instead of starting over.

import { BootError, bootErrorCopy, toBootError } from '../sdk/bootError';
import type * as Discord from '../sdk/discord';
import type { BootStep, Session, SessionAccess } from '../sdk/handshake';

/** `load` is the SDK chunk downloading, before any handshake step. */
export type BootProgress = BootStep | 'load';

export type SessionState =
  | { status: 'loading'; step: BootProgress; slow: boolean }
  | { status: 'authed'; session: Session }
  | { status: 'error'; error: BootError };

export interface BootSessionHooks {
  /** Called once the leaf token exists, before Discord confirms the user. */
  onToken?: (access: SessionAccess) => void;
}

/** After this long on one step the copy admits it is taking a while. */
const SLOW_AFTER_MS = 4_000;
/**
 * A stalled phone connection can hold the SDK chunk's download open for
 * minutes before the browser gives up; stop waiting on it well before that.
 */
const LOAD_TIMEOUT_MS = 30_000;

export const session = $state<{ value: SessionState }>({
  value: { status: 'loading', step: 'load', slow: false },
});

/**
 * The line under the leaf while booting. `authorize` is never timed out (the
 * permission sheet may be open), so its slow copy says what it is waiting on.
 */
export function bootStatusText(step: BootProgress, slow: boolean): string {
  if (step === 'load' || step === 'ready') {
    return slow ? 'Still connecting to Discord…' : 'Opening leaf…';
  }
  if (!slow) return 'Signing you in…';
  return step === 'authorize' ? 'Waiting for your OK in Discord…' : 'Still signing you in…';
}

type SdkModule = typeof Discord;

let sdkModule: Promise<SdkModule> | null = null;
let inflight: Promise<void> | null = null;
let slowTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * The SDK chunk (Discord SDK, API client, zod), downloaded once. Lazy so the
 * loading shell paints before the heavy handshake code is fetched, which
 * keeps the initial chunk small — see the bundle budget.
 */
function loadSdk(): Promise<SdkModule> {
  if (!sdkModule) {
    const started = import('../sdk/discord');
    sdkModule = started;
    // A failed download is forgotten so the next attempt asks again.
    started.catch(() => {
      if (sdkModule === started) sdkModule = null;
    });
  }
  return sdkModule;
}

/**
 * Starts downloading the SDK chunk without waiting for the shell to mount.
 * `bootSession` picks the same download up and reports it if it fails.
 */
export function preloadSdk(): void {
  loadSdk().catch(() => undefined);
}

/** {@link loadSdk}, giving up (but not cancelling) after `ms`. */
function loadSdkWithin(ms: number): Promise<SdkModule> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new BootError('load_failed', `the SDK chunk did not arrive within ${ms / 1000} s`));
    }, ms);
    loadSdk().then(
      (sdk) => {
        clearTimeout(timer);
        resolve(sdk);
      },
      (e: unknown) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

/** Steps whose copy reads the same, slow or not. */
function sameCopy(a: BootProgress, b: BootProgress): boolean {
  return (
    bootStatusText(a, false) === bootStatusText(b, false) &&
    bootStatusText(a, true) === bootStatusText(b, true)
  );
}

function stopSlowClock(): void {
  clearTimeout(slowTimer);
  slowTimer = undefined;
}

function setStep(step: BootProgress): void {
  const was = session.value;
  // `load` and `ready` read the same, so the clock runs across both: a slow
  // download and then a slow answer still admit to being slow.
  if (was.status === 'loading' && slowTimer !== undefined && sameCopy(was.step, step)) {
    session.value = { ...was, step };
    return;
  }
  stopSlowClock();
  session.value = { status: 'loading', step, slow: false };
  slowTimer = setTimeout(() => {
    if (session.value.status === 'loading') session.value = { ...session.value, slow: true };
  }, SLOW_AFTER_MS);
}

/**
 * Shows a failed boot. If `late` resolves while that screen is still up,
 * the boot carries on by itself rather than leave the person on it.
 */
function fail(error: BootError, hooks: BootSessionHooks, late?: Promise<unknown>): void {
  session.value = { status: 'error', error };
  void late?.then(
    () => {
      if (session.value.status === 'error' && session.value.error === error) {
        void bootSession(hooks);
      }
    },
    () => undefined,
  );
}

async function run(hooks: BootSessionHooks): Promise<void> {
  setStep('load');
  let sdk: SdkModule;
  try {
    sdk = await loadSdkWithin(LOAD_TIMEOUT_MS);
  } catch (e) {
    stopSlowClock();
    console.error('leaf: the SDK chunk did not load', e);
    if (e instanceof BootError) {
      // Timed out: the download may still finish.
      fail(e, hooks, loadSdk());
    } else {
      const detail = e instanceof Error ? e.message : 'the SDK chunk did not load';
      fail(new BootError('load_failed', detail, e), hooks);
    }
    return;
  }
  try {
    const s = await sdk.boot({
      onStep: setStep,
      ...(hooks.onToken ? { onToken: hooks.onToken } : {}),
    });
    stopSlowClock();
    session.value = { status: 'authed', session: s };
  } catch (e) {
    stopSlowClock();
    const error = toBootError(e);
    // A DM launch or a declined prompt is a state to explain, not a fault.
    const log = bootErrorCopy(error.kind).tone === 'error' ? console.error : console.warn;
    log(`leaf: boot stopped (${error.kind})`, error);
    // After a `ready()` timeout Discord may still answer.
    fail(error, hooks, error.kind === 'ready_timeout' ? sdk.whenReady() : undefined);
  }
}

/**
 * Runs the SDK handshake, moving the store through loading → authed/error.
 * Call it again from an error screen to retry: only the failed step and the
 * ones after it are repeated. A call while one is running joins it.
 */
export function bootSession(hooks: BootSessionHooks = {}): Promise<void> {
  if (session.value.status === 'authed') return Promise.resolve();
  inflight ??= run(hooks).finally(() => {
    inflight = null;
  });
  return inflight;
}
