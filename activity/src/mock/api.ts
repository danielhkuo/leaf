// A stand-in for leaf-server's gallery API (`/api/guilds/...`), answering
// `fetch` from the fixtures. With it the mock screens mount the real views
// over the real store and API client, instead of hand-made copies that fall
// behind. Every answer goes through the client's zod schemas, so a fixture
// that drifts from the API's shape fails where it is read.
//
// It keeps what was done to it (a series created, settings saved) until the
// page reloads, so those flows can be walked through to the end.

import type { CreateSeriesInput, Eligibility, Series, UpdateSeriesInput } from '../lib/types/api';
import {
  dayOf,
  eligibilityOk,
  indexOf,
  mineOf,
  options,
  ownerSettings,
  series as allSeries,
  settingsOf,
  statsOf,
  TIMEZONE,
  USER_ID,
  worstCase,
  type OwnerSettings,
} from './fixtures';

/** What the server holds for one mock screen. Everything defaults to the fixtures. */
export interface Scenario {
  /** The series this viewer can see. */
  series?: Series[] | undefined;
  /** Whether the viewer may start a series. */
  eligibility?: Eligibility | undefined;
  /** Requests for a day never answer: the viewer stays on its loading state. */
  holdDays?: boolean | undefined;
  /**
   * The series list does not load. `expired`: the server refuses the session
   * and will not renew it. `unavailable`: it answers 503, as the server does
   * when it cannot reach Discord (its only 503).
   */
  listFails?: 'expired' | 'unavailable' | undefined;
  /**
   * Every name, description and caption in an answer is as long as it can
   * be. For checking layouts: a name saved in this mode reads back stretched
   * too, so the settings form reports it as not saved.
   */
  longText?: boolean | undefined;
}

interface Reply {
  status: number;
  body: unknown;
}

/** A request this scenario leaves unanswered. */
const HOLD = Symbol('hold');

const ok = (body: unknown): Reply => ({ status: 200, body });
const refuse = (status: number, error: string): Reply => ({ status, body: { error } });
const NOT_FOUND = refuse(404, 'not_found');

/**
 * A path or method the mock has no answer for. The real API may well have
 * one: a call added to the client needs a branch in `answer()`. Logged,
 * because all the screen shows is the view's "not available" state.
 */
function unanswered(method: string, path: string): Reply {
  console.error(`mock API: nothing answers ${method} ${path}`);
  return NOT_FOUND;
}

interface State {
  series: Series[];
  owner: Map<number, OwnerSettings>;
  nextId: number;
}

function nameTaken(state: State, name: string, except?: number): boolean {
  const wanted = name.trim().toLowerCase();
  return state.series.some((s) => s.id !== except && s.name.toLowerCase() === wanted);
}

function create(state: State, input: CreateSeriesInput): Reply {
  if (nameTaken(state, input.name)) return refuse(409, 'name_taken');
  const created: Series = {
    id: state.nextId,
    name: input.name,
    description: input.description ?? '',
    creator_id: USER_ID,
    cadence: input.cadence,
    emoji: '🍃',
    start_day: input.start_day ?? 1,
    max_day: null,
    // A new series starts as a sprout while the server has the stage on.
    state: 'sprout',
    privacy: input.privacy,
    is_owner: true,
    channel_ids: [input.channel_id],
    timezone: TIMEZONE,
    total_days: 0,
    sprout: { archived: 0, threshold: options.sprout_threshold },
  };
  state.nextId += 1;
  state.series.push(created);
  if (input.privacy_role_id) {
    state.owner.set(created.id, { privacy_role_id: input.privacy_role_id });
  }
  return { status: 201, body: created };
}

function update(state: State, found: Series, patch: UpdateSeriesInput): Reply {
  if (found.state === 'revoked') return refuse(403, 'revoked');
  if (patch.name !== undefined && nameTaken(state, patch.name, found.id)) {
    return refuse(409, 'name_taken');
  }
  const {
    name,
    description,
    emoji,
    cadence,
    privacy,
    start_day,
    channel_id,
    privacy_role_id,
    reminder_enabled,
    reminder_time,
    reminder_timezone,
    reminder_dm,
  } = patch;
  if (name !== undefined) found.name = name;
  if (description !== undefined) found.description = description;
  if (emoji !== undefined) found.emoji = emoji;
  if (cadence !== undefined) found.cadence = cadence;
  if (privacy !== undefined) found.privacy = privacy;
  if (start_day !== undefined) found.start_day = start_day;
  if (channel_id !== undefined) found.channel_ids = [channel_id];

  const own: OwnerSettings = { ...state.owner.get(found.id) };
  // As the server does: the recorded failure is forgotten only when the
  // route really changes. Sending the stored value again keeps it.
  const before = settingsOf(found, own);
  const rerouted =
    (reminder_enabled !== undefined && reminder_enabled !== before.reminder_enabled) ||
    (reminder_dm !== undefined && reminder_dm !== before.reminder_dm) ||
    (!(reminder_dm ?? before.reminder_dm) &&
      channel_id !== undefined &&
      channel_id !== before.channel_id);
  if (privacy_role_id !== undefined) own.privacy_role_id = privacy_role_id;
  if (reminder_enabled !== undefined) own.reminder_enabled = reminder_enabled;
  if (reminder_time !== undefined) own.reminder_time = reminder_time;
  // An empty zone clears the override, back to the server's timezone.
  if (reminder_timezone !== undefined) own.reminder_timezone = reminder_timezone || null;
  if (reminder_dm !== undefined) own.reminder_dm = reminder_dm;
  if (rerouted) {
    delete own.reminder_error;
    delete own.reminder_error_at;
  }
  state.owner.set(found.id, own);
  return ok(settingsOf(found, own));
}

