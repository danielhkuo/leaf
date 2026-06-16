// Typed client for the admin API (`/api/admin/*`), Bearer-authed with the
// admin token from the browser OAuth login. Responses are zod-validated.

import type { z } from 'zod';

import {
  adminGuildDetailSchema,
  adminGuildListSchema,
  adminSeriesSchema,
  adminSettingsSchema,
  type AdminGuild,
  type AdminGuildDetail,
  type AdminSeries,
  type SeriesPatch,
  type SettingsPatch,
} from './schemas';

const BASE = '/api/admin';

/** A non-2xx response from the admin API. */
export class AdminApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'AdminApiError';
  }
}

export class AdminApi {
  readonly #token: string;
  readonly #fetch: typeof fetch;

  constructor(token: string, fetchImpl?: typeof fetch) {
    this.#token = token;
    this.#fetch = fetchImpl ?? globalThis.fetch.bind(globalThis);
  }

  async #req<T>(method: string, path: string, schema: z.ZodType<T>, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { authorization: `Bearer ${this.#token}` };
    const init: RequestInit = { method, headers };
    if (body !== undefined) {
      headers['content-type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    const res = await this.#fetch(`${BASE}${path}`, init);
    if (!res.ok) {
      throw new AdminApiError(res.status, `${method} ${path} → ${res.status}`);
    }
    return schema.parse(await res.json());
  }

  listGuilds(): Promise<AdminGuild[]> {
    return this.#req('GET', '/guilds', adminGuildListSchema);
  }

  guild(guildId: string): Promise<AdminGuildDetail> {
    return this.#req('GET', `/guilds/${guildId}`, adminGuildDetailSchema);
  }

  patchSettings(guildId: string, patch: SettingsPatch) {
    return this.#req('PATCH', `/guilds/${guildId}/settings`, adminSettingsSchema, patch);
  }

  patchSeries(guildId: string, seriesId: number, patch: SeriesPatch): Promise<AdminSeries> {
    return this.#req('PATCH', `/guilds/${guildId}/series/${seriesId}`, adminSeriesSchema, patch);
  }
}
