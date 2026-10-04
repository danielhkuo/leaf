// The Activity's error model and the copy shown for it. Kept free of zod and
// of the API client so views in the initial chunk can describe a failure
// without pulling either in (`api/client.ts` re-exports `ApiError`).

import { seriesErrorMessage } from './labels';

/**
 * Why a request failed, coarse enough to pick copy and decide on a retry:
 *
 * - `network`: the request never got an answer (offline, DNS, proxy).
 * - `timeout`: no answer within the client's time limit.
 * - `unauthorized`: 401, the session token is missing, expired or invalid.
 * - `forbidden`: 403.
 * - `not_found`: 404, missing or hidden from this viewer.
 * - `server`: 5xx, 408 or 429; worth trying again.
 * - `bad_response`: a 2xx whose body is not what this build expects.
 * - `rejected`: any other 4xx (validation, policy, conflict); `code` says which.
 */
export type ApiErrorKind =
  | 'network'
  | 'timeout'
  | 'unauthorized'
  | 'forbidden'
  | 'not_found'
  | 'server'
  | 'bad_response'
  | 'rejected';

export interface ApiErrorInit {
  /** Overrides the kind derived from `status`. */
  kind?: ApiErrorKind;
  /** The server's own sentence for this failure (the body's `message`). */
  detail?: string;
  /** The server's `retryable` flag; defaults from the kind. */
  retryable?: boolean;
  /** The underlying error (a fetch `TypeError`, a `ZodError`). */
  cause?: unknown;
}

function kindForStatus(status: number): ApiErrorKind {
  if (status === 401) return 'unauthorized';
  if (status === 403) return 'forbidden';
  if (status === 404) return 'not_found';
  if (status >= 500 || status === 408 || status === 429) return 'server';
  return 'rejected';
}

/**
 * A failed API call. `message` is a developer label (`GET /path → 500`) for
 * logs and is never shown to people; use {@link describeError} for that.
 * `status` is 0 when no response arrived. `code` is the server's stable
 * machine code (`name_taken`, `discord_unavailable`) when the body has one.
 */
export class ApiError extends Error {
  readonly kind: ApiErrorKind;
  readonly detail: string | undefined;
  /** Whether repeating the same request may succeed. */
  readonly retryable: boolean;

  constructor(
    readonly status: number,
    message: string,
    readonly code?: string,
    init: ApiErrorInit = {},
  ) {
    super(message, init.cause === undefined ? undefined : { cause: init.cause });
    this.name = 'ApiError';
    this.kind = init.kind ?? kindForStatus(status);
    this.detail = init.detail;
    this.retryable =
      init.retryable ??
      (this.kind === 'network' || this.kind === 'timeout' || this.kind === 'server');
  }
}

const KIND_COPY: Record<ApiErrorKind, string> = {
  network: 'Can’t reach leaf. Check your connection and try again.',
  timeout: 'leaf is taking too long to answer. Try again in a moment.',
  unauthorized: 'Your session has ended. Close leaf and open it again.',
  forbidden: 'You don’t have access to this. If you think you should, ask a server admin.',
  not_found: 'This isn’t available. It may have been removed, or you may not have access to it.',
  server: 'Something went wrong on leaf’s side. Try again in a moment.',
  bad_response: 'leaf sent something this version can’t read. Close leaf and open it again.',
  rejected: 'leaf couldn’t do that. Check what you entered and try again.',
};

const UNKNOWN_COPY = 'Something went wrong. Try again.';

/** The failure's kind, or `unknown` for anything that is not an {@link ApiError}. */
export function errorKind(e: unknown): ApiErrorKind | 'unknown' {
  return e instanceof ApiError ? e.kind : 'unknown';
}

/**
 * Whether a Retry button makes sense. Unknown errors count as retryable:
 * trying again costs nothing and there is no better advice to give.
 */
export function isRetryable(e: unknown): boolean {
  return e instanceof ApiError ? e.retryable : true;
}

/**
 * One sentence a person can act on, for any thrown value. Pass it as the
 * `message` of `ErrorState`; the view supplies its own title. A known server
 * code wins, then the server's sentence for a refusal, then copy for the kind.
 */
export function describeError(e: unknown): string {
  if (!(e instanceof ApiError)) return UNKNOWN_COPY;
  const fallback =
    (e.kind === 'rejected' || e.kind === 'forbidden') && e.detail ? e.detail : KIND_COPY[e.kind];
  return seriesErrorMessage(e.code, fallback);
}
