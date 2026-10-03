# leaf activity (embedded app)

The gallery: a Vite + Svelte 5 SPA that runs inside Discord's activity iframe
and talks to `leaf-server` over the REST API. The same bundle also holds the
admin panel (`/admin`, a browser page) and the "leaf is running" page a browser
gets at `/`. Code standards are in `docs/svelte-guidelines.md` (a local-only
file; `docs/` is gitignored).

## Scripts

| Command                           | What it does                                            |
| --------------------------------- | ------------------------------------------------------- |
| `npm run dev`                     | Vite dev server on :5173, proxying `/api` → leaf-server |
| `npm run build`                   | `svelte-check` then `vite build` → `dist/`              |
| `npm run check`                   | Type-check (`svelte-check`)                             |
| `npm run lint`                    | ESLint (flat config; strict)                            |
| `npm run format` / `format:check` | Prettier                                                |
| `npm run test` / `test:run`       | Vitest (watch / once)                                   |
| `npm run bundle:check`            | Fail if the initial JS chunk blows the budget           |

## Running it in development

A Discord Activity is an iframe served from `https://<app-id>.discordsays.com/`,
and Discord's proxy fetches your machine through a public HTTPS **tunnel**. So
the dev loop is: run leaf-server + Vite locally, expose Vite over a tunnel, and
point a **dev** Discord application at that tunnel. There is no localhost-only
way to see the authed gallery — the SDK handshake only completes inside the
Discord client.

> **Just want to look at the screens?** You don't need any of the tunnel /
> Discord setup below. With the dev server running (`npm run dev`), open
> **<http://localhost:5173/mock.html>**: every screen (23 of them), with fixture
> data and no SDK, network, or auth. Pick a screen in the sidebar. The width
> buttons (**320 / 375 / 430 / Wide**) render it in an iframe of that width,
> and **Long text** fills names and captions with worst-case strings. The
> gallery and creator screens are the real views over a mock API, so taps,
> navigation and saves work (state lasts until reload); the boot screens and
> the admin page's header are copies. `mock.html?embed=1&screen=<id>[&long=1]`
> opens one screen on its own; the ids are in
> [`src/mock/screens.ts`](src/mock/screens.ts). Dev-only: it never ships in
> `dist/`. Source: [`src/mock/`](src/mock/).

> Run `cargo` commands from the repo root and `npm` commands from `activity/`.

### What you need (and where each value goes)

| Value                       | Where to get it                             | Where it goes                                                                    |
| --------------------------- | ------------------------------------------- | -------------------------------------------------------------------------------- |
| **Application (Client) ID** | Dev Portal → your app → General Information | leaf-server setup (the gallery reads it from its `<id>.discordsays.com` address) |
| **Client Secret**           | Dev Portal → OAuth2 → Reset Secret          | leaf-server setup only — never the frontend                                      |
| **Bot Token**               | Dev Portal → Bot → Reset Token              | leaf-server setup                                                                |
| **R2 bucket + keys**        | Cloudflare dashboard → R2                   | leaf-server setup                                                                |
| **Public URL**              | your tunnel hostname (step 2)               | leaf-server setup (`public_url`)                                                 |

### 1. Create a dev Discord application

At <https://discord.com/developers/applications> → **New Application** (name it
e.g. "leaf (dev)"; keep it separate from any production app so URL mappings and
redirects don't collide). Then, in the left sidebar:

- **General Information** → copy the **Application ID**.
- **Bot** → **Reset Token** and copy it. Leave the **Privileged Gateway
  Intents** off; the bot connects without them.
- **OAuth2** → copy the **Client Secret** (Reset Secret if blank). Under
  **Redirects**, **Add** your tunnel URL from step 2, e.g.
  `https://leaf-dev.example.com`, and the same with `/admin/callback` if you
  want the admin panel. **Save Changes**.
- **Activities** → enable it; under **Supported Platforms** tick **iOS** and
  **Android** if you test on a phone (they are off by default); under **URL
  Mappings** add **Prefix** `/` → **Target** = your tunnel host **without** the
  scheme, e.g. `leaf-dev.example.com`. **Save**.

Invite the app to a test server: **OAuth2 → URL Generator**, tick **`bot`** and
**`applications.commands`**, open the generated URL, and add it to a server you
can test in. (The bot must be a member so leaf-server can check who may view a
series. The gallery's own `identify` scope is requested at runtime by the SDK,
not here.)

### 2. Set up the tunnel (stable hostname)

A **named** `cloudflared` tunnel gives a fixed `https://leaf-dev.<domain>`, so
you configure Discord once. It connects Cloudflare straight to your laptop and
**does not touch your home server, reverse proxy, or DDNS**.

