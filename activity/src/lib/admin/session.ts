// What the admin page keeps in the browser between loads: the admin token,
// the server to reopen after a sign-in round trip, and settings that were
// typed but not saved when the session ran out. Plus the page's URL state
// (`/admin?guild=<id>`).
//
// Every storage call is guarded: Safari's private mode and locked-down
// browsers throw on access, and the page should then simply not remember.

const TOKEN_KEY = 'leaf:adminToken';
/** The server to open once sign-in comes back; the callback returns to bare `/admin`. */
const RETURN_KEY = 'leaf:adminReturn';
const DRAFT_PREFIX = 'leaf:adminDraft:';

function read(storage: () => Storage, key: string): string | null {
  try {
    return storage().getItem(key);
  } catch {
    return null;
  }
}

/** Stores `value` (`null` removes it). `false` when the browser would not take it. */
function write(storage: () => Storage, key: string, value: string | null): boolean {
  try {
    if (value === null) storage().removeItem(key);
    else storage().setItem(key, value);
    return true;
  } catch {
    // Storage is unavailable; the page works without remembering.
    return false;
  }
}

const local = (): Storage => localStorage;
const tab = (): Storage => sessionStorage;

export function readToken(): string | null {
  return read(local, TOKEN_KEY) || null;
}

export function storeToken(token: string | null): void {
  write(local, TOKEN_KEY, token);
}

/** Digits only, as a Discord id is. Anything else in the URL or in storage is ignored. */
function looksLikeId(id: string): boolean {
  return /^\d{1,25}$/.test(id);
}

/** Remembers which server to open after signing in (`null` forgets it). */
export function rememberGuild(guildId: string | null): void {
  write(tab, RETURN_KEY, guildId);
}

/** The server remembered by {@link rememberGuild}; reading it forgets it. */
export function takeRememberedGuild(): string | null {
  const id = read(tab, RETURN_KEY);
  write(tab, RETURN_KEY, null);
  return id !== null && looksLikeId(id) ? id : null;
}

/**
 * Keeps a server's unsaved settings for after the sign-in round trip. `false`
 * when they could not be kept, so the page does not promise them back.
 */
export function stashDraft(guildId: string, draft: unknown): boolean {
  return write(tab, `${DRAFT_PREFIX}${guildId}`, JSON.stringify(draft));
}

/**
 * What {@link stashDraft} kept, as parsed JSON (the settings form checks its
 * shape); reading it forgets it. `null` when there is none.
 */
export function takeDraft(guildId: string): unknown {
  const key = `${DRAFT_PREFIX}${guildId}`;
  const raw = read(tab, key);
  if (raw === null) return null;
  write(tab, key, null);
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return null;
  }
}

/** Forgets everything kept for this tab's sign-in (on Sign out). */
export function forgetTab(): void {
  try {
    const stale: string[] = [];
    for (let i = 0; i < sessionStorage.length; i += 1) {
      const key = sessionStorage.key(i);
      if (key !== null && (key === RETURN_KEY || key.startsWith(DRAFT_PREFIX))) stale.push(key);
    }
    for (const key of stale) sessionStorage.removeItem(key);
  } catch {
    // Nothing was kept.
  }
}

/** What the OAuth callback handed back in the URL fragment. */
export type Fragment = { kind: 'token'; token: string } | { kind: 'error'; code: string };

/** Reads `#token=<token>` or `#error=<code>`; `null` for any other fragment. */
export function parseFragment(hash: string): Fragment | null {
  const value = (prefix: string): string | null => {
    if (!hash.startsWith(prefix)) return null;
    try {
      return decodeURIComponent(hash.slice(prefix.length));
    } catch {
      return null;
    }
  };
  const token = value('#token=');
  if (token) return { kind: 'token', token };
  const code = value('#error=');
  if (code !== null) return { kind: 'error', code };
  return null;
}

/** The server named by `?guild=`, or `null`. */
export function guildInUrl(search: string): string | null {
  const id = new URLSearchParams(search).get('guild');
  return id !== null && looksLikeId(id) ? id : null;
}

/** The page's URL for a server's panel, or for the server list (`null`). */
export function adminUrl(pathname: string, guildId: string | null): string {
  return guildId === null ? pathname : `${pathname}?guild=${encodeURIComponent(guildId)}`;
}
