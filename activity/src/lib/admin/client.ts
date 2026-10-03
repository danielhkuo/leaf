// Typed client for the admin API (`/api/admin/*`), Bearer-authed with the
// admin token from the browser OAuth login. Responses are zod-validated.
//
// Every failure leaves here as an `AdminApiError` with a `kind` and, when the
// server sent one, its machine `code` and its own sentence. The panel turns
// that into copy in one place (`copy.ts`), never by printing `message`.

import type { z } from 'zod';

import { ApiError } from '../utils/errors';
import {
  adminGuildDetailSchema,
  adminGuildListSchema,
  adminOptionsSchema,
  adminSeriesSchema,
  adminSettingsAnswerSchema,
  type AdminGuild,
  type AdminGuildDetail,
  type AdminOptions,
  type AdminSeries,
  type AdminSettings,
  type SeriesPatch,
  type SettingsPatch,
} from './schemas';

const BASE = '/api/admin';

/** Reads may wait on Discord server-side (names, role and channel lists). */
const GET_TIMEOUT_MS = 15_000;
/** Writes are never repeated, so they get longer before the page gives up. */
const SEND_TIMEOUT_MS = 20_000;

/** A schema whose parsed output is `T`, whatever its input type. */
type Schema<T> = z.ZodType<T, z.ZodTypeDef, unknown>;

/**
 * A failed admin API call. `message` is a developer label
 * (`PATCH /guilds/1/settings → 422`) for logs; show `adminErrorMessage(e)`
 * instead. `status` is 0 when no response arrived, `code` is the body's
 * `error`, `detail` its `message`.
 */
export class AdminApiError extends ApiError {
  override name = 'AdminApiError';
}

/** Whether `e` is the server refusing the admin token (expired or invalid). */
export function isUnauthorized(e: unknown): boolean {
  return e instanceof AdminApiError && e.kind === 'unauthorized';
}

/** Builds the error for a non-2xx response from its `{error, message?, retryable?}` body. */
async function refusal(res: Response, label: string): Promise<AdminApiError> {
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
    // Not JSON (a proxy's error page): the status alone decides the kind.
  }
  return new AdminApiError(res.status, `${label} → ${res.status}`, code, {
    ...(detail === undefined ? {} : { detail }),
    ...(retryable === undefined ? {} : { retryable }),
  });
}

export class AdminApi {
  readonly #token: string;
  readonly #fetch: typeof fetch;

  constructor(token: string, fetchImpl?: typeof fetch) {
    this.#token = token;
    this.#fetch = fetchImpl ?? globalThis.fetch.bind(globalThis);
  }

  async #req<T>(method: string, path: string, schema: Schema<T>, body?: unknown): Promise<T> {
    const label = `${method} ${path}`;
    const headers: Record<string, string> = { authorization: `Bearer ${this.#token}` };
    const init: RequestInit = { method, headers };
    if (body !== undefined) {
      headers['content-type'] = 'application/json';
      init.body = JSON.stringify(body);
    }

    // An AbortController and a timer rather than `AbortSignal.timeout`: the
    // static method is missing from iOS 15 Safari, and the flag says for
    // certain that an abort was the time limit.
    const controller = new AbortController();
    let timedOut = false;
    const timer = setTimeout(
      () => {
        timedOut = true;
        controller.abort();
      },
      method === 'GET' ? GET_TIMEOUT_MS : SEND_TIMEOUT_MS,
    );
    const lost = (cause: unknown): AdminApiError =>
      timedOut
        ? new AdminApiError(0, `${label} → timed out`, undefined, { kind: 'timeout', cause })
        : new AdminApiError(0, `${label} → no response`, undefined, { kind: 'network', cause });

    try {
      let res: Response;
      try {
        res = await this.#fetch(`${BASE}${path}`, { ...init, signal: controller.signal });
      } catch (cause) {
        throw lost(cause);
      }
      if (!res.ok) throw await refusal(res, label);

      let raw: unknown;
      try {
        raw = await res.json();
      } catch (cause) {
        // Anything but a JSON syntax error means the connection dropped or
        // the time ran out part-way through the body.
        const malformed = (cause as { name?: unknown } | null)?.name === 'SyntaxError';
        if (timedOut || !malformed) throw lost(cause);
        throw new AdminApiError(res.status, `${label} → unreadable body`, undefined, {
          kind: 'bad_response',
          cause,
        });
      }
      const parsed = schema.safeParse(raw);
      if (parsed.success) return parsed.data;
      throw new AdminApiError(res.status, `${label} → unexpected response shape`, undefined, {
        kind: 'bad_response',
        cause: parsed.error,
      });
    } finally {
      clearTimeout(timer);
    }
  }

  listGuilds(): Promise<AdminGuild[]> {
    return this.#req('GET', '/guilds', adminGuildListSchema);
  }

  guild(guildId: string): Promise<AdminGuildDetail> {
    return this.#req('GET', `/guilds/${encodeURIComponent(guildId)}`, adminGuildDetailSchema);
  }

  /**
   * The roles and channels the pickers offer. An older server has no such
   * route and answers 404: the panel then falls back to text boxes.
   */
  options(guildId: string): Promise<AdminOptions> {
    return this.#req('GET', `/guilds/${encodeURIComponent(guildId)}/options`, adminOptionsSchema);
  }

  patchSettings(guildId: string, patch: SettingsPatch): Promise<AdminSettings> {
    return this.#req(
      'PATCH',
      `/guilds/${encodeURIComponent(guildId)}/settings`,
      adminSettingsAnswerSchema,
      patch,
    );
  }

  patchSeries(guildId: string, seriesId: number, patch: SeriesPatch): Promise<AdminSeries> {
    return this.#req(
      'PATCH',
      `/guilds/${encodeURIComponent(guildId)}/series/${seriesId}`,
      adminSeriesSchema,
      patch,
    );
  }
}
