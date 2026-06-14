import { describe, expect, it, vi } from 'vitest';

import { displayName, runHandshake, type ExchangeResult } from './handshake';
import type { AuthenticateResult, DiscordUser, SdkLike } from './types';

const USER: DiscordUser = { id: '42', username: 'johan', global_name: 'Johan' };

/** A controllable fake SDK that records the call order it observes. */
function fakeSdk(order: string[], over: Partial<SdkLike> = {}): SdkLike {
  return {
    guildId: 'g1',
    channelId: 'c1',
    ready: vi.fn(() => {
      order.push('ready');
      return Promise.resolve();
    }),
    commands: {
      authorize: vi.fn((args) => {
        order.push('authorize');
        expect(args.scope).toContain('identify');
        return Promise.resolve({ code: 'CODE' });
      }),
      authenticate: vi.fn((args) => {
        order.push('authenticate');
        return Promise.resolve<AuthenticateResult>({
          access_token: args.access_token,
          user: USER,
          scopes: ['identify'],
          expires: '',
        });
      }),
    },
    ...over,
  };
}

describe('runHandshake', () => {
  it('drives ready → authorize → exchange → authenticate into a session', async () => {
    const order: string[] = [];
    const sdk = fakeSdk(order);
    const exchangeToken = vi.fn((code: string): Promise<ExchangeResult> => {
      order.push('exchange');
      expect(code).toBe('CODE');
      return Promise.resolve({ token: 'leaf-tok', access_token: 'discord-at', expires_in: 3600 });
    });

    const result = await runHandshake({
      clientId: 'app-id',
      sdk,
      exchangeToken,
      now: () => 1000,
    });

    expect(order).toEqual(['ready', 'authorize', 'exchange', 'authenticate']);
    expect(result.user).toEqual(USER);
    expect(result.token).toBe('leaf-tok');
    expect(result.guildId).toBe('g1');
    expect(result.expiresAt).toBe(1000 + 3600 * 1000);
    // The Discord access token (not our session token) authenticates the SDK.
    expect(sdk.commands.authenticate).toHaveBeenCalledWith({ access_token: 'discord-at' });
  });

  it('propagates an authorize failure (e.g. the user declines)', async () => {
    const order: string[] = [];
    const sdk = fakeSdk(order, {
      commands: {
        authorize: vi.fn(() => Promise.reject(new Error('user denied'))),
        authenticate: vi.fn(() => Promise.reject(new Error('unreachable'))),
      },
    });
    const exchangeToken = vi.fn(() => Promise.reject(new Error('unreachable')));

    await expect(runHandshake({ clientId: 'a', sdk, exchangeToken })).rejects.toThrow(
      'user denied',
    );
    expect(exchangeToken).not.toHaveBeenCalled();
  });
});

describe('displayName', () => {
  it('prefers global_name and falls back to username', () => {
    expect(displayName({ id: '1', username: 'u', global_name: 'Display' })).toBe('Display');
    expect(displayName({ id: '1', username: 'u', global_name: null })).toBe('u');
    expect(displayName({ id: '1', username: 'u' })).toBe('u');
  });
});
