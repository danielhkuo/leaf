// What the suites under e2e/app build on: the shared `test` (fixtures.ts)
// plus a leaf server put back to its seed before every test, its control
// routes, and the Discord stand-in the Activity is launched in.
//
// The server is leaf-server's `e2e_server` example (see its doc comment for
// every route and the whole seed): the real API, the real policy and the
// real media proxy, serving the production bundle. Specs under e2e/app
// import `test` from this file.

import type { APIRequestContext, APIResponse, Page } from '@playwright/test';

import {
  DISCORD_PATH,
  launchActivity,
  type Behaviour,
  type DiscordClient,
  type Launch,
} from './discord';
import { APP_CLIENT_ID, APP_NAME } from './env';
import { test as base, expect } from './fixtures';
import type { PageGuard } from './guard';

export { expect };
export type { DiscordClient };

/** The people the server is seeded with (`PERSONAS` in its seed.rs). */
export type PersonaKey = 'creator' | 'viewer' | 'admin' | 'outsider' | 'newcomer' | 'patron';

/** The seeded series, all the creator's (`SERIES` in seed.rs). */
export type SeriesKey = 'long' | 'sprout' | 'role_gated' | 'creator_only' | 'revoked' | 'reminder';

interface Named {
  id: string;
  name: string;
}

interface SeededGuild<Channel extends string, Role extends string> extends Named {
  timezone: string;
  channels: Record<Channel, Named & { kind: number }>;
  roles: Record<Role, Named>;
  creator_role_id: string | null;
  sprout_threshold: number;
}

/** The guild `/setup` was run in: `#daily-sketch` and `#photo-share` take series, `#leaf-log` the log. */
type MainGuild = SeededGuild<
  'general' | 'daily_sketch' | 'photo_share' | 'announcements' | 'voice' | 'leaf_log',
  'artists' | 'patrons' | 'members'
>;

interface SeededPersona {
  id: string;
  username: string;
  display_name: string;
  code: string;
  access_token: string;
}

interface SeededSeries {
  id: number;
  name: string;
  state: string;
  privacy: string;
  channel_id: string | null;
  archived_days: number;
  last_day: number;
}

/** `GET /__e2e/state`: the seed as it was stored. It does not follow later changes. */
export interface Seed {
  origin: string;
  client_id: string;
  timezone: string;
  suggested_now_unix: number;
  guilds: { main: MainGuild; unset: SeededGuild<'general' | 'art', 'regulars'> };
  personas: Record<PersonaKey, SeededPersona>;
  series: Record<SeriesKey, SeededSeries>;
  /** Where the long series keeps each kind of day. */
  long_series: {
    three_images_day: number;
    mp4_day: number;
    webm_day: number;
    missing_media_day: number;
    no_caption_day: number;
    wordy_day: number;
    same_date: string;
    same_date_days: number[];
    empty_month: string;
    gaps: [number, number][];
    last_posted_at: number;
  };
  newcomer_eligible_at: number;
}

/** A call leaf makes to Discord, as `POST /__e2e/discord` names them. */
export type DiscordCall =
  | 'exchange_code'
  | 'current_user'
  | 'guild_member'
  | 'guild_channels'
  | 'guild_roles'
  | 'guild_summary'
  | 'managed_guilds'
  | 'send_message';

/** How the server's stand-in for Discord's API answers leaf. */
export interface DiscordApiMode {
  mode: 'ok' | 'down' | 'slow';
  /** The longest a `slow` call is held; it is let go when the mode is set again. */
  delay_ms?: number;
  /** The calls the mode applies to. Left out: all of them. */
  only?: DiscordCall[];
}

export interface NewDay {
  day?: number;
  media?: 'image' | 'images' | 'mp4' | 'webm' | 'missing' | 'none';
  caption?: string;
  /** Unix seconds. Left out: a day after the series' newest post. */
  posted_at?: number;
}

export interface AddedDay {
  series_id: number;
  day: number;
  posted_at: number;
  local_date: string;
  state: string;
  promoted: boolean;
}

/** `POST /__e2e/session`: a gallery session in the shape of `POST /api/token`'s answer. */
export interface MintedSession {
  token: string;
  access_token: string;
  expires_in: number;
  user_id: string;
  auth_at: number;
  exp: number;
}

