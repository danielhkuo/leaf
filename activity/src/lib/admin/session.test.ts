import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  adminUrl,
  forgetTab,
  guildInUrl,
  parseFragment,
  readToken,
  rememberGuild,
  stashDraft,
  storeToken,
  takeDraft,
  takeRememberedGuild,
} from './session';

/** A working Storage. Newer Node defines its own unusable `localStorage` global. */
function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => void items.delete(key),
    setItem: (key, value) => void items.set(key, String(value)),
  };
}

beforeEach(() => {
  vi.stubGlobal('localStorage', memoryStorage());
  vi.stubGlobal('sessionStorage', memoryStorage());
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe('parseFragment', () => {
  it('reads the token the callback hands back', () => {
    expect(parseFragment('#token=abc.def%3D')).toEqual({ kind: 'token', token: 'abc.def=' });
  });

  it('reads the error code a failed sign-in comes back with', () => {
    expect(parseFragment('#error=no_guilds')).toEqual({ kind: 'error', code: 'no_guilds' });
    expect(parseFragment('#error=')).toEqual({ kind: 'error', code: '' });
  });

  it('leaves any other fragment alone', () => {
    expect(parseFragment('')).toBeNull();
    expect(parseFragment('#settings')).toBeNull();
    expect(parseFragment('#token=')).toBeNull();
    expect(parseFragment('#token=%E0%A4%A')).toBeNull();
  });
});

describe('URL state', () => {
  it('reads the server from ?guild=, and only an id', () => {
    expect(guildInUrl('?guild=900000000000000009')).toBe('900000000000000009');
    expect(guildInUrl('?frame_id=1&guild=42')).toBe('42');
    expect(guildInUrl('')).toBeNull();
    expect(guildInUrl('?guild=')).toBeNull();
    expect(guildInUrl('?guild=../settings')).toBeNull();
  });

  it('builds the URL for a panel and for the server list', () => {
    expect(adminUrl('/admin', '42')).toBe('/admin?guild=42');
    expect(adminUrl('/admin', null)).toBe('/admin');
  });
});

describe('what the page remembers', () => {
  it('keeps the admin token across loads until it is cleared', () => {
    expect(readToken()).toBeNull();
    storeToken('tok');
    expect(readToken()).toBe('tok');
    storeToken(null);
    expect(readToken()).toBeNull();
  });

  it('hands back the server to reopen once, after the sign-in round trip', () => {
    rememberGuild('42');
    expect(takeRememberedGuild()).toBe('42');
    expect(takeRememberedGuild()).toBeNull();
  });

  it('keeps a draft per server and forgets it once read', () => {
    expect(stashDraft('42', { maxSeries: '9' })).toBe(true);
    expect(takeDraft('7')).toBeNull();
    expect(takeDraft('42')).toEqual({ maxSeries: '9' });
    expect(takeDraft('42')).toBeNull();
  });

  it('survives a draft that is not JSON', () => {
    sessionStorage.setItem('leaf:adminDraft:42', '{nope');
    expect(takeDraft('42')).toBeNull();
  });

  it('forgets this tab’s leftovers on sign-out, and nothing else', () => {
    rememberGuild('42');
    stashDraft('42', { maxSeries: '9' });
    sessionStorage.setItem('other', 'kept');

    forgetTab();

    expect(takeRememberedGuild()).toBeNull();
    expect(takeDraft('42')).toBeNull();
    expect(sessionStorage.getItem('other')).toBe('kept');
  });

  it('works without storage: nothing is remembered, nothing throws', () => {
    const locked = (): never => {
      throw new DOMException('denied', 'SecurityError');
    };
    const blocked = { getItem: locked, setItem: locked, removeItem: locked, key: locked };
    vi.stubGlobal('localStorage', blocked);
    vi.stubGlobal('sessionStorage', blocked);

    storeToken('tok');
    rememberGuild('42');
    // The caller is told, so it does not promise the draft back.
    expect(stashDraft('42', { maxSeries: '9' })).toBe(false);
    forgetTab();

    expect(readToken()).toBeNull();
    expect(takeRememberedGuild()).toBeNull();
    expect(takeDraft('42')).toBeNull();
  });
});