function answer(
  scenario: Scenario,
  state: State,
  method: string,
  path: string,
  body: unknown,
): Reply | typeof HOLD {
  const expired = scenario.listFails === 'expired';
  if (path === '/token/refresh') {
    if (method !== 'POST') return unanswered(method, path);
    return expired ? refuse(401, 'unauthorized') : ok({ token: 'mock-token', expires_in: 21_600 });
  }

  // /guilds/{gid}/launch-intent, /guilds/{gid}/series[/...]
  const [, root, , area, ...rest] = path.split('/');
  if (root !== 'guilds') return unanswered(method, path);
  // Nothing in chat asked the mock to open anywhere.
  if (area === 'launch-intent') return ok(null);
  if (area !== 'series') return unanswered(method, path);
  const [first, second, third] = rest;

  if (first === undefined) {
    if (method === 'POST') return create(state, body as CreateSeriesInput);
    if (expired) return refuse(401, 'unauthorized');
    if (scenario.listFails === 'unavailable') {
      return {
        status: 503,
        body: {
          error: 'discord_unavailable',
          message: "leaf can't reach Discord right now. Try again in a moment.",
          retryable: true,
        },
      };
    }
    return ok(state.series);
  }
  if (first === 'eligibility') return ok(scenario.eligibility ?? eligibilityOk);
  if (first === 'options') return ok(options);
  if (first === 'mine') {
    return ok(state.series.filter((s) => s.is_owner).map((s) => mineOf(s, state.owner.get(s.id))));
  }

  const found = state.series.find((s) => s.id === Number(first));
  if (second === undefined) {
    if (method !== 'PATCH') return unanswered(method, path);
    // As the server does: a series is not there for anyone but its owner.
    return found?.is_owner ? update(state, found, body as UpdateSeriesInput) : NOT_FOUND;
  }
  if (second !== 'settings' && second !== 'stats' && second !== 'days') {
    return unanswered(method, path);
  }
  if (!found) return NOT_FOUND;
  if (second === 'settings') {
    return found.is_owner ? ok(settingsOf(found, state.owner.get(found.id))) : NOT_FOUND;
  }
  // A revoked series is listed for its owner, but its days are not served.
  if (found.state === 'revoked') return NOT_FOUND;
  if (second === 'stats') return ok(statsOf(found));
  if (third === undefined) return ok(indexOf(found));
  if (scenario.holdDays) return HOLD;
  const day = dayOf(found, Number(third));
  return day ? ok(day) : NOT_FOUND;
}

/** The `/api`-relative path of a request, or `null` for anything else. */
function apiPath(input: RequestInfo | URL): string | null {
  const href = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
  const { pathname } = new URL(href, 'http://mock.invalid');
  // The admin API is not mocked here: GuildPanel takes a stand-in client.
  if (!pathname.startsWith('/api/') || pathname.startsWith('/api/admin')) return null;
  return pathname.slice('/api'.length);
}

/**
 * A `fetch` that answers the gallery API for `scenario` and rejects any
 * other request, as a network that is not there would. A gallery API path
 * it has no answer for gets a 404 and a line in the console.
 */
export function createMockApi(scenario: Scenario = {}): typeof fetch {
  const state: State = {
    series: structuredClone(scenario.series ?? allSeries),
    owner: new Map(Object.entries(ownerSettings).map(([id, own]) => [Number(id), { ...own }])),
    nextId: 100,
  };
  return (input, init) => {
    const path = apiPath(input);
    if (path === null) return Promise.reject(new TypeError('mock API: not a gallery API request'));
    const sent = typeof init?.body === 'string' ? (JSON.parse(init.body) as unknown) : undefined;
    const reply = answer(scenario, state, (init?.method ?? 'GET').toUpperCase(), path, sent);
    // Left pending whatever the caller's time limit says: that is the point.
    if (reply === HOLD) return new Promise<Response>(() => undefined);
    const answered = scenario.longText ? worstCase(reply.body) : reply.body;
    return Promise.resolve(
      new Response(JSON.stringify(answered), {
        status: reply.status,
        headers: { 'content-type': 'application/json' },
      }),
    );
  };
}

/**
 * Routes this page's gallery API requests to a mock for `scenario`; every
 * other request still reaches the network. Install it before the gallery
 * store builds its client, which keeps the `fetch` it finds. Returns the
 * function that puts the real `fetch` back.
 */
export function installMockApi(scenario: Scenario = {}): () => void {
  const real = window.fetch;
  const mock = createMockApi(scenario);
  window.fetch = (input, init) =>
    apiPath(input) === null ? real.call(window, input, init) : mock(input, init);
  return () => {
    window.fetch = real;
  };
}
