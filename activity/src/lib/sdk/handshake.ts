// The embedded-app authentication handshake, as pure logic over injectable
// dependencies so it can be unit-tested without Discord.
//
// Flow (per the Embedded App SDK):
//   ready() → authorize (OAuth code) → POST /api/token (exchange) →
//   authenticate (with the Discord access token).
//
// Discord answers one handshake per iframe, and a reload hangs `ready()`, so
// a failed boot is never restarted: `createBoot` keeps what each step
// produced and a second `run()` resumes at the step that failed.

import { ApiError } from '../utils/errors';
import { BootError, describeThrown, toBootError } from './bootError';
import type { AuthenticateResult, DiscordUser, Platform, SdkLike } from './types';

/** `identify` only: the gallery needs to know who is looking, nothing else. */
const DEFAULT_SCOPE = ['identify'];
/** Discord answers the handshake well inside a second when it answers at all. */
const READY_TIMEOUT_MS = 12_000;
const AUTHENTICATE_TIMEOUT_MS = 10_000;
/** RPC error codes that mean Discord refused this application or origin. */
const RPC_INVALID_CLIENT_ID = 4007;
const RPC_INVALID_ORIGIN = 4008;
/**
 * RPC error codes for a request the client could not take at all (a bad
 * payload, or a command an old client does not know). Never the person
 * saying no.
 */
const RPC_INVALID_PAYLOAD = 4000;
const RPC_INVALID_COMMAND = 4002;

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
  /** Which Discord client hosts the Activity. */
  platform: Platform;
  /**
   * The `custom_id` of the activity link that launched this instance, as
   * Discord passed it. Read it with `parseCustomId` (see `customId.ts`).
   */
  customId: string | null;
  /** leaf session token for `Authorization: Bearer`. */
  token: string;
  /** When `token` expires, epoch milliseconds. */
  expiresAt: number;
}

/** The part of a session that exists before Discord has confirmed the user. */
export type SessionAccess = Pick<Session, 'guildId' | 'token' | 'expiresAt'>;

/** The step a boot is on, for progress copy. */
export type BootStep = 'ready' | 'authorize' | 'exchange' | 'authenticate';

export interface BootHooks {
  /** Called as each step starts. */
  onStep?: (step: BootStep) => void;
  /**
   * Called once, when the leaf token exists. `authenticate` is still to
   * come, so loading that needs only the token can start alongside it.
   */
  onToken?: (access: SessionAccess) => void;
}

/** Dependencies for {@link createBoot} and {@link runHandshake}. */
export interface HandshakeDeps {
  clientId: string;
  sdk: SdkLike;
  /** Rejects with an `ApiError` (see `api/client.ts`). */
  exchangeToken: (code: string) => Promise<ExchangeResult>;
  scope?: string[];
  now?: () => number;
  readyTimeoutMs?: number;
  authenticateTimeoutMs?: number;
}

export interface Boot {
  /**
   * Runs the handshake, or resumes it at the step that failed last time.
   * Rejects with a `BootError`. Calls while one is in flight share it, and
   * once it has succeeded every call resolves to the same session.
   */
  run(hooks?: BootHooks): Promise<Session>;
  /**
   * Resolves when Discord answers the handshake, however late. After a
   * `ready_timeout` it tells the caller the boot can go on after all.
   */
  whenReady(): Promise<void>;
}

const DISCORD_HOST = /^(\d+)\.discordsays\.com$/i;

/**
 * The Discord application id to hand the SDK. Inside Discord every Activity
 * is served from `<application id>.discordsays.com`, so the hostname is the
 * one source that cannot disagree with the app being launched. The id baked
 * in at build time (`VITE_DISCORD_CLIENT_ID`) only stands in on other hosts.
 */
export function resolveClientId(hostname: string, configured: string | undefined): string | null {
  const fromHost = DISCORD_HOST.exec(hostname)?.[1];
  if (fromHost) return fromHost;
  const fromBuild = configured?.trim();
  return fromBuild ? fromBuild : null;
}

function withTimeout<T>(work: Promise<T>, ms: number, onTimeout: () => Error): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(onTimeout()), ms);
    work.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (e: unknown) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

/** `authorize: 5000 OAuth2 Error: access_denied`, for logs and "Details". */
function rpcDetail(command: string, e: unknown): string {
  const { code, text } = describeThrown(e);
  return `${command}: ${code === null ? '' : `${code} `}${text}`;
}

/**
 * A failed exchange as a boot error, and whether the OAuth code is spent
 * (so the next attempt needs a fresh one rather than the same request again).
 */
function exchangeFailure(e: unknown): { error: BootError; spent: boolean } {
  if (!(e instanceof ApiError)) return { error: toBootError(e, 'exchange'), spent: false };
  const detail = e.detail ? `${e.message} (${e.detail})` : e.message;
  if (e.code === 'code_rejected') {
    return { error: new BootError('sign_in_rejected', detail, e), spent: true };
  }
  if (e.code === 'discord_unavailable') {
    return { error: new BootError('discord_unavailable', detail, e), spent: false };
  }
  if (e.kind === 'network' || e.kind === 'timeout') {
    return { error: new BootError('network', detail, e), spent: false };
  }
  // Any other 4xx: an older server answers 400 for a refused code and for a
  // Discord outage alike, so treat the code as spent but keep Retry.
  return { error: new BootError('server', detail, e), spent: e.kind === 'rejected' };
}

/**
 * A resumable handshake over one SDK instance. Create it once per page:
 * Discord does not answer a second handshake in the same iframe.
 */
