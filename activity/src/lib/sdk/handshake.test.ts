import { describe, expect, it, vi } from 'vitest';

import { ApiError } from '../utils/errors';
import { BootError, bootErrorCopy, describeThrown, toBootError } from './bootError';
import {
  createBoot,
  displayName,
  resolveClientId,
  runHandshake,
  type ExchangeResult,
  type HandshakeDeps,
} from './handshake';
import type { AuthenticateResult, DiscordUser, SdkLike } from './types';

const USER: DiscordUser = { id: '42', username: 'johan', global_name: 'Johan' };
const TOKENS: ExchangeResult = { token: 'leaf-tok', access_token: 'discord-at', expires_in: 3600 };

type Authorize = SdkLike['commands']['authorize'];
type Authenticate = SdkLike['commands']['authenticate'];

/** A controllable fake SDK that records the call order it observes. */
function fakeSdk(
  order: string[],
  over: {
    guildId?: string | null;
    customId?: string | null;
    ready?: () => Promise<void>;
    authorize?: Authorize;
    authenticate?: Authenticate;
  } = {},
) {
  const authorize = vi.fn<Authorize>((args) => {
    order.push('authorize');
    return over.authorize ? over.authorize(args) : Promise.resolve({ code: 'CODE' });
  });
  const authenticate = vi.fn<Authenticate>((args) => {
    order.push('authenticate');
    return over.authenticate
      ? over.authenticate(args)
      : Promise.resolve<AuthenticateResult>({
          access_token: args.access_token,
          user: USER,
          scopes: ['identify'],
          expires: '',
        });
  });
  const ready = vi.fn(() => {
    order.push('ready');
    return over.ready ? over.ready() : Promise.resolve();
  });
  const sdk: SdkLike = {
    guildId: over.guildId === undefined ? 'g1' : over.guildId,
    channelId: 'c1',
    platform: 'mobile',
    customId: over.customId ?? null,
    ready,
    commands: { authorize, authenticate },
  };
  return { sdk, ready, authorize, authenticate };
}

/** An exchange that records itself and answers from a queue of outcomes. */
function fakeExchange(order: string[], outcomes: (ExchangeResult | Error)[] = [TOKENS]) {
  const queue = [...outcomes];
  return vi.fn((code: string): Promise<ExchangeResult> => {
    order.push(`exchange:${code}`);
    const next = queue.shift() ?? TOKENS;
    return next instanceof Error ? Promise.reject(next) : Promise.resolve(next);
  });
}

function deps(over: Partial<HandshakeDeps> & Pick<HandshakeDeps, 'sdk'>): HandshakeDeps {
  return { clientId: 'app-id', exchangeToken: fakeExchange([]), now: () => 1000, ...over };
}

/** Runs `work`, expecting a `BootError`, and hands it back. */
async function bootError(work: Promise<unknown>): Promise<BootError> {
  const e: unknown = await work.then(
    () => undefined,
    (thrown: unknown) => thrown,
  );
  expect(e).toBeInstanceOf(BootError);
  return e as BootError;
}

describe('runHandshake', () => {
  it('drives ready → authorize → exchange → authenticate into a session', async () => {
    const order: string[] = [];
    const { sdk, authenticate } = fakeSdk(order, { customId: 's7d3' });
    const exchangeToken = fakeExchange(order);

    const result = await runHandshake(deps({ sdk, exchangeToken }));

    expect(order).toEqual(['ready', 'authorize', 'exchange:CODE', 'authenticate']);
    expect(result).toEqual({
      user: USER,
      guildId: 'g1',
      channelId: 'c1',
      platform: 'mobile',
      customId: 's7d3',
      token: 'leaf-tok',
      expiresAt: 1000 + 3600 * 1000,
    });
    // The Discord access token (not our session token) authenticates the SDK.
    expect(authenticate).toHaveBeenCalledWith({ access_token: 'discord-at' });
  });

  it('asks for the identify scope and nothing else', async () => {
    const { sdk, authorize } = fakeSdk([]);

    await runHandshake(deps({ sdk }));

    expect(authorize).toHaveBeenCalledWith({
      client_id: 'app-id',
      response_type: 'code',
      state: '',
      prompt: 'none',
      scope: ['identify'],
    });
  });
});

