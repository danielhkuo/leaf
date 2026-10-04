// A stand-in for the Discord client around the Activity: a page that frames
// it with Discord's launch parameters and answers the embedded-app SDK's
// messages as the client does. The Activity inside is the real bundle, and
// its SDK is the real `@discord/embedded-app-sdk`; only the far side of
// `postMessage` is played here.
//
// The SDK accepts messages from its own origin (and Discord's), so the
// stand-in is served under the origin of the server under test, at a path
// that server does not have.
//
// The protocol, from the SDK's own source (`output/Discord.mjs`,
// `output/schema`): every message is `[opcode, payload]`.
//
// - `[0, {v, encoding, client_id, frame_id}]` is the handshake. The client
//   answers `[1, {cmd: 'DISPATCH', evt: 'READY', data, nonce: null}]`.
// - `[1, {cmd, args, evt, nonce}]` is a command. The answer is
//   `[1, {cmd, data, evt: null, nonce}]`, or `evt: 'ERROR'` with
//   `data: {code, message}` for a refusal. The SDK checks `data` against its
//   schema for the command and logs an error for one that does not fit.
// - `[2, {code, message}]` asks the client to close the Activity.

import type { FrameLocator, Page } from '@playwright/test';

/** A path the leaf server does not serve; the stand-in is fulfilled there. */
export const DISCORD_PATH = '/__discord';

/** What launches carry as `frame_id` and `instance_id`. */
export const FRAME_ID = 'e2e-frame';
const INSTANCE_ID = 'e2e-instance';

/**
 * The `mobile_app_version` a phone launch carries. The SDK names its own
 * version in the handshake to any phone client from 250 on, which is every
 * client in use.
 */
const MOBILE_APP_VERSION = '300.0';

/** `layout_mode` of ACTIVITY_LAYOUT_MODE_UPDATE. */
const LAYOUT_MODES = { focused: 0, pip: 1, grid: 2 } as const;
export type LayoutMode = keyof typeof LAYOUT_MODES;

/** The person AUTHENTICATE answers with. */
export interface DiscordUser {
  id: string;
  username: string;
  displayName: string;
}

/** What the client does when asked. A test may change these while the Activity runs. */
export interface Behaviour {
  /**
   * AUTHORIZE: hand out the code, refuse as a dismissed permission sheet
   * does, or leave the sheet open (no answer until this changes).
   */
  authorize: 'grant' | 'decline' | 'wait';
  /** Whether the handshake is answered with READY. */
  ready: boolean;
  /**
   * OPEN_EXTERNAL_LINK: the link opens, the person stays on Discord's
   * "leaving Discord" prompt, or the client refuses the command.
   */
  openLink: 'opened' | 'cancelled' | 'refused';
}

export interface Launch extends Behaviour {
  /** The application the Activity was launched for. */
  applicationId: string;
  /** What Discord calls that application: its entry in the Apps list. */
  applicationName: string;
  /** The OAuth code AUTHORIZE hands out. */
  code: string;
  /** Who each access token belongs to, for AUTHENTICATE. */
  users: Record<string, DiscordUser>;
  platform: 'mobile' | 'desktop';
  /** `null` for a launch from a DM: Discord sends no `guild_id`. */
  guildId: string | null;
  channelId: string;
  /** The `custom_id` of the activity link that was pressed, if one was. */
  customId: string | null;
}

/** A command the Activity sent. */
export interface Command {
  cmd: string;
  evt: string | null;
  args: unknown;
}

/** What the stand-in keeps on its window for the test to read and drive. */
interface Client {
  behaviour: Behaviour;
  handshakes: unknown[];
  commands: Command[];
  closes: unknown[];
  /** Commands the stand-in has no answer for. */
  unanswered: string[];
  behave(change: Partial<Behaviour>): void;
  layout(mode: number): void;
}

declare global {
  interface Window {
    /** Only on the stand-in page. */
    discordClient: Client;
  }
}

/**
 * The client's side of the protocol. Runs inside the stand-in page, so it
 * uses nothing from this module but its argument.
 */
