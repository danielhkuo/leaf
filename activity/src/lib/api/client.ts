// Typed client for the leaf-server REST API. Responses are validated with
// zod (see `schemas.ts`); the client is the only place the app talks to the
// backend. Components mock this module, not `fetch`.

import type { z } from 'zod';

import type { Day, DaySummary, Series, Stats } from '../types/api';
import {
  daySchema,
  daySummaryListSchema,
  exchangeSchema,
  seriesListSchema,
  statsSchema,
} from './schemas';

/** Same-origin by default; the Discord proxy maps it to leaf-server. */
const API_BASE = import.meta.env.VITE_API_BASE ?? '/api';

/** A non-2xx response from the API. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
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
      throw new ApiError(res.status, `GET ${path} → ${res.status}`);
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
}
