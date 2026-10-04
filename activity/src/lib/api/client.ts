// Typed client for the leaf-server REST API. Responses are validated with
// zod (see `schemas.ts`); the client is the only place the app talks to the
// backend. Components mock this module, not `fetch`.
//
// Every failure leaves here as an `ApiError` with a `kind` (see
// `utils/errors.ts`), so views never see a raw `TypeError` or `ZodError`.

import type { z } from 'zod';

import type {
  CreatedSeries,
  CreateSeriesInput,
  Day,
  DaySummary,
  Eligibility,
  MySeries,
  Series,
  SeriesOptions,
  SeriesSettings,
  Stats,
  UpdateSeriesInput,
} from '../types/api';
import { ApiError } from '../utils/errors';
import {
  createdSeriesSchema,
  daySchema,
  daySummaryListSchema,
  eligibilitySchema,
  exchangeSchema,
  launchIntentSchema,
  mySeriesListSchema,
  refreshSchema,
  seriesListSchema,
  seriesOptionsSchema,
  seriesSettingsSchema,
  statsSchema,
} from './schemas';

export { ApiError, type ApiErrorKind } from '../utils/errors';

/** Same-origin by default; the Discord proxy maps it to leaf-server. */
const API_BASE = import.meta.env.VITE_API_BASE ?? '/api';

/** Reads answer from SQLite, or from Discord within the server's own limit. */
const GET_TIMEOUT_MS = 10_000;
/** Writes are not repeated, so they get longer before the client gives up. */
const SEND_TIMEOUT_MS = 20_000;
/**
 * The exchange makes two Discord calls server-side. Giving up before the
 * server does would burn the one-shot OAuth code for nothing.
 */
const EXCHANGE_TIMEOUT_MS = 25_000;
/**
 * Once the headers are in, the server is answering: a slow link gets this
 * much longer to deliver the body (the whole day index is the largest).
 */
const BODY_TIMEOUT_MS = 20_000;
/** Pause before the one automatic retry of a failed GET. */
const RETRY_DELAY_MS = 400;
/** Renew the session once this share of its lifetime has passed. */
const REFRESH_AT = 0.75;
/** After a refresh that failed for a passing reason, wait this long. */
const REFRESH_BACKOFF_MS = 60_000;
/**
 * Never plan a renewal sooner than this. Near the 7-day cap the server may
 * hand out ever shorter tokens; without a floor the renewals would bunch up.
 */
const MIN_REFRESH_WAIT_MS = 30_000;

/** A schema whose parsed output is `T`, whatever its input type. */
type Schema<T> = z.ZodType<T, z.ZodTypeDef, unknown>;

type Method = 'GET' | 'POST' | 'PATCH';

/** Builds an `ApiError` from a failed response's `{error, message?, retryable?}` body. */
async function apiError(res: Response, label: string): Promise<ApiError> {
  let code: string | undefined;
  let detail: string | undefined;
  let retryable: boolean | undefined;
  try {
    const body: unknown = await res.json();
    if (body && typeof body === 'object') {
      const fields = body as { error?: unknown; message?: unknown; retryable?: unknown };
      if (typeof fields.error === 'string') code = fields.error;
      if (typeof fields.message === 'string') detail = fields.message;
      if (typeof fields.retryable === 'boolean') retryable = fields.retryable;
    }
  } catch {
    // Non-JSON body (a proxy error page): the status alone decides the kind.
  }
  return new ApiError(res.status, `${label} → ${res.status}`, code, {
    ...(detail === undefined ? {} : { detail }),
    ...(retryable === undefined ? {} : { retryable }),
  });
}

/**
 * One fetch with a time limit: `timeoutMs` for the response to start, then
 * {@link BODY_TIMEOUT_MS} for its body. Resolves to the parsed JSON of a 2xx;
 * throws `ApiError` otherwise.
 *
 * An `AbortController` and a timer rather than `AbortSignal.timeout`: the
 * static method is missing from the iOS 15 webview, and the timer says for
 * certain that an abort was the time limit.
 */