function runClient(
  launch: Launch,
  frameId: string,
  instanceId: string,
  mobileAppVersion: string,
): void {
  const LAYOUT_EVENT = 'ACTIVITY_LAYOUT_MODE_UPDATE';
  const frame = document.createElement('iframe');
  // As in Discord, the Activity is sandboxed: it can open no popup or modal
  // dialog and cannot navigate the window around it.
  frame.setAttribute('sandbox', 'allow-forms allow-pointer-lock allow-same-origin allow-scripts');
  frame.title = 'Activity';

  const query = new URLSearchParams({
    frame_id: frameId,
    instance_id: instanceId,
    platform: launch.platform,
    channel_id: launch.channelId,
  });
  if (launch.platform === 'mobile') query.set('mobile_app_version', mobileAppVersion);
  if (launch.guildId !== null) query.set('guild_id', launch.guildId);
  if (launch.customId !== null) query.set('custom_id', launch.customId);
  frame.src = `/?${query.toString()}`;

  const post = (message: unknown): void => {
    frame.contentWindow?.postMessage(message, location.origin);
  };
  const dispatch = (evt: string, data: unknown): void => {
    post([1, { cmd: 'DISPATCH', evt, data, nonce: null }]);
  };
  const answer = (cmd: string, nonce: string, data: unknown): void => {
    post([1, { cmd, data, evt: null, nonce }]);
  };
  const refuse = (cmd: string, nonce: string, code: number, message: string): void => {
    post([1, { cmd, evt: 'ERROR', data: { code, message }, nonce }]);
  };

  let handshaken = false;
  let readySent = false;
  let layoutMode = 0;
  const subscribed = new Set<string>();
  /** AUTHORIZE commands left open: the permission sheet is up. */
  let sheets: string[] = [];

  function sendReady(): void {
    if (!handshaken || readySent || !client.behaviour.ready) return;
    readySent = true;
    dispatch('READY', {
      v: 1,
      config: { cdn_host: '', api_endpoint: '', environment: 'production' },
    });
  }

  function authorize(nonce: string): void {
    const { authorize: how } = client.behaviour;
    if (how === 'wait') sheets.push(nonce);
    else if (how === 'grant') answer('AUTHORIZE', nonce, { code: launch.code });
    else refuse('AUTHORIZE', nonce, 5000, 'OAuth2 Error: access_denied');
  }

  function command(cmd: string, evt: string | null, nonce: string, args: unknown): void {
    const fields = (args ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case 'AUTHORIZE':
        authorize(nonce);
        return;
      case 'AUTHENTICATE': {
        const token = typeof fields.access_token === 'string' ? fields.access_token : '';
        const user = Object.prototype.hasOwnProperty.call(launch.users, token)
          ? launch.users[token]
          : undefined;
        if (!user) {
          refuse(cmd, nonce, 4009, 'Invalid OAuth2 access token');
          return;
        }
        answer(cmd, nonce, {
          access_token: token,
          user: {
            id: user.id,
            username: user.username,
            discriminator: '0',
            global_name: user.displayName,
            avatar: null,
            public_flags: 0,
          },
          scopes: ['identify'],
          expires: new Date(Date.now() + 7 * 86_400_000).toISOString(),
          application: {
            id: launch.applicationId,
            name: launch.applicationName,
            description: '',
            icon: null,
          },
        });
        return;
      }
      case 'SUBSCRIBE':
        if (evt === null) break;
        subscribed.add(evt);
        answer(cmd, nonce, { evt });
        // Discord publishes the current layout mode to a new subscriber.
        if (evt === LAYOUT_EVENT) dispatch(LAYOUT_EVENT, { layout_mode: layoutMode });
        return;
      case 'UNSUBSCRIBE':
        if (evt === null) break;
        subscribed.delete(evt);
        answer(cmd, nonce, { evt });
        return;
      case 'CAPTURE_LOG':
        answer(cmd, nonce, null);
        return;
      case 'OPEN_EXTERNAL_LINK': {
        const { openLink } = client.behaviour;
        if (openLink === 'refused') refuse(cmd, nonce, 4002, 'Invalid command');
        else answer(cmd, nonce, { opened: openLink === 'opened' });
        return;
      }
    }
    client.unanswered.push(evt === null ? cmd : `${cmd} ${evt}`);
    refuse(cmd, nonce, 4002, 'Invalid command');
  }

  const client: Client = {
    behaviour: { authorize: launch.authorize, ready: launch.ready, openLink: launch.openLink },
    handshakes: [],
    commands: [],
    closes: [],
    unanswered: [],
    behave(change) {
      Object.assign(client.behaviour, change);
      sendReady();
      if (client.behaviour.authorize !== 'wait') {
        const open = sheets;
        sheets = [];
        open.forEach(authorize);
      }
    },
    layout(mode) {
      layoutMode = mode;
      if (subscribed.has(LAYOUT_EVENT)) dispatch(LAYOUT_EVENT, { layout_mode: mode });
    },
  };
  window.discordClient = client;

  window.addEventListener('message', (event) => {
    if (event.source !== frame.contentWindow || !Array.isArray(event.data)) return;
    const [opcode, payload] = event.data as [unknown, Record<string, unknown> | undefined];
    if (opcode === 0) {
      client.handshakes.push(payload);
      handshaken = true;
      sendReady();
    } else if (opcode === 1 && payload) {
      const cmd = String(payload.cmd);
      const evt = typeof payload.evt === 'string' ? payload.evt : null;
      client.commands.push({ cmd, evt, args: payload.args ?? null });
      command(cmd, evt, String(payload.nonce), payload.args);
    } else if (opcode === 2) {
      client.closes.push(payload);
    }
  });

  // Listening before the frame exists: its first message is the handshake.
  document.body.append(frame);
}