export interface SessionRequest {
  /**
   * `fresh` lasts six hours, `expiring` five seconds, `expired` lapsed a
   * minute ago, and `capped` is valid for five minutes but was signed in
   * longer ago than a session may be renewed.
   */
  kind?: 'fresh' | 'expiring' | 'expired' | 'capped';
  /** How long ago the sign-in was, in seconds. */
  age_secs?: number;
  /** How long from now the token stays valid, in seconds. */
  ttl_secs?: number;
}

/** The answer, or an error that says what the server refused and why. */
async function ok(response: APIResponse): Promise<APIResponse> {
  if (response.ok()) return response;
  throw new Error(`${response.url()} answered ${response.status()}: ${await response.text()}`);
}

/**
 * The server under test, from outside the browser: its control routes
 * (`/__e2e/…`, which exist only in the example) and the real API as a
 * signed-in persona. Nothing here goes through the page, so the guard does
 * not see it.
 */
export class LeafServer {
  readonly #request: APIRequestContext;
  readonly seed: Seed;

  constructor(request: APIRequestContext, seed: Seed) {
    this.#request = request;
    this.seed = seed;
  }

  async #post<T>(path: string, data: unknown): Promise<T> {
    const response = await ok(await this.#request.post(path, { data }));
    return (await response.json()) as T;
  }

  /** Changes how Discord's API answers leaf, from the next call on. */
  async discordApi(mode: DiscordApiMode): Promise<void> {
    await this.#post('/__e2e/discord', mode);
  }

