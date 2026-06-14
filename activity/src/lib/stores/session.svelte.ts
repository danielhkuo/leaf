// Boot/session state as a runes module (Svelte 5 universal reactivity).
// Components read `session.value`; `bootSession()` drives the handshake.

import type { Session } from '../sdk/handshake';

export type SessionState =
  | { status: 'loading' }
  | { status: 'authed'; session: Session }
  | { status: 'error'; error: string };

export const session = $state<{ value: SessionState }>({
  value: { status: 'loading' },
});

/** Runs the SDK handshake, moving the store through loading → authed/error. */
export async function bootSession(): Promise<void> {
  session.value = { status: 'loading' };
  try {
    // Lazy-load the SDK + API client so the loading shell paints before the
    // heavy handshake code (Discord SDK, zod) is fetched. Keeps the initial
    // chunk small — see the bundle budget.
    const { boot } = await import('../sdk/discord');
    const s = await boot();
    session.value = { status: 'authed', session: s };
  } catch (e) {
    session.value = {
      status: 'error',
      error: e instanceof Error ? e.message : String(e),
    };
  }
}