```sh
brew install cloudflared
cloudflared tunnel login                                    # authorize your domain's zone
cloudflared tunnel create leaf-dev                          # prints a tunnel UUID + creds .json
cloudflared tunnel route dns leaf-dev leaf-dev.example.com  # creates the DNS record
```

Then `~/.cloudflared/config.yml`:

```yaml
tunnel: leaf-dev
credentials-file: /Users/you/.cloudflared/<TUNNEL-UUID>.json
ingress:
  - hostname: leaf-dev.example.com
    service: http://localhost:5173
  - service: http_status:404
```

_Throwaway alternative (no domain):_ `cloudflared tunnel --url
http://localhost:5173` prints a random `https://<x>.trycloudflare.com` — already
allowed by `vite.config.ts`, but you must re-paste that URL into the URL
mapping, the OAuth redirect, and `public_url` every run.

### 3. Configure leaf-server (one-time)

leaf-server stores its secrets in `leaf.conf` in its data directory, written by
a small web setup flow. With `cargo run` from the repo root that directory is
`./data` (`DATA_DIR` changes it); in Docker it is `/data`.

```sh
export DEV_GUILD_ID=<your test server id>   # optional: see guide/01-install.md
cargo run --bin leaf
```

`DEV_GUILD_ID` makes the bot register its commands in that one server (it must
be a member) instead of globally, which keeps a dev bot's commands apart.
Without it the commands are registered globally, as in production. If you use
it, keep it exported for every `cargo run` below.

With no config it starts in **setup mode** and prints a `/setup` URL and a
one-time **setup code** to the terminal. Open `http://localhost:3777/setup`,
enter the code, then fill in the **Application ID**, **Client Secret**, **Bot
Token**, **R2** bucket/keys, and **Public URL** = `https://leaf-dev.example.com`.
Saving validates the values and flips leaf-server into run mode.

Already configured? Change the public URL with
`cargo run --bin leaf -- --reconfigure`. The form starts empty, so every value
has to be entered again.

### 4. Install the frontend

```sh
cd activity
npm install
```

No `.env` is needed. `.env.example` lists the two optional overrides.

### 5. Run it (three terminals)

```sh
cargo run --bin leaf                                          # API + assets on :3777
LEAF_DEV_HOST=leaf-dev.example.com npm run dev                # Vite on :5173 (from activity/)
cloudflared tunnel run leaf-dev                               # tunnel → :5173
```

`LEAF_DEV_HOST` adds your tunnel host to Vite's `allowedHosts` (Vite blocks
unknown hosts since 5.4). Quick-tunnel users can omit it.

### 6. Launch in Discord

In your test server, in a text channel: on desktop click the **Apps** button in
the message box and pick your app; on a phone tap **+** next to the message
box, then **Apps**, then your app. `/gallery` and the **Open gallery** buttons
on the bot's messages open it too. You land on the series list, or on a series
(the one a button or `/gallery series:` named, the one that uses this channel,
the one you opened last, or the only one there is).

### Troubleshooting

- **A page saying "leaf is running" in a normal browser** — expected. Without
  Discord's launch parameters the app shows that page instead of the gallery;
  the handshake only works inside the Discord client.
- **The bot is offline or has no commands** — `curl localhost:3777/api/status`
  says whether the gateway is `online`, `starting` or in `error` (with the
  reason), and carries a `notice` when the bot is online but Discord refused
  its command list. The terminal running `cargo run` has the detail. With
  `DEV_GUILD_ID` set, the bot must be in that server for its commands to
  register (guide/07-troubleshooting.md, "Bot offline").
- **"Blocked request. This host is not allowed."** — set `LEAF_DEV_HOST` to your
  tunnel host (see step 5).
- **Token exchange fails (400)** — the **URL Mapping** target, the **OAuth2
  Redirect**, and leaf-server's **Public URL** must all name the same origin.
- **"Couldn't load the gallery"** — with "leaf's bot isn't in this server",
  invite the dev bot to the test server and let it connect once (leaf-server
  learns of a server when the bot is online in it). With "You don't have access
  to this", your account isn't a member of that server.
- **"No series here yet"** although one exists — it isn't visible to your
  account (private, role-limited, someone else's sprout, or revoked).
- HMR may not survive the proxy round-trip; close the Activity and open it
  again (reloading only the iframe hangs the handshake).

## Build

`vite build` emits `dist/`, which leaf-server serves from `STATIC_DIR`
(`activity/dist` when unset). The Docker image does this build itself, with no
build arguments. Production deployment is covered in
[../DEPLOY.md](../DEPLOY.md) and [../guide/](../guide/README.md).
