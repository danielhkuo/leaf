// Wires the real Discord Embedded App SDK into the pure handshake. This is
// the one module that touches `@discord/embedded-app-sdk` directly; tests
// exercise `createBoot` with a fake SDK instead of importing this.
//
// There is exactly one SDK instance and one boot per page. Discord answers a
// single handshake per iframe, so a second `new DiscordSDK()` would wait on
// `ready()` forever.

import { DiscordSDK, RPCCloseCodes } from '@discord/embedded-app-sdk';

import { exchangeToken } from '../api/client';
import { BootError, describeThrown } from './bootError';
import { forwardConsole } from './consoleForward';
import { createBoot, resolveClientId, type Boot, type BootHooks, type Session } from './handshake';
import type { SdkLike } from './types';

let sdkInstance: DiscordSDK | null = null;
let machine: Boot | null = null;
/** Why the SDK could not be constructed; it will not go better a second time. */
let startError: BootError | null = null;

/** The live SDK, available once {@link boot} has been called. */
export function getSdk(): DiscordSDK {
  if (!sdkInstance) throw new Error('SDK used before boot()');
  return sdkInstance;
}

function start(): Boot {
  if (machine) return machine;
  if (startError) throw startError;
  const built = import.meta.env.VITE_DISCORD_CLIENT_ID;
  const clientId = resolveClientId(location.hostname, built);
  if (!clientId) {
    startError = new BootError(
      'misconfigured',
      'no application id: this page is not on <id>.discordsays.com and VITE_DISCORD_CLIENT_ID is not set',
    );
    throw startError;
  }
  if (built && built.trim() !== clientId) {
    console.warn(`leaf: using application id ${clientId} from the hostname, not the built-in one`);
  }
  let sdk: DiscordSDK;
  try {
    // The SDK's own console forwarding leaks a rejection for every line the
    // client refuses; leaf forwards the console itself below instead.
    sdk = new DiscordSDK(clientId, { disableConsoleLogOverride: true });
  } catch (e) {
    // The constructor throws when Discord's launch parameters are missing.
    startError = new BootError('not_in_discord', `sdk: ${describeThrown(e).text}`, e);
    throw startError;
  }
  sdkInstance = sdk;
  machine = createBoot({
    clientId,
    // The real SDK is a structural superset of `SdkLike`.
    sdk: sdk as unknown as SdkLike,
    exchangeToken: (code) => exchangeToken(code),
  });
  // From READY on, as the SDK would, so a phone's Debug Logs show leaf's.
  void machine.whenReady().then(
    () => forwardConsole(console, (level, message) => sdk.commands.captureLog({ level, message })),
    () => undefined,
  );
  return machine;
}

/**
 * Runs the handshake and resolves to a session. Call it again after a
 * failure to resume at the step that failed; the SDK is built only once.
 * Rejects with a `BootError`.
 */
export async function boot(hooks?: BootHooks): Promise<Session> {
  return start().run(hooks);
}

/**
 * Resolves when Discord answers the handshake, however late; never, when the
 * SDK could not be built.
 */
export function whenReady(): Promise<void> {
  return machine ? machine.whenReady() : new Promise(() => undefined);
}

/**
 * Asks Discord to close the Activity. `false` when there is no SDK to ask
 * with. The SDK stops listening afterwards, so nothing else works once this
 * has been called.
 */
export function closeSdk(): boolean {
  if (!sdkInstance) return false;
  sdkInstance.close(RPCCloseCodes.CLOSE_NORMAL, 'Closed by the user');
  return true;
}