  /** Resolves once leaf is waiting on `calls` Discord calls the stand-in is holding (mode `slow`). */
  async discordHolds(calls = 1): Promise<void> {
    await expect
      .poll(async () => {
        const status = (await (await ok(await this.#request.get('/__e2e/discord'))).json()) as {
          held: number;
        };
        return status.held;
      })
      .toBeGreaterThanOrEqual(calls);
  }

  /** The log lines leaf has posted to Discord since the reset, oldest first. */
  async postedMessages(): Promise<{ channel_id: string; content: string }[]> {
    const response = await ok(await this.#request.get('/__e2e/discord/messages'));
    return (await response.json()) as { channel_id: string; content: string }[];
  }

  /** Archives one more day, as the bot would while a gallery is open. */
  addDay(series: SeriesKey | number, day: NewDay = {}): Promise<AddedDay> {
    return this.#post(`/__e2e/series/${series}/days`, day);
  }

  /** Removes an archived day, as when its post is deleted in chat. */
  async removeDay(series: SeriesKey | number, day: number): Promise<void> {
    await ok(await this.#request.delete(`/__e2e/series/${series}/days/${day}`));
  }

  /** Records an "Open gallery" press in chat, for the Activity to collect. */
  async pressOpenGallery(persona: PersonaKey, series: SeriesKey, day?: number): Promise<void> {
    await this.#post('/__e2e/launch-intent', { persona, series, day });
  }

  /** Mints a gallery session without a sign-in, in whatever state a test needs. */
  mintSession(persona: PersonaKey, request: SessionRequest = {}): Promise<MintedSession> {
    return this.#post('/__e2e/session', { persona, ...request });
  }

  /**
   * Mints an admin-panel token for the servers `persona` manages, without a
   * sign-in. Open `/admin#token=<token>` with it. `ttlSecs` below zero gives
   * one that has run out.
   */
  async mintAdminToken(persona: PersonaKey, ttlSecs?: number): Promise<string> {
    const body = { persona, ...(ttlSecs === undefined ? {} : { ttl_secs: ttlSecs }) };
    return (await this.#post<{ token: string }>('/__e2e/admin-session', body)).token;
  }

  /**
   * Signs in through the real `POST /api/token` and returns the headers that
   * carry the session, for calling the API directly.
   */
  async signIn(persona: PersonaKey): Promise<{ authorization: string }> {
    const { code } = this.seed.personas[persona];
    const session = await this.#post<{ token: string }>('/api/token', { code });
    return { authorization: `Bearer ${session.token}` };
  }

  /** `GET /api/guilds/<main guild><path>` as `persona`. The response is returned whatever its status. */
  async get(persona: PersonaKey, path: string): Promise<APIResponse> {
    const headers = await this.signIn(persona);
    return this.#request.get(`/api/guilds/${this.seed.guilds.main.id}${path}`, { headers });
  }
}

/** Where and how the Activity is launched; everything has a default. */
export interface LaunchOptions extends Partial<Behaviour> {
  /** Which Discord client hosts the Activity. Default `mobile`, like the viewport. */
  platform?: Launch['platform'];
  /** A guild id, or `null` for a launch from a DM. Default: the seeded, set-up guild. */
  guildId?: string | null;
  /**
   * A channel id. Default #general, which no series archives from, so the
   * gallery opens on the series list.
   */
  channelId?: string;
  customId?: string;
}

export interface Discord {
  /**
   * Opens the Discord stand-in with the Activity launched in it by
   * `persona`. Launching again is closing the Activity and opening it anew.
   */
  launch(persona: PersonaKey, options?: LaunchOptions): Promise<DiscordClient>;
}

export interface Time {
  /** Moves the page's clock on; timers are not run for it, as after a phone's sleep. */
  pass(ms: number): Promise<void>;
}

/**
 * Tells the guard a refusal is part of the test: the response itself, and
 * the line the browser writes to the console about it.
 */
export function expectRefusal(
  guard: PageGuard,
  status: number,
  method: string,
  path: RegExp,
): void {
  guard.allow(
    new RegExp(`^HTTP ${status}: ${method} https?://[^/]+${path.source}$`),
    new RegExp(
      `^console error: Failed to load resource: the server responded with a status of ${status}\\b`,
    ),
  );
}

/**
 * Every request the page makes to the API from now on, as `METHOD /api/path`
 * (no query), in the order they were sent. Media is left out: a calendar
 * asks for a hundred thumbnails.
 */
export function watchApi(page: Page): string[] {
  const calls: string[] = [];
  page.on('request', (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith('/api/') && !pathname.startsWith('/api/media/')) {
      calls.push(`${request.method()} ${pathname}`);
    }
  });
  return calls;
}

interface Fixtures {
  /** The server, reset to its seed before the test. */
  leaf: LeafServer;
  discord: Discord;
  time: Time;
}

export const test = base.extend<Fixtures>({
  leaf: [
    async ({ request }, use) => {
      // Before the test's page exists, so nothing of the last test is still
      // on its way through the server when the world is replaced.
      const response = await ok(await request.post('/__e2e/reset'));
      await use(new LeafServer(request, (await response.json()) as Seed));
    },
    { auto: true },
  ],

  discord: async ({ page, leaf, guard }, use) => {
    const { seed } = leaf;
    // Chromium's remark about the stand-in itself: its frame is sandboxed as
    // Discord's is, but on the page's own origin, which Discord's never is.
    guard.allow(/^console warning: An iframe which has both allow-scripts and allow-same-origin/);
    const users = Object.fromEntries(
      Object.values(seed.personas).map((persona) => [
        persona.access_token,
        { id: persona.id, username: persona.username, displayName: persona.display_name },
      ]),
    );
    const launched: DiscordClient[] = [];
    await use({
      async launch(persona, options = {}) {
        const client = await launchActivity(page, {
          applicationId: APP_CLIENT_ID,
          applicationName: APP_NAME,
          code: seed.personas[persona].code,
          users,
          platform: options.platform ?? 'mobile',
          guildId: options.guildId === undefined ? seed.guilds.main.id : options.guildId,
          channelId: options.channelId ?? seed.guilds.main.channels.general.id,
          customId: options.customId ?? null,
          authorize: options.authorize ?? 'grant',
          ready: options.ready ?? true,
          openLink: options.openLink ?? 'opened',
        });
        launched.push(client);
        return client;
      },
    });
    // A command the stand-in could not answer is one leaf has started
    // sending: the stand-in needs it, and so does this suite. Only the
    // launch still on screen can be asked.
    const current = launched.at(-1);
    if (current && !page.isClosed() && new URL(page.url()).pathname === DISCORD_PATH) {
      expect(await current.unanswered(), 'commands the stand-in had no answer for').toEqual([]);
    }
  },

  time: async ({ page, now }, use) => {
    let at = (now ?? new Date()).getTime();
    await use({
      async pass(ms) {
        at += ms;
        await page.clock.setFixedTime(at);
      },
    });
  },
});