/** The stand-in page. The viewport line keeps a phone-sized page phone-sized. */
function page(launch: Launch): string {
  const args = [launch, FRAME_ID, INSTANCE_ID, MOBILE_APP_VERSION]
    .map((arg) => JSON.stringify(arg))
    .join(', ');
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
    <title>Discord (stand-in)</title>
    <style>
      html, body { height: 100%; margin: 0; overflow: hidden; background: #1e1f22; }
      iframe { display: block; width: 100%; height: 100%; border: 0; }
    </style>
  </head>
  <body>
    <script>(${runClient.toString()})(${args.replace(/</g, '\\u003c')});</script>
  </body>
</html>`;
}

/** The stand-in as a test holds it: the Activity inside, and the client's side to read and drive. */
export class DiscordClient {
  readonly #page: Page;
  /** The Activity. */
  readonly activity: FrameLocator;

  constructor(host: Page) {
    this.#page = host;
    this.activity = host.frameLocator('iframe');
  }

  /** Changes how the client answers from now on; a sheet left open or a READY held back follows. */
  async behave(change: Partial<Behaviour>): Promise<void> {
    await this.#page.evaluate((next) => window.discordClient.behave(next), change);
  }

  /** Puts the Activity in a layout mode, telling it if it subscribed. */
  async layout(mode: LayoutMode): Promise<void> {
    await this.#page.evaluate((next) => window.discordClient.layout(next), LAYOUT_MODES[mode]);
  }

  /**
   * The person comes back to the Activity: out of picture-in-picture, into
   * the full view.
   */
  async foreground(): Promise<void> {
    await this.layout('pip');
    await this.layout('focused');
  }

  /** Every handshake the Activity sent. One, for an Activity that booted once. */
  handshakes(): Promise<unknown[]> {
    return this.#page.evaluate(() => window.discordClient.handshakes);
  }

  /** The commands the Activity sent, oldest first; only those named `cmd` when given. */
  commands(cmd?: string): Promise<Command[]> {
    return this.#page.evaluate(
      (name) => window.discordClient.commands.filter((sent) => name === null || sent.cmd === name),
      cmd ?? null,
    );
  }

  /** What the Activity sent to ask Discord to close it. */
  closes(): Promise<unknown[]> {
    return this.#page.evaluate(() => window.discordClient.closes);
  }

  /** Commands the stand-in had no answer for. */
  unanswered(): Promise<string[]> {
    return this.#page.evaluate(() => window.discordClient.unanswered);
  }
}

/** Opens the stand-in in `host` and launches the Activity inside it. */
export async function launchActivity(host: Page, launch: Launch): Promise<DiscordClient> {
  const pattern = `**${DISCORD_PATH}`;
  await host.unroute(pattern);
  await host.route(pattern, (route) =>
    route.fulfill({ contentType: 'text/html; charset=utf-8', body: page(launch) }),
  );
  await host.goto(DISCORD_PATH);
  return new DiscordClient(host);
}

/** What the person does on Discord's consent screen: approve as someone, or cancel. */
export type Consent = { code: string } | { error: string };

/**
 * Stands in for Discord's OAuth consent screen in a plain browser page (the
 * admin panel's sign-in). leaf's `/admin/login` answers with a redirect to
 * discord.com, and the browser follows a redirect by itself, where no route
 * handler sees it. So the login request is taken here instead: the server is
 * asked without following the redirect, and the browser gets a page that
 * sends it where Discord would, back to the `redirect_uri` with the `state`
 * it was given and a code (or an error). Nothing goes to discord.com.
 *
 * Resolves to the list of authorize URLs leaf redirected to, which grows
 * with every sign-in.
 */
export async function standInForConsent(host: Page, consent: Consent): Promise<URL[]> {
  const asked: URL[] = [];
  await host.route('**/admin/login', async (route) => {
    const answer = await route.fetch({ maxRedirects: 0 });
    const authorize = new URL(answer.headers().location ?? '');
    asked.push(authorize);
    const back = new URL(authorize.searchParams.get('redirect_uri') ?? '');
    back.searchParams.set('state', authorize.searchParams.get('state') ?? '');
    for (const [key, value] of Object.entries(consent)) back.searchParams.set(key, value);
    await route.fulfill({
      contentType: 'text/html; charset=utf-8',
      body: `<!doctype html><title>Discord (stand-in)</title>
<script>location.replace(${JSON.stringify(back.toString()).replace(/</g, '\\u003c')});</script>`,
    });
  });
  return asked;
}
