// The servers Playwright starts before the suites, one per kind of project.
// Each project names its own origin as `use.baseURL`; a server is only
// reused when it is already running outside CI.

import type { PlaywrightTestConfig } from '@playwright/test';

import { APP_CLIENT_ID, APP_ORIGINS, CI, MOCK_ORIGIN, type AppEngine } from './env';

type WebServer = Exclude<NonNullable<PlaywrightTestConfig['webServer']>, unknown[]>;

/** Vite, serving the mock screens (`/mock.html`) with fixture data. */
export const mockServer: WebServer = {
  command: 'npx vite --config e2e/support/vite.mock.config.ts',
  url: `${MOCK_ORIGIN}/mock.html`,
  reuseExistingServer: !CI,
  timeout: 60_000,
  stdout: 'ignore',
  stderr: 'pipe',
};

/**
 * Where the suite's own build of the Activity goes: not `dist/`, which is
 * the build a developer deploys or serves, and under node_modules so that
 * git, Prettier, ESLint and vitest all leave it alone.
 */
const APP_BUILD = 'node_modules/.leaf-e2e/dist';

/**
 * The production bundle, built for the e2e server's application id and
 * talking to the API on its own origin whatever a local `.env` says.
 */
const BUILD_APP = `npx vite build --outDir ${APP_BUILD} --emptyOutDir --logLevel warn`;

/** Runs from `activity/`, like every command here. */
const RUN_SERVER = 'cargo run --manifest-path ../Cargo.toml -p leaf-server --example e2e_server';

/**
 * Set to a `tracing` filter (`info`, `debug`, `leaf_server=debug`) to see the
 * leaf server's own log in the run's output.
 */
const SERVER_LOG = process.env.LEAF_E2E_SERVER_LOG;

/**
 * leaf-server's `e2e_server` example: the real API over a seeded database,
 * an in-memory store and a stand-in for Discord, serving the built Activity
 * through the real static router. `build` puts the bundle there first.
 */
function appServer(engine: AppEngine, build: boolean): WebServer {
  const origin = APP_ORIGINS[engine];
  return {
    name: `leaf ${engine}`,
    command: build ? `${BUILD_APP} && ${RUN_SERVER}` : RUN_SERVER,
    url: `${origin}/healthz`,
    reuseExistingServer: !CI,
    // The first run compiles leaf-server and everything under it.
    timeout: 20 * 60_000,
    // The server removes its database on the way out, given the chance.
    gracefulShutdown: { signal: 'SIGTERM', timeout: 5_000 },
    stdout: SERVER_LOG ? 'pipe' : 'ignore',
    stderr: 'pipe',
    env: {
      VITE_DISCORD_CLIENT_ID: APP_CLIENT_ID,
      VITE_API_BASE: '/api',
      E2E_PORT: new URL(origin).port,
      STATIC_DIR: APP_BUILD,
      LOG_LEVEL: SERVER_LOG ?? 'warn',
      SQLX_OFFLINE: 'true',
    },
  };
}

/** The project names given with `--project`, as Playwright reads them. */
function projectFilters(argv: readonly string[]): string[] {
  const names: string[] = [];
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i] ?? '';
    if (arg.startsWith('--project=')) {
      names.push(arg.slice('--project='.length));
    } else if (arg === '--project') {
      // The option takes every word up to the next option.
      while (i + 1 < argv.length && !argv[i + 1]?.startsWith('-')) {
        i += 1;
        names.push(argv[i] ?? '');
      }
    }
  }
  return names;
}

/** Whether a `--project` value (which may hold `*`) selects a project. */
function selects(filter: string, project: string): boolean {
  const pattern = filter
    .split('*')
    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('.*');
  return new RegExp(`^${pattern}$`, 'i').test(project);
}

/**
 * The servers a run needs. Playwright starts every server it is given,
 * whichever projects were asked for, and the leaf server takes a cargo build
 * to start: so a run that names its projects (`npm run e2e:mock`) gets only
 * the servers behind them. `projects` maps each project to its server.
 *
 * Whichever leaf server starts first also builds the Activity; Playwright
 * starts them one after another, so the second finds the bundle in place.
 */
export function serversFor(
  argv: readonly string[],
  projects: Readonly<Record<string, 'mock' | AppEngine>>,
): WebServer[] {
  const filters = projectFilters(argv);
  const wanted = new Set(
    Object.entries(projects)
      .filter(([name]) => filters.length === 0 || filters.some((f) => selects(f, name)))
      .map(([, server]) => server),
  );
  const servers: WebServer[] = [];
  if (wanted.has('mock')) servers.push(mockServer);
  const engines = (Object.keys(APP_ORIGINS) as AppEngine[]).filter((e) => wanted.has(e));
  engines.forEach((engine, i) => servers.push(appServer(engine, i === 0)));
  return servers;
}