describe('createBoot', () => {
  it('reports the step it is on and hands out the token before authenticate', async () => {
    const order: string[] = [];
    const { sdk } = fakeSdk(order);
    const onToken = vi.fn(() => void order.push('token'));
    const steps: string[] = [];

    await createBoot(deps({ sdk, exchangeToken: fakeExchange(order) })).run({
      onStep: (step) => void steps.push(step),
      onToken,
    });

    expect(steps).toEqual(['ready', 'authorize', 'exchange', 'authenticate']);
    expect(order).toEqual(['ready', 'authorize', 'exchange:CODE', 'token', 'authenticate']);
    expect(onToken).toHaveBeenCalledOnce();
    expect(onToken).toHaveBeenCalledWith({
      guildId: 'g1',
      token: 'leaf-tok',
      expiresAt: 1000 + 3600 * 1000,
    });
  });

  it('stops before any prompt when launched outside a server', async () => {
    const order: string[] = [];
    const { sdk } = fakeSdk(order, { guildId: null });

    const e = await bootError(createBoot(deps({ sdk })).run());

    expect(e.kind).toBe('no_guild');
    expect(order).toEqual([]);
  });

  it('reads the plain object the SDK rejects with when consent is declined', async () => {
    const order: string[] = [];
    const exchangeToken = fakeExchange(order);
    const { sdk } = fakeSdk(order, {
      // The real SDK rejects with Discord's payload, not an Error.
      authorize: () => Promise.reject({ code: 5000, message: 'OAuth2 Error: access_denied' }),
    });

    const e = await bootError(createBoot(deps({ sdk, exchangeToken })).run());

    expect(e.kind).toBe('consent_declined');
    expect(e.detail).toBe('authorize: 5000 OAuth2 Error: access_denied');
    expect(e.detail).not.toContain('[object Object]');
    expect(exchangeToken).not.toHaveBeenCalled();
  });

  it('asks again on the same SDK when consent is granted on a retry', async () => {
    const order: string[] = [];
    let declined = false;
    const { sdk, ready } = fakeSdk(order, {
      authorize: () => {
        if (declined) return Promise.resolve({ code: 'CODE' });
        declined = true;
        return Promise.reject({ code: 5000, message: 'denied' });
      },
    });
    const boot = createBoot(deps({ sdk, exchangeToken: fakeExchange(order) }));

    await bootError(boot.run());
    const session = await boot.run();

    expect(session.token).toBe('leaf-tok');
    expect(ready).toHaveBeenCalledOnce();
    expect(order).toEqual(['ready', 'authorize', 'authorize', 'exchange:CODE', 'authenticate']);
  });

  it('calls a refused application id or origin a misconfiguration', async () => {
    const { sdk } = fakeSdk([], {
      authorize: () => Promise.reject({ code: 4007, message: 'Invalid Client ID' }),
    });

    const e = await bootError(createBoot(deps({ sdk })).run());

    expect(e.kind).toBe('misconfigured');
  });

  it('does not read a request the client could not take as a declined consent', async () => {
    for (const code of [4000, 4002]) {
      const { sdk } = fakeSdk([], {
        authorize: () => Promise.reject({ code, message: 'Invalid command' }),
      });

      const e = await bootError(createBoot(deps({ sdk })).run());

      expect(e.kind).toBe('unknown');
      expect(e.detail).toBe(`authorize: ${code} Invalid command`);
    }
  });

  it('retries a failed exchange with the same code, without ready or authorize again', async () => {
    const order: string[] = [];
    const { sdk, ready, authorize } = fakeSdk(order);
    const exchangeToken = fakeExchange(order, [
      new ApiError(0, 'POST /token → no response', undefined, { kind: 'network' }),
      TOKENS,
    ]);
    const boot = createBoot(deps({ sdk, exchangeToken }));

    const e = await bootError(boot.run());
    expect(e.kind).toBe('network');

    const session = await boot.run();

    expect(session.token).toBe('leaf-tok');
    expect(ready).toHaveBeenCalledOnce();
    expect(authorize).toHaveBeenCalledOnce();
    expect(order).toEqual(['ready', 'authorize', 'exchange:CODE', 'exchange:CODE', 'authenticate']);
  });

  it('maps a Discord outage to its own retryable kind', async () => {
    const { sdk } = fakeSdk([]);
    const exchangeToken = fakeExchange(
      [],
      [new ApiError(503, 'POST /token → 503', 'discord_unavailable')],
    );

    const e = await bootError(createBoot(deps({ sdk, exchangeToken })).run());

    expect(e.kind).toBe('discord_unavailable');
    expect(bootErrorCopy(e.kind).retry).toBe('Try again');
  });

  it('gets a fresh code once when a held code turns out to be spent', async () => {
    const order: string[] = [];
    let codes = 0;
    const { sdk, authorize } = fakeSdk(order, {
      authorize: () => Promise.resolve({ code: `CODE${++codes}` }),
    });
    const exchangeToken = fakeExchange(order, [
      new ApiError(0, 'POST /token → timed out', undefined, { kind: 'timeout' }),
      new ApiError(400, 'POST /token → 400', 'code_rejected'),
      TOKENS,
    ]);
    const boot = createBoot(deps({ sdk, exchangeToken }));

    await bootError(boot.run());
    const session = await boot.run();

    expect(session.token).toBe('leaf-tok');
    expect(authorize).toHaveBeenCalledTimes(2);
    expect(order).toEqual([
      'ready',
      'authorize',
      'exchange:CODE1',
      'exchange:CODE1',
      'authorize',
      'exchange:CODE2',
      'authenticate',
    ]);
  });

  it('does not repeat a fresh code that Discord refused', async () => {
    const order: string[] = [];
    const { sdk, authorize } = fakeSdk(order);
    const exchangeToken = fakeExchange(order, [
      new ApiError(400, 'POST /token → 400', 'code_rejected', { detail: 'invalid_client' }),
    ]);

    const e = await bootError(createBoot(deps({ sdk, exchangeToken })).run());

    expect(e.kind).toBe('sign_in_rejected');
    expect(e.detail).toBe('POST /token → 400 (invalid_client)');
    expect(bootErrorCopy(e.kind).retry).toBeNull();
    expect(authorize).toHaveBeenCalledOnce();
    expect(exchangeToken).toHaveBeenCalledOnce();
  });

  it('keeps Retry for an older server that answers 400 for every exchange failure', async () => {
    const order: string[] = [];
    let codes = 0;
    const { sdk } = fakeSdk(order, {
      authorize: () => Promise.resolve({ code: `CODE${++codes}` }),
    });
    const exchangeToken = fakeExchange(order, [
      new ApiError(400, 'POST /token → 400', 'bad_request'),
      TOKENS,
    ]);
    const boot = createBoot(deps({ sdk, exchangeToken }));

    const e = await bootError(boot.run());
    expect(e.kind).toBe('server');

    await boot.run();
    // The refused code is not sent again.
    expect(order).toEqual([
      'ready',
      'authorize',
      'exchange:CODE1',
      'authorize',
      'exchange:CODE2',
      'authenticate',
    ]);
  });

  it('does not blame the person when a second authorize is refused', async () => {
    const order: string[] = [];
    let calls = 0;
    const { sdk } = fakeSdk(order, {
      authorize: () =>
        ++calls === 1
          ? Promise.resolve({ code: 'CODE' })
          : Promise.reject({ code: 5000, message: 'Already authorized' }),
    });
    const exchangeToken = fakeExchange(order, [
      new ApiError(400, 'POST /token → 400', 'bad_request'),
    ]);
    const boot = createBoot(deps({ sdk, exchangeToken }));

    await bootError(boot.run());
    const e = await bootError(boot.run());

    expect(e.kind).toBe('unknown');
  });

  it('retries only authenticate when authenticate failed', async () => {
    const order: string[] = [];
    let refused = false;
    const { sdk, authorize } = fakeSdk(order, {
      authenticate: (args) => {
        if (!refused) {
          refused = true;
          return Promise.reject({ code: 4009, message: 'Invalid token' });
        }
        return Promise.resolve({
          access_token: args.access_token,
          user: USER,
          scopes: ['identify'],
          expires: '',
        });
      },
    });
    const exchangeToken = fakeExchange(order);
    const onToken = vi.fn();
    const boot = createBoot(deps({ sdk, exchangeToken }));

    const e = await bootError(boot.run({ onToken }));
    expect(e.kind).toBe('unknown');
    expect(e.detail).toBe('authenticate: 4009 Invalid token');

    const session = await boot.run({ onToken });

    expect(session.user).toEqual(USER);
    expect(authorize).toHaveBeenCalledOnce();
    expect(exchangeToken).toHaveBeenCalledOnce();
    expect(onToken).toHaveBeenCalledOnce();
    expect(order).toEqual(['ready', 'authorize', 'exchange:CODE', 'authenticate', 'authenticate']);
  });

  it('times out a handshake Discord never answers, and resumes if it answers late', async () => {
    vi.useFakeTimers();
    try {
      const order: string[] = [];
      let answer: () => void = () => undefined;
      const { sdk, ready } = fakeSdk(order, {
        ready: () => new Promise<void>((resolve) => (answer = resolve)),
      });
      const boot = createBoot(deps({ sdk, exchangeToken: fakeExchange(order) }));

      const first = bootError(boot.run());
      await vi.advanceTimersByTimeAsync(12_000);
      const e = await first;

      expect(e.kind).toBe('ready_timeout');
      expect(bootErrorCopy(e.kind)).toMatchObject({ retry: null, close: true });
      expect(order).toEqual(['ready']);

      const late = vi.fn();
      void boot.whenReady().then(late);
      answer();
      await vi.advanceTimersByTimeAsync(0);
      expect(late).toHaveBeenCalledOnce();

      const session = await boot.run();
      expect(session.token).toBe('leaf-tok');
      // The SDK is asked once; the second run waits on the same answer.
      expect(ready).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('never times out authorize: the permission sheet may be open', async () => {
    vi.useFakeTimers();
    try {
      let grant: (result: { code: string }) => void = () => undefined;
      const { sdk } = fakeSdk([], {
        authorize: () => new Promise((resolve) => (grant = resolve)),
      });
      const settled = vi.fn();
      const run = createBoot(deps({ sdk })).run();
      void run.then(settled, settled);

      await vi.advanceTimersByTimeAsync(10 * 60_000);
      expect(settled).not.toHaveBeenCalled();

      grant({ code: 'CODE' });
      await expect(run).resolves.toMatchObject({ token: 'leaf-tok' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('shares one run between callers and returns the same session afterwards', async () => {
    const order: string[] = [];
    const { sdk } = fakeSdk(order);
    const boot = createBoot(deps({ sdk, exchangeToken: fakeExchange(order) }));

    const [a, b] = await Promise.all([boot.run(), boot.run()]);
    const c = await boot.run();

    expect(a).toBe(b);
    expect(c).toBe(a);
    expect(order).toEqual(['ready', 'authorize', 'exchange:CODE', 'authenticate']);
  });
});

describe('resolveClientId', () => {
  it('reads the application id from the discordsays hostname', () => {
    expect(resolveClientId('1215413995645968394.discordsays.com', undefined)).toBe(
      '1215413995645968394',
    );
  });

  it('prefers the hostname over an id baked in at build time', () => {
    expect(resolveClientId('111.discordsays.com', '222')).toBe('111');
  });

  it('falls back to the built-in id on any other host', () => {
    expect(resolveClientId('leaf-dev.example.com', ' 222 ')).toBe('222');
    expect(resolveClientId('evil-discordsays.com', '222')).toBe('222');
    expect(resolveClientId('111.discordsays.com.example.com', '222')).toBe('222');
  });

  it('is null when neither source has one', () => {
    expect(resolveClientId('localhost', undefined)).toBeNull();
    expect(resolveClientId('localhost', '  ')).toBeNull();
  });
});

describe('boot errors', () => {
  it('reads Errors, SDK payloads, strings and junk alike', () => {
    expect(describeThrown(new Error('boom'))).toEqual({ code: null, text: 'boom' });
    expect(describeThrown({ code: 4002, message: 'Invalid command' })).toEqual({
      code: 4002,
      text: 'Invalid command',
    });
    expect(describeThrown('plain')).toEqual({ code: null, text: 'plain' });
    expect(describeThrown(undefined)).toEqual({ code: null, text: 'no message' });
    expect(describeThrown({})).toEqual({ code: null, text: 'no message' });
  });

  it('wraps anything that is not a BootError as unknown, keeping the cause', () => {
    const cause = { code: 1000, message: 'Unknown error' };
    const e = toBootError(cause, 'authenticate');

    expect(e.kind).toBe('unknown');
    expect(e.detail).toBe('authenticate: 1000 Unknown error');
    expect(e.cause).toBe(cause);

    const known = new BootError('network', 'x');
    expect(toBootError(known)).toBe(known);
  });

  it('has copy for every kind, and a way forward on each', () => {
    const kinds = [
      'no_guild',
      'not_in_discord',
      'misconfigured',
      'ready_timeout',
      'consent_declined',
      'network',
      'discord_unavailable',
      'sign_in_rejected',
      'server',
      'load_failed',
      'unknown',
    ] as const;
    for (const kind of kinds) {
      const copy = bootErrorCopy(kind);
      expect(copy.title).not.toBe('');
      expect(copy.message).toMatch(/\.$/);
      // Never internals: no status codes, env names or SDK parameter names.
      expect(`${copy.title} ${copy.message}`).not.toMatch(/VITE_|frame_id|\b[45]\d\d\b|\.env/);
    }
    expect(bootErrorCopy('consent_declined').retry).toBe('Grant access');
    expect(bootErrorCopy('load_failed').close).toBe(false);
    // A "declined" consent can be a broken setup: its detail stays reachable.
    expect(bootErrorCopy('consent_declined').details).toBe(true);
    expect(bootErrorCopy('no_guild').details).toBe(false);
  });
});

describe('displayName', () => {
  it('prefers global_name and falls back to username', () => {
    expect(displayName({ id: '1', username: 'u', global_name: 'Display' })).toBe('Display');
    expect(displayName({ id: '1', username: 'u', global_name: null })).toBe('u');
    expect(displayName({ id: '1', username: 'u' })).toBe('u');
  });
});