export function createBoot(deps: HandshakeDeps): Boot {
  const { clientId, sdk, exchangeToken } = deps;
  const scope = deps.scope ?? DEFAULT_SCOPE;
  const now = deps.now ?? Date.now;
  const readyTimeoutMs = deps.readyTimeoutMs ?? READY_TIMEOUT_MS;
  const authenticateTimeoutMs = deps.authenticateTimeoutMs ?? AUTHENTICATE_TIMEOUT_MS;

  // What each step produced, kept across attempts.
  let ready: Promise<void> | null = null;
  let code: string | null = null;
  /** Whether Discord has handed out a code in this session (consent exists). */
  let authorized = false;
  let exchanged: { result: ExchangeResult; expiresAt: number } | null = null;
  /** Every `authenticate` sent and not refused; a late answer still counts. */
  const authenticating: Promise<AuthenticateResult>[] = [];
  let session: Session | null = null;

  let running: Promise<Session> | null = null;
  let hooks: BootHooks = {};

  function whenReady(): Promise<void> {
    ready ??= sdk.ready().catch((e: unknown) => {
      // The real SDK never rejects; a fake might. Let the next attempt ask again.
      ready = null;
      throw e;
    });
    return ready;
  }

  async function authorize(): Promise<string> {
    hooks.onStep?.('authorize');
    try {
      const granted = await sdk.commands.authorize({
        client_id: clientId,
        response_type: 'code',
        state: '',
        prompt: 'none',
        scope,
      });
      authorized = true;
      return granted.code;
    } catch (e) {
      const detail = rpcDetail('authorize', e);
      const rpcCode = describeThrown(e).code;
      if (rpcCode === RPC_INVALID_CLIENT_ID || rpcCode === RPC_INVALID_ORIGIN) {
        throw new BootError('misconfigured', detail, e);
      }
      if (rpcCode === RPC_INVALID_PAYLOAD || rpcCode === RPC_INVALID_COMMAND) {
        throw new BootError('unknown', detail, e);
      }
      // Which code a dismissed permission sheet sends is not documented, so
      // anything else reads as a decline (its screen still shows the detail).
      // Once Discord has granted a code, a refusal is not the person saying
      // no: asking again in one session is not something Discord promises.
      throw new BootError(authorized ? 'unknown' : 'consent_declined', detail, e);
    }
  }

  async function signIn(): Promise<{ result: ExchangeResult; expiresAt: number }> {
    // A code held from an earlier attempt may have been spent by a request
    // whose answer never arrived, so one refusal of it earns a fresh code.
    let held = code !== null;
    for (;;) {
      code ??= await authorize();
      hooks.onStep?.('exchange');
      try {
        const result = await exchangeToken(code);
        return { result, expiresAt: now() + result.expires_in * 1000 };
      } catch (e) {
        const failure = exchangeFailure(e);
        if (failure.spent) {
          code = null;
          if (held) {
            held = false;
            continue;
          }
        }
        throw failure.error;
      }
    }
  }

  async function authenticate(accessToken: string): Promise<AuthenticateResult> {
    hooks.onStep?.('authenticate');
    // A command that timed out may still be answered. Keep listening to it
    // next to the new one rather than lose a late success.
    authenticating.push(sdk.commands.authenticate({ access_token: accessToken }));
    try {
      return await withTimeout(
        Promise.any(authenticating),
        authenticateTimeoutMs,
        () => new BootError('unknown', 'authenticate: no answer from Discord'),
      );
    } catch (e) {
      if (e instanceof BootError) throw e;
      // Every attempt was refused: start clean next time.
      authenticating.length = 0;
      const errors: unknown[] = e instanceof AggregateError ? e.errors : [e];
      const last = errors[errors.length - 1];
      throw new BootError('unknown', rpcDetail('authenticate', last), last);
    }
  }

  async function attempt(): Promise<Session> {
    // Before anything that can prompt: a DM launch has no gallery to sign in to.
    if (!sdk.guildId) {
      throw new BootError('no_guild', 'launched without a guild_id (DM or group DM)');
    }

    hooks.onStep?.('ready');
    await withTimeout(
      whenReady(),
      readyTimeoutMs,
      () => new BootError('ready_timeout', 'ready: no answer from Discord'),
    );

    if (!exchanged) {
      exchanged = await signIn();
      hooks.onToken?.({
        guildId: sdk.guildId,
        token: exchanged.result.token,
        expiresAt: exchanged.expiresAt,
      });
    }

    const authed = await authenticate(exchanged.result.access_token);

    session = {
      user: authed.user,
      guildId: sdk.guildId,
      channelId: sdk.channelId,
      platform: sdk.platform,
      customId: sdk.customId,
      token: exchanged.result.token,
      expiresAt: exchanged.expiresAt,
    };
    return session;
  }

  return {
    run(next: BootHooks = {}): Promise<Session> {
      if (session) return Promise.resolve(session);
      if (!running) {
        hooks = next;
        running = attempt()
          .catch((e: unknown) => {
            throw toBootError(e);
          })
          .finally(() => {
            running = null;
          });
      }
      return running;
    },
    whenReady,
  };
}

/** Runs the four-step handshake once and resolves to a {@link Session}. */
export function runHandshake(deps: HandshakeDeps): Promise<Session> {
  return createBoot(deps).run();
}

/** Preferred label for a user: display name, else username. */
export function displayName(user: DiscordUser): string {
  return user.global_name ?? user.username;
}
