// The embedded-app authentication handshake, as a pure function over
// injectable dependencies so it can be unit-tested without Discord.
//
// Flow (per the Embedded App SDK):
//   ready() → authorize (OAuth code) → POST /api/token (exchange) →
//   authenticate (with the Discord access token).

import type { DiscordUser, SdkLike } from './types';

const DEFAULT_SCOPE = ['identify', 'guilds'];

/** What `POST /api/token` returns: our session token + the Discord token. */
export interface ExchangeResult {
  /** leaf session token (HMAC) — gates the leaf API. */
  token: string;
  /** Discord OAuth access token — feeds `sdk.commands.authenticate`. */
  access_token: string;
  /** Lifetime of `token`, in seconds. */
  expires_in: number;
}

/** A fully-authenticated session, the handshake's product. */
export interface Session {
  user: DiscordUser;
  guildId: string | null;
  channelId: string | null;
  /** leaf session token for `Authorization: Bearer`. */
  token: string;
  /** When `token` expires, epoch milliseconds. */
  expiresAt: number;
}

/** Dependencies for {@link runHandshake}. */
export interface HandshakeDeps {
  clientId: string;
  sdk: SdkLike;
  exchangeToken: (code: string) => Promise<ExchangeResult>;
  scope?: string[];
  now?: () => number;
}

/** Runs the four-step handshake and resolves to a {@link Session}. */
export async function runHandshake(deps: HandshakeDeps): Promise<Session> {
  const { clientId, sdk, exchangeToken } = deps;
  const scope = deps.scope ?? DEFAULT_SCOPE;
  const now = deps.now ?? Date.now;

  await sdk.ready();

  const { code } = await sdk.commands.authorize({
    client_id: clientId,
    response_type: 'code',
    state: '',
    prompt: 'none',
    scope,
  });

  const exchanged = await exchangeToken(code);

  const authed = await sdk.commands.authenticate({
    access_token: exchanged.access_token,
  });

  return {
    user: authed.user,
    guildId: sdk.guildId,
    channelId: sdk.channelId,
    token: exchanged.token,
    expiresAt: now() + exchanged.expires_in * 1000,
  };
}

/** Preferred label for a user: display name, else username. */
export function displayName(user: DiscordUser): string {
  return user.global_name ?? user.username;
}
