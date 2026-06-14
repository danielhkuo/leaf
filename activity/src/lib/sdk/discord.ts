// Wires the real Discord Embedded App SDK into the pure handshake. This is
// the one module that touches `@discord/embedded-app-sdk` directly; tests
// exercise `runHandshake` with a fake SDK instead of importing this.

import { DiscordSDK } from '@discord/embedded-app-sdk';

import { exchangeToken } from '../api/client';
import { runHandshake, type Session } from './handshake';
import type { SdkLike } from './types';

const CLIENT_ID = import.meta.env.VITE_DISCORD_CLIENT_ID;

/** Constructs the SDK and runs the handshake; resolves to a session. */
export async function boot(): Promise<Session> {
  if (!CLIENT_ID) {
    throw new Error(
      'VITE_DISCORD_CLIENT_ID is not set — copy activity/.env.example to ' +
        'activity/.env and set your Discord application (client) id.',
    );
  }
  const sdk = new DiscordSDK(CLIENT_ID);
  // The real SDK is a structural superset of `SdkLike`.
  return runHandshake({
    clientId: CLIENT_ID,
    sdk: sdk as unknown as SdkLike,
    exchangeToken: (code) => exchangeToken(code),
  });
}
