# 01 — Install & first-run setup

This guide gets the container running and walks the one-time setup form. You'll
start it here, gather credentials in [02-discord.md](02-discord.md) and
[03-cloudflare.md](03-cloudflare.md), then come back to
[§ First-run setup](#first-run-setup).

## How leaf runs

leaf is a **single process** with a **two-state boot** (`crates/leaf/src/main.rs`):

- **Setup mode** — when no config file exists at `<DATA_DIR>/leaf.conf`, the
  bot does *not* connect to Discord. The web server comes up serving only a
  local setup page and prints a one-time **setup code** to the logs.
- **Run mode** — once the config is written, leaf connects the gateway and
  serves the API + gallery + admin panel.

All persistent state lives in one directory (the `/data` volume in Docker): the
`leaf.conf` credentials file and the `leaf.db` SQLite database. Media does **not**
live here — it goes to Cloudflare R2.

## Prerequisites

- A machine with **Docker** + the Docker Compose plugin (a home server, NAS,
  or VPS). The gateway connection is outbound-only — **no port forwarding** and
  no exposed home IP are required.
- A **domain** and a **Cloudflare account** (free tier is plenty). Both are set
  up in [03-cloudflare.md](03-cloudflare.md).
- A **Discord application** (created in [02-discord.md](02-discord.md)).

## 1. Get the code and start the container

```sh
git clone <your-leaf-repo-url> leaf
cd leaf
docker compose up -d                 # builds the image, starts leaf on :3777
docker compose logs -f leaf          # watch startup and grab the setup code
```

The image builds the gallery and the Rust binary, then runs leaf. On first boot
with an empty data volume you'll see something like:

```
leaf  | no configuration found — starting in setup mode
leaf  | → open http://localhost:3777/setup (or your mapped host)
leaf  | → setup code: 7QF2-IDT4
```

> The setup **code** (printed only to the logs, single-use, format `XXXX-XXXX`)
> is what protects the setup page — Discord OAuth can't, because the bot isn't
> running yet. If you lose it, restart the container to mint a new one.

At this point leaf is reachable at `http://localhost:3777` but **not yet
public**. Before you can finish setup you need a public HTTPS origin and your
credentials — do [02-discord.md](02-discord.md) and [03-cloudflare.md](03-cloudflare.md)
now, then return here.

## Environment variables

leaf needs **no env vars to function** — runtime config is the setup form, not
the environment (PLAN.md § Configuration). Only these machine-level knobs exist,
and the defaults are fine for the supported Docker deployment:

| Var | Default (Docker) | Purpose |
| --- | --- | --- |
| `DATA_DIR` | `/data` | Where `leaf.conf` and `leaf.db` live (the volume). |
| `BIND_ADDR` | `0.0.0.0:3777` | Address/port the web server binds. |
| `STATIC_DIR` | `/app/dist` | Built gallery assets (set in the image). |
| `LOG_LEVEL` | `info` | `tracing` env-filter (e.g. `info,leaf_bot=debug`). |
| `DEV_GUILD_ID` | *(unset)* | If set, registers slash commands to that one guild **instantly** instead of globally (which can take ~1h to propagate). Handy while testing; leave unset in production. |

Set any of these in a `.env` file next to `docker-compose.yml` (the compose file
already wires `LOG_LEVEL` and `DEV_GUILD_ID` through). Do **not** put
credentials here — those go through the setup page into `leaf.conf`.

## First-run setup

Once you have a public hostname pointed at the container (see
[03-cloudflare.md](03-cloudflare.md)), open **`https://leaf.example.com/setup`**
(or `http://localhost:3777/setup` if you're finishing locally before exposing
it). `/` redirects to `/setup` automatically.

Enter the **setup code** from the logs, then fill the form. Every field is
required, and the page itself links to the exact place each value comes from:

| Section | Field | Where it comes from |
| --- | --- | --- |
| Discord application | **Bot token** | [02-discord.md](02-discord.md) — Bot → Reset Token |
| | **Application (client) ID** | [02-discord.md](02-discord.md) — OAuth2 → Client information |
| | **OAuth client secret** | [02-discord.md](02-discord.md) — OAuth2 → Reset Secret |
| Public origin | **Public URL** | your hostname, e.g. `https://leaf.example.com` |
| Cloudflare R2 | **S3 endpoint** | [03-cloudflare.md](03-cloudflare.md) — R2 → Overview, `https://<account-id>.r2.cloudflarestorage.com` |
| | **Bucket** | [03-cloudflare.md](03-cloudflare.md) — the bucket you created |
| | **Access key ID** + **Secret access key** | [03-cloudflare.md](03-cloudflare.md) — R2 API token (Object Read & Write) |

When you submit, leaf **validates everything live** before writing anything:

- Discord: the bot token (`GET /users/@me`) and the OAuth client ID/secret pair.
- R2: a put + get + delete of a canary object in your bucket.

On success it writes `leaf.conf` (owner-only `0600`) to the data volume,
invalidates the setup code, and transitions into run mode **in-process — no
restart**. The bot connects and the gallery comes online. Continue to
[04-usage.md](04-usage.md).

> The **Public URL**, the Discord **OAuth redirect**, and the Discord **URL
> mapping target** must all name the **same origin**, or authentication fails.
> See [02-discord.md](02-discord.md#the-three-hosts-must-match).

## Changing credentials later (`--reconfigure`)

Tier-1 values (tokens, R2 keys, public URL) are not editable from the admin
panel — they're the setup flow's job. To rotate one, run the container once with
`--reconfigure`, which re-enters setup mode (pre-filled except secrets) over the
existing config:

```sh
docker compose run --rm leaf --reconfigure
# then open /setup again, enter a fresh code from the logs, and resubmit
```

(Per-guild/per-series settings — channels, policy, privacy — are **not** Tier-1;
pick channels in Discord with `/setup`, and edit policy, timezone, creator role,
and series privacy/revoke in the admin panel. See [04-usage.md](04-usage.md).)

## Data, backups, and updates

- **Everything persistent is the `leaf-data` volume** (`/data`): your config and
  the SQLite database. Back it up by snapshotting that volume (stop the
  container first for a consistent copy, or use SQLite's online backup). Media is
  separately durable in R2.
- **Update** to a new version:
  ```sh
  git pull && docker compose up -d --build
  ```
  This rebuilds the gallery and binary from the working tree and restarts,
  reusing the volume — your config and database persist.

For a public production deploy with the Cloudflare Tunnel sidecar and the admin
panel, [DEPLOY.md](../DEPLOY.md) is the quick reference; this guide set is the
detailed version.