async function requestJson(
  fetchImpl: typeof fetch,
  label: string,
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<unknown> {
  const controller = new AbortController();
  let timedOut = false;
  const giveUp = (): void => {
    timedOut = true;
    controller.abort();
  };
  let timer = setTimeout(giveUp, timeoutMs);
  const lost = (cause: unknown): ApiError =>
    timedOut
      ? new ApiError(0, `${label} → timed out`, undefined, { kind: 'timeout', cause })
      : new ApiError(0, `${label} → no response`, undefined, { kind: 'network', cause });
  try {
    let res: Response;
    try {
      res = await fetchImpl(url, { ...init, signal: controller.signal });
    } catch (cause) {
      throw lost(cause);
    }
    clearTimeout(timer);
    timer = setTimeout(giveUp, BODY_TIMEOUT_MS);
    if (!res.ok) throw await apiError(res, label);
    try {
      return (await res.json()) as unknown;
    } catch (cause) {
      // Anything but a JSON syntax error means the connection dropped or the
      // time ran out part-way through the body. (Matched by name: the error
      // may come from another realm.)
      const malformed = (cause as { name?: unknown } | null)?.name === 'SyntaxError';
      if (timedOut || !malformed) throw lost(cause);
      // A 2xx that is not JSON: e.g. an older server answering an unknown
      // `/api/*` path with the app's HTML.
      throw new ApiError(res.status, `${label} → unreadable body`, undefined, {
        kind: 'bad_response',
        cause,
      });
    }
  } finally {
    clearTimeout(timer);
  }
}

function parse<T>(schema: Schema<T>, raw: unknown, label: string): T {
  const result = schema.safeParse(raw);
  if (result.success) return result.data;
  throw new ApiError(200, `${label} → unexpected response shape`, undefined, {
    kind: 'bad_response',
    cause: result.error,
  });
}

/**
 * A random id for one logical call, the same across its retries. Not a
 * secret: the server scopes it to the signed-in user. `Math.random` because
 * `crypto.randomUUID` is missing from the iOS 15.0-15.3 webview.
 */
function attemptId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;
}

/**
 * What the SDK handshake needs: exchange an OAuth code for tokens. Rejects
 * with an `ApiError`: `code` is `code_rejected` when Discord refused the code
 * and `discord_unavailable` when leaf could not reach Discord; `kind` is
 * `network` or `timeout` when leaf itself did not answer.
 */
export async function exchangeToken(
  code: string,
  fetchImpl: typeof fetch = fetch,
  timeoutMs: number = EXCHANGE_TIMEOUT_MS,
): Promise<z.infer<typeof exchangeSchema>> {
  const label = 'POST /token';
  const raw = await requestJson(
    fetchImpl,
    label,
    `${API_BASE}/token`,
    {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ code }),
    },
    timeoutMs,
  );
  return parse(exchangeSchema, raw, label);
}

export interface ClientOptions {
  token: string;
  baseUrl?: string;
  fetch?: typeof fetch;
  /**
   * When `token` expires, epoch milliseconds. With it the client renews the
   * token at 75% of its remaining life; without it the token is used as is.
   */
  expiresAt?: number;
  /**
   * Called once, when the server refuses the token and it cannot be renewed.
   * Nothing the client does afterwards will succeed: show "close and reopen".
   */
  onUnauthorized?: () => void;
  /** Pause before the one GET retry, in milliseconds. */
  retryDelayMs?: number;
  now?: () => number;
}

/** Authenticated, guild-scoped API calls for one session. */
export class LeafApi {
  #token: string;
  readonly #base: string;
  readonly #fetch: typeof fetch;
  readonly #now: () => number;
  readonly #retryDelayMs: number;
  readonly #onUnauthorized: (() => void) | undefined;
  /** When to renew the token (epoch ms); `null` when it can't or needn't be. */
  #refreshAt: number | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #refreshing: Promise<boolean> | null = null;
  #expired = false;
  #disposed = false;

  constructor(opts: ClientOptions) {
    this.#token = opts.token;
    this.#base = opts.baseUrl ?? API_BASE;
    // Bind to the global: native fetch throws "Illegal invocation" if called
    // with `this` set to anything but the window (which `this.#fetch(...)`
    // would do). Injected fetches (tests) are used as-is.
    this.#fetch = opts.fetch ?? globalThis.fetch.bind(globalThis);
    this.#now = opts.now ?? Date.now;
    this.#retryDelayMs = opts.retryDelayMs ?? RETRY_DELAY_MS;
    this.#onUnauthorized = opts.onUnauthorized;
    if (opts.expiresAt !== undefined) this.#schedule(opts.expiresAt);
  }

