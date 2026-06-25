# leaf activity (embedded app)

The gallery: a Vite + Svelte 5 SPA that runs inside Discord's activity iframe
and talks to `leaf-server` over the REST API. See
`docs/svelte-guidelines.md` for code standards and the UI/UX plan for the
screen design.

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
> **<http://localhost:5173/mock.html>** — a gallery of every screen rendered
> with fixture data and no SDK, network, or auth. Pick a screen in the sidebar;
> toggle **Phone / Wide** to preview responsive layouts (each screen renders in
> a real device-width iframe). Dev-only — it never ships in `dist/`. Source:
> [`src/mock/`](src/mock/).

> Run `cargo` commands from the repo root and `npm` commands from `activity/`.

### What you need (and where each value goes)

| Value                       | Where to get it                             | Where it goes                                                        |
| --------------------------- | ------------------------------------------- | -------------------------------------------------------------------- |
| **Application (Client) ID** | Dev Portal → your app → General Information | `activity/.env` (`VITE_DISCORD_CLIENT_ID`) **and** leaf-server setup |
| **Client Secret**           | Dev Portal → OAuth2 → Reset Secret          | leaf-server setup only — never the frontend                          |
| **Bot Token**               | Dev Portal → Bot → Reset Token              | leaf-server setup                                                    |
| **R2 bucket + keys**        | Cloudflare dashboard → R2                   | leaf-server setup                                                    |
| **Public URL**              | your tunnel hostname (step 2)               | leaf-server setup (`public_url`)                                     |

### 1. Create a dev Discord application

At <https://discord.com/developers/applications> → **New Application** (name it
e.g. "leaf (dev)"; keep it separate from any production app so URL mappings and
redirects don't collide). Then, in the left sidebar:

- **General Information** → copy the **Application ID**.
- **Bot** → **Reset Token** and copy it. Scroll to **Privileged Gateway
  Intents** and turn on **Message Content Intent**.
- **OAuth2** → copy the **Client Secret** (Reset Secret if blank). Under
  **Redirects**, **Add** your tunnel URL from step 2, e.g.
  `https://leaf-dev.example.com`. **Save Changes**.
- **Activities** → enable it, then under **URL Mappings** add **Prefix** `/`
  → **Target** = your tunnel host **without** the scheme, e.g.
  `leaf-dev.example.com`. **Save**.

Invite the app to a test server: **OAuth2 → URL Generator**, tick **`bot`** and
**`applications.commands`**, open the generated URL, and add it to a server you
can test in. (The bot must be a member so leaf-server can check who may view a
series. The activity's own `identify`/`guilds` scopes are requested at runtime
by the SDK, not here.)

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

leaf-server stores its secrets in `/data/leaf.conf` via a small web setup flow.

```sh
cargo run --bin leaf
```

With no config it starts in **setup mode** and prints a `/setup` URL and a
one-time **setup code** to the terminal. Open `http://localhost:3777/setup`,
enter the code, then fill in the **Application ID**, **Client Secret**, **Bot
Token**, **R2** bucket/keys, and **Public URL** = `https://leaf-dev.example.com`.
Saving validates the values and flips leaf-server into run mode.

Already configured from a previous phase? Update just the public URL with
`cargo run --bin leaf -- --reconfigure`.

### 4. Configure the frontend

```sh
cd activity
cp .env.example .env        # set VITE_DISCORD_CLIENT_ID=<Application ID>
npm install
```

### 5. Run it (three terminals)

```sh
cargo run --bin leaf                                          # API + assets on :3777
LEAF_DEV_HOST=leaf-dev.example.com npm run dev                # Vite on :5173 (from activity/)
cloudflared tunnel run leaf-dev                               # tunnel → :5173
```

`LEAF_DEV_HOST` adds your tunnel host to Vite's `allowedHosts` (Vite blocks
unknown hosts since 5.4). Quick-tunnel users can omit it.

### 6. Launch in Discord

In your test server, join a **voice channel**, open the **Activities** launcher
(the rocket icon), and pick your app. You should land on the series picker /
heatmap; tapping a day opens the Phase-17 viewer stub.

### Troubleshooting

- **"Connecting to Discord…" then an error in a normal browser** — expected; the
  handshake only works inside the Discord client.
- **"Blocked request. This host is not allowed."** — set `LEAF_DEV_HOST` to your
  tunnel host (see step 5).
- **Token exchange fails (400)** — the **URL Mapping** target, the **OAuth2
  Redirect**, and leaf-server's **Public URL** must all name the same origin.
- **Empty / "not a member"** — confirm the bot is in the test server and the
  series is visible to your account.
- HMR may not survive the proxy round-trip; a manual reload always works.

## Build

`vite build` emits `dist/`, which leaf-server serves from `STATIC_DIR`.
Production deployment (reverse proxy, the production Discord app) is wired in
Phase 18 and documented then — not here yet.
