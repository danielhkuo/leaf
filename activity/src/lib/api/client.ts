// Typed client for the leaf-server REST API. Responses are validated with
// zod (see `schemas.ts`); the client is the only place the app talks to the
// backend. Components mock this module, not `fetch`.

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
import {
  createdSeriesSchema,
  daySchema,
  daySummaryListSchema,
  eligibilitySchema,
  exchangeSchema,
  mySeriesListSchema,
  seriesListSchema,
  seriesOptionsSchema,
  seriesSettingsSchema,
  statsSchema,
} from './schemas';

/** Same-origin by default; the Discord proxy maps it to leaf-server. */
const API_BASE = import.meta.env.VITE_API_BASE ?? '/api';

/**
 * A non-2xx response from the API. `code` is the server's stable machine
 * code (e.g. `name_taken`, `invalid_channel`) when the body carries one, so
 * the UI can show a specific message instead of a generic failure.
 */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
    readonly code?: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/** Builds an `ApiError` from a failed response, reading its `error` code. */
async function apiError(res: Response, label: string): Promise<ApiError> {
  let code: string | undefined;
  try {
    const body: unknown = await res.clone().json();
    if (body && typeof body === 'object' && 'error' in body) {
      const value = (body as { error: unknown }).error;
      if (typeof value === 'string') code = value;
    }
  } catch {
    // Non-JSON body — leave `code` undefined.
  }
  return new ApiError(res.status, `${label} → ${res.status}`, code);
}

/** What the SDK handshake needs: exchange an OAuth code for tokens. */
export async function exchangeToken(
  code: string,
  fetchImpl: typeof fetch = fetch,
): Promise<z.infer<typeof exchangeSchema>> {
  const res = await fetchImpl(`${API_BASE}/token`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ code }),
  });
  if (!res.ok) {
    throw new ApiError(res.status, `token exchange failed (${res.status})`);
  }
  return exchangeSchema.parse(await res.json());
}

export interface ClientOptions {
  token: string;
  baseUrl?: string;
  fetch?: typeof fetch;
}

/** Authenticated, guild-scoped reads. */
export class LeafApi {
  readonly #token: string;
  readonly #base: string;
  readonly #fetch: typeof fetch;

  constructor(opts: ClientOptions) {
    this.#token = opts.token;
    this.#base = opts.baseUrl ?? API_BASE;
    // Bind to the global: native fetch throws "Illegal invocation" if called
    // with `this` set to anything but the window (which `this.#fetch(...)`
    // would do). Injected fetches (tests) are used as-is.
    this.#fetch = opts.fetch ?? globalThis.fetch.bind(globalThis);
  }

  async #get<T>(path: string, schema: z.ZodType<T>): Promise<T> {
    const res = await this.#fetch(`${this.#base}${path}`, {
      headers: { authorization: `Bearer ${this.#token}` },
    });
    if (!res.ok) {
      throw await apiError(res, `GET ${path}`);
    }
    return schema.parse(await res.json());
  }

  async #send<T>(
    method: 'POST' | 'PATCH',
    path: string,
    body: unknown,
    schema: z.ZodType<T>,
  ): Promise<T> {
    const res = await this.#fetch(`${this.#base}${path}`, {
      method,
      headers: { authorization: `Bearer ${this.#token}`, 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      throw await apiError(res, `${method} ${path}`);
    }
    return schema.parse(await res.json());
  }

  listSeries(guildId: string): Promise<Series[]> {
    return this.#get(`/guilds/${guildId}/series`, seriesListSchema);
  }

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

  createSeries(guildId: string, input: CreateSeriesInput): Promise<CreatedSeries> {
    return this.#send('POST', `/guilds/${guildId}/series`, input, createdSeriesSchema);
  }

  patchSeries(
    guildId: string,
    seriesId: number,
    patch: UpdateSeriesInput,
  ): Promise<SeriesSettings> {
    return this.#send('PATCH', `/guilds/${guildId}/series/${seriesId}`, patch, seriesSettingsSchema);
  }
}