  /** Stops the scheduled renewals for good. Call when the client is replaced. */
  dispose(): void {
    this.#disposed = true;
    this.#stopRenewing();
  }

  /**
   * Renews the token if it is past its refresh point, e.g. when the Activity
   * returns to the foreground after its timers were paused. Never throws.
   */
  async ensureFresh(): Promise<void> {
    if (this.#refreshDue()) await this.#refresh();
  }

  #clearTimer(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
  }

  #stopRenewing(): void {
    this.#refreshAt = null;
    this.#clearTimer();
  }

  #refreshDue(): boolean {
    return !this.#expired && this.#refreshAt !== null && this.#now() >= this.#refreshAt;
  }

  /** Plans the next renewal for a token that expires at `expiresAt`. */
  #schedule(expiresAt: number): void {
    const wait = Math.max(0, expiresAt - this.#now()) * REFRESH_AT;
    this.#wake(Math.max(MIN_REFRESH_WAIT_MS, wait));
  }

  #wake(inMs: number): void {
    if (this.#disposed) return;
    this.#clearTimer();
    this.#refreshAt = this.#now() + inMs;
    // A hidden webview may pause timers; requests and `ensureFresh` also
    // check `#refreshAt`, so a late timer only delays an idle renewal.
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.ensureFresh();
    }, inMs);
  }

  /** Single-flight token renewal. Resolves to whether a new token is in place. */
  #refresh(): Promise<boolean> {
    this.#refreshing ??= this.#renew().finally(() => {
      this.#refreshing = null;
    });
    return this.#refreshing;
  }

  async #renew(): Promise<boolean> {
    const label = 'POST /token/refresh';
    try {
      const raw = await requestJson(
        this.#fetch,
        label,
        `${this.#base}/token/refresh`,
        { method: 'POST', headers: { authorization: `Bearer ${this.#token}` } },
        GET_TIMEOUT_MS,
      );
      const next = parse(refreshSchema, raw, label);
      this.#token = next.token;
      this.#schedule(this.#now() + next.expires_in * 1000);
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.retryable) {
        // A passing failure: the current token still works, try again later.
        this.#wake(REFRESH_BACKOFF_MS);
      } else {
        // The server will not renew this token: it is past the 7-day cap, or
        // this is an older server with no refresh route. The token stays in
        // use until the server refuses it.
        this.#stopRenewing();
      }
      return false;
    }
  }

  #expire(): void {
    if (this.#expired) return;
    this.#expired = true;
    this.#stopRenewing();
    this.#onUnauthorized?.();
  }

  async #once<T>(method: Method, path: string, body: unknown, schema: Schema<T>): Promise<T> {
    const label = `${method} ${path}`;
    const headers: Record<string, string> = { authorization: `Bearer ${this.#token}` };
    const init: RequestInit = { headers };
    if (method !== 'GET') {
      init.method = method;
      headers['content-type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    const timeoutMs = method === 'GET' ? GET_TIMEOUT_MS : SEND_TIMEOUT_MS;
    const raw = await requestJson(this.#fetch, label, `${this.#base}${path}`, init, timeoutMs);
    return parse(schema, raw, label);
  }

  async #request<T>(method: Method, path: string, body: unknown, schema: Schema<T>): Promise<T> {
    if (this.#expired) {
      // The server already refused this session; asking again cannot help.
      throw new ApiError(401, `${method} ${path} → session ended`, 'unauthorized');
    }
    // Renew in the background; this request goes out on the current token,
    // which is still valid between the refresh point and its expiry.
    if (this.#refreshDue()) void this.#refresh();
    let replayed = false;
    let retried = false;
    for (;;) {
      const sentWith = this.#token;
      try {
        return await this.#once(method, path, body, schema);
      } catch (e) {
        if (!(e instanceof ApiError)) throw e;
        if (e.kind === 'unauthorized') {
          // Replay once if a newer token is (or can be put) in place; the
          // request may have crossed a renewal. Otherwise the session is
          // over. A refused write was not applied, so replaying it is safe.
          const renewed =
            !replayed && !this.#expired && (this.#token !== sentWith || (await this.#refresh()));
          if (!renewed) {
            this.#expire();
            throw e;
          }
          replayed = true;
        } else if (method === 'GET' && e.retryable && !retried) {
          // One automatic retry, for reads only: a write may have been
          // applied even though its response was lost.
          retried = true;
          await new Promise((resolve) => setTimeout(resolve, this.#retryDelayMs));
        } else {
          throw e;
        }
      }
    }
  }

  #get<T>(path: string, schema: Schema<T>): Promise<T> {
    return this.#request('GET', path, undefined, schema);
  }

  listSeries(guildId: string): Promise<Series[]> {
    return this.#get(`/guilds/${guildId}/series`, seriesListSchema);
  }

  /**
   * Day tiles for a series. With no range a current server returns the whole
   * index (rows carry `local_date`); an older one returns only its default
   * window of recent days, so callers page with explicit bounds there.
   */
  listDays(
    guildId: string,
    seriesId: number,
    range?: { from?: number; to?: number },
  ): Promise<DaySummary[]> {
    const q = new URLSearchParams();
    if (range?.from !== undefined) q.set('from', String(range.from));
    if (range?.to !== undefined) q.set('to', String(range.to));
    const qs = q.toString();
    const suffix = qs ? `?${qs}` : '';
    return this.#get(`/guilds/${guildId}/series/${seriesId}/days${suffix}`, daySummaryListSchema);
  }

  getDay(guildId: string, seriesId: number, day: number): Promise<Day> {
    return this.#get(`/guilds/${guildId}/series/${seriesId}/days/${day}`, daySchema);
  }

  getStats(guildId: string, seriesId: number): Promise<Stats> {
    return this.#get(`/guilds/${guildId}/series/${seriesId}/stats`, statsSchema);
  }

  /**
   * The series or day a button in chat asked the gallery to open, or `null`.
   * The server forgets the intent as it answers, so call this once per launch
   * or return to the foreground and act on the result.
   *
   * Each call carries its own `attempt` id, and the automatic retry of a
   * failed GET repeats it. A server that remembers the id can give the retry
   * the answer whose first copy was lost on the way back, instead of `null`.
   * A server that ignores it still serves the retry whenever the first
   * request never reached it, which is why the retry stays.
   */
  getLaunchIntent(guildId: string): Promise<z.infer<typeof launchIntentSchema>> {
    return this.#get(`/guilds/${guildId}/launch-intent?attempt=${attemptId()}`, launchIntentSchema);
  }

  // --- creator series management ---

  getEligibility(guildId: string): Promise<Eligibility> {
    return this.#get(`/guilds/${guildId}/series/eligibility`, eligibilitySchema);
  }

  getOptions(guildId: string): Promise<SeriesOptions> {
    return this.#get(`/guilds/${guildId}/series/options`, seriesOptionsSchema);
  }

  listMySeries(guildId: string): Promise<MySeries[]> {
    return this.#get(`/guilds/${guildId}/series/mine`, mySeriesListSchema);
  }

  getSettings(guildId: string, seriesId: number): Promise<SeriesSettings> {
    return this.#get(`/guilds/${guildId}/series/${seriesId}/settings`, seriesSettingsSchema);
  }

  /**
   * Creates a series. A current server answers with the full series (pass it
   * to the gallery store's `adoptSeries`), and answers a repeat of the same
   * create within five minutes with that same series instead of `name_taken`.
   */
  createSeries(guildId: string, input: CreateSeriesInput): Promise<CreatedSeries> {
    return this.#request('POST', `/guilds/${guildId}/series`, input, createdSeriesSchema);
  }

  patchSeries(
    guildId: string,
    seriesId: number,
    patch: UpdateSeriesInput,
  ): Promise<SeriesSettings> {
    return this.#request(
      'PATCH',
      `/guilds/${guildId}/series/${seriesId}`,
      patch,
      seriesSettingsSchema,
    );
  }
}
