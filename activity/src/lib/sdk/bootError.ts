// Why the Activity could not start, and what to tell the person about it.
// Kept free of the SDK and the API client so the boot screen (which is in
// the initial chunk) can show a failure without loading either.

/**
 * Coarse enough to pick copy and decide which buttons help:
 *
 * - `no_guild`: launched from a DM or group DM; galleries belong to servers.
 * - `not_in_discord`: the SDK could not start (launch parameters missing).
 * - `misconfigured`: no application id, or Discord refused it or this origin.
 * - `ready_timeout`: Discord never answered the handshake.
 * - `consent_declined`: `authorize` was refused, most often the person
 *   dismissing the permission sheet.
 * - `network`: leaf's server did not answer the token exchange.
 * - `discord_unavailable`: leaf's server could not reach Discord.
 * - `sign_in_rejected`: Discord refused a fresh sign-in code; the install's
 *   client id and secret most likely disagree. Repeating it cannot help.
 * - `server`: the exchange failed on leaf's side for another reason.
 * - `load_failed`: the SDK chunk itself did not download.
 * - `unknown`: anything else.
 */
export type BootErrorKind =
  | 'no_guild'
  | 'not_in_discord'
  | 'misconfigured'
  | 'ready_timeout'
  | 'consent_declined'
  | 'network'
  | 'discord_unavailable'
  | 'sign_in_rejected'
  | 'server'
  | 'load_failed'
  | 'unknown';

/**
 * A failed boot. `message` equals `detail`: technical text for logs and the
 * "Details" disclosure, never the headline. Use {@link bootErrorCopy} for
 * what people read.
 */
export class BootError extends Error {
  constructor(
    readonly kind: BootErrorKind,
    readonly detail: string,
    cause?: unknown,
  ) {
    super(detail, cause === undefined ? undefined : { cause });
    this.name = 'BootError';
  }
}

/**
 * The code and text of anything thrown. The SDK rejects a command with the
 * plain `{code, message}` payload Discord sent, not an `Error`, so
 * `String(e)` on it reads "[object Object]".
 */
export function describeThrown(e: unknown): { code: number | null; text: string } {
  if (typeof e === 'string') return { code: null, text: e };
  if (e && typeof e === 'object') {
    const { code, message } = e as { code?: unknown; message?: unknown };
    return {
      code: typeof code === 'number' ? code : null,
      text: typeof message === 'string' && message ? message : 'no message',
    };
  }
  return { code: null, text: 'no message' };
}

/** Any thrown value as a {@link BootError}; `unknown` unless it already is one. */
export function toBootError(e: unknown, context = 'boot'): BootError {
  if (e instanceof BootError) return e;
  const { code, text } = describeThrown(e);
  return new BootError('unknown', `${context}: ${code === null ? '' : `${code} `}${text}`, e);
}

export interface BootErrorCopy {
  title: string;
  message: string;
  /**
   * Label for the button that runs the boot again (it resumes at the step
   * that failed), or `null` when repeating it cannot help.
   */
  retry: string | null;
  /** Whether to offer Close. Needs the SDK, so not when it never loaded. */
  close: boolean;
  /** `neutral` is a state to explain, not a failure to announce. */
  tone: 'neutral' | 'error';
  /**
   * Whether to offer the technical detail behind a collapsed disclosure.
   * Not on the two screens about where leaf was opened, whose detail adds
   * nothing; yes on a declined consent, which may be a broken setup instead.
   */
  details: boolean;
}

const COPY: Record<BootErrorKind, BootErrorCopy> = {
  no_guild: {
    title: 'leaf opens from a server',
    message:
      'Galleries belong to servers, so there is nothing to show in a DM. Go to a channel in a server that uses leaf and open leaf from there.',
    retry: null,
    close: true,
    tone: 'neutral',
    details: false,
  },
  not_in_discord: {
    title: 'leaf opens inside Discord',
    message: 'Open leaf from the Apps menu in a server channel.',
    retry: null,
    close: false,
    tone: 'neutral',
    details: false,
  },
  misconfigured: {
    title: 'leaf isn’t set up correctly',
    message:
      'This copy of leaf doesn’t match its Discord app. Tell a server admin; there is nothing to fix on your side.',
    retry: null,
    close: true,
    tone: 'error',
    details: true,
  },
  ready_timeout: {
    title: 'Discord didn’t respond',
    message: 'Close leaf and open it again.',
    retry: null,
    close: true,
    tone: 'error',
    details: true,
  },
  consent_declined: {
    title: 'leaf needs your OK',
    message:
      'Discord asks once whether leaf can see your username and avatar. leaf never posts as you. If Discord doesn’t ask again, close leaf and open it again.',
    retry: 'Grant access',
    close: true,
    tone: 'neutral',
    details: true,
  },
  network: {
    title: 'Can’t reach leaf',
    message: 'Check your connection, then try again.',
    retry: 'Try again',
    close: true,
    tone: 'error',
    details: true,
  },
  discord_unavailable: {
    title: 'Discord isn’t answering',
    message: 'leaf can’t reach Discord right now. Try again in a moment.',
    retry: 'Try again',
    close: true,
    tone: 'error',
    details: true,
  },
  sign_in_rejected: {
    title: 'Couldn’t sign you in',
    message:
      'Discord didn’t accept leaf’s sign-in. That usually means leaf’s Discord app settings need fixing. Tell a server admin.',
    retry: null,
    close: true,
    tone: 'error',
    details: true,
  },
  server: {
    title: 'Couldn’t sign you in',
    message:
      'Something went wrong on leaf’s side. Try again in a moment. If it keeps happening, tell a server admin.',
    retry: 'Try again',
    close: true,
    tone: 'error',
    details: true,
  },
  load_failed: {
    title: 'leaf didn’t finish loading',
    message:
      'Check your connection, then try again. If that doesn’t help, close leaf and open it again.',
    retry: 'Try again',
    close: false,
    tone: 'error',
    details: true,
  },
  unknown: {
    title: 'Couldn’t start leaf',
    message: 'Try again. If it keeps happening, close leaf and open it again.',
    retry: 'Try again',
    close: true,
    tone: 'error',
    details: true,
  },
};

/** What the boot screen shows for a failure of this kind. */
export function bootErrorCopy(kind: BootErrorKind): BootErrorCopy {
  return COPY[kind];
}
