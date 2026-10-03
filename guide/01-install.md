# 01 — Install & first-run setup

This guide gets the container running and walks the one-time setup form. You'll
start it here, gather credentials in [02-discord.md](02-discord.md) and
[03-cloudflare.md](03-cloudflare.md), then come back to
[§ First-run setup](#first-run-setup).

## How leaf runs

leaf is a **single process** with a **two-state boot** (`crates/leaf/src/main.rs`):

- **Setup mode**: when no config file exists at `<DATA_DIR>/leaf.conf`, the
  bot does *not* connect to Discord. The web server comes up serving only a
  local setup page and prints a one-time **setup code** to the logs.
- **Run mode**: once the config is written, leaf connects the bot to Discord
  and serves the API, the gallery and the admin panel.

All persistent state lives in one directory (the `/data` volume in Docker): the
`leaf.conf` credentials file and the `leaf.db` SQLite database. Media does **not**
live here; it goes to Cloudflare R2.

## Prerequisites

- A machine with **Docker** and the Docker Compose plugin (a home server, NAS,
  or VPS). The bot's connection to Discord is outbound-only: **no port
  forwarding** and no exposed home IP are required.
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

The image builds the gallery and the Rust binary, then runs leaf. Nothing about
your Discord application is baked into the image: the gallery reads the
application ID from the address Discord serves it on. On first boot with an
empty data volume you'll see something like:

```
leaf  | no configuration found — starting in setup mode
leaf  | → open http://localhost:3777/setup (or your mapped host)
leaf  | → setup code: 7QF2-KDT4
```

> The setup **code** (printed only to the logs, format `XXXX-XXXX`) is what
> protects the setup page; Discord sign-in can't, because the bot isn't running
> yet. A code never contains `0`, `O`, `1` or `I`, so there is nothing to
> mix up when typing it. It stops working once setup succeeds, and after ten
> wrong tries. Every start of the container prints a new one, so restart leaf
> if you lose it.

At this point leaf is reachable at `http://localhost:3777` but **not yet
public**. Before you can finish setup you need a public HTTPS origin and your
credentials. Do [02-discord.md](02-discord.md) and [03-cloudflare.md](03-cloudflare.md)
now, then return here.

## Environment variables

Credentials never go in the environment: runtime config is the setup form.
These machine-level settings exist, and the defaults are right for the
supported Docker deployment:

| Var | Default (Docker) | Purpose |
| --- | --- | --- |
| `DATA_DIR` | `/data` | Where `leaf.conf` and `leaf.db` live (the volume). |
| `BIND_ADDR` | `0.0.0.0:3777` | Address and port the web server binds. |
| `STATIC_DIR` | `/app/dist` | Built gallery assets (set in the image). |
| `LOG_LEVEL` | `info` | `tracing` env-filter (e.g. `info,leaf_bot=debug`). |
| `DEV_GUILD_ID` | *(unset)* | Development only: register the commands in this one server instead of globally (next section). Leave it unset for a real install. |

Set any of these in a `.env` file next to `docker-compose.yml` (the compose file
already passes `LOG_LEVEL` and `DEV_GUILD_ID` through), then `docker compose up
-d` to apply. Do **not** put credentials here; those go through the setup page
into `leaf.conf`.

## Command registration and `DEV_GUILD_ID`

leaf registers its slash commands and message menus each time the bot connects.
You don't have to do anything for this.

- **`DEV_GUILD_ID` unset (the default):** the commands are registered
  **globally**, so they exist in every server the bot is in, including ones it
  joins later. Once Activities is enabled
  ([02 § 5](02-discord.md#5-activities-the-gallery)) the application's global
  list also holds Discord's **Entry Point** command; leaf reads the list first
  and sends that command back unchanged along with its own.
- **`DEV_GUILD_ID` set to a server's ID:** the commands are registered in that
  one server only, which keeps a development bot's commands apart from the
  global list. The bot has to be a member of that server.

The log shows `commands registered`, with `scope="global"` or `scope="guild"`,
when it worked. A failed registration does not take the bot offline: the log
has a line starting `registering commands failed`, and leaf tries again by
itself (after 30 seconds, then at growing intervals up to 30 minutes). Until
it works, Discord keeps the command list of an earlier run, so new or changed
commands are missing. While that lasts, `/api/status` carries a `notice`
saying so, and the setup page's status line shows it
([07 § Bot offline](07-troubleshooting.md#bot-offline)).

Switching between the two never leaves every command listed twice: after a
global registration leaf empties the per-server lists of the servers it is in
(skipped when it is in more than 25), and after a `DEV_GUILD_ID` registration
it reduces the global list to the Entry Point command.

> **Set `DEV_GUILD_ID` because an earlier version of this guide said to?**
> Remove the line from `.env` and run `docker compose up -d`. Nothing else is
> needed.

## First-run setup

Once you have a public hostname pointed at the container (see
[03-cloudflare.md](03-cloudflare.md)), open **`https://leaf.example.com/setup`**
(or `http://localhost:3777/setup` if you're finishing locally before exposing
it). `/` redirects to `/setup` automatically.

Enter the **setup code** from the logs, then fill the form. Every field is
required, and the page links to the place each value comes from:

| Section | Field | Where it comes from |
| --- | --- | --- |
| Discord application | **Bot token** | [02-discord.md](02-discord.md): Bot → Reset Token |
| | **Application (client) ID** | [02-discord.md](02-discord.md): OAuth2 → Client information |
| | **OAuth client secret** | [02-discord.md](02-discord.md): OAuth2 → Reset Secret |
| Public origin | **Public URL** | your hostname, e.g. `https://leaf.example.com`: the address only, no path (`http://` is accepted only for localhost) |
| Cloudflare R2 | **S3 endpoint** | [03-cloudflare.md](03-cloudflare.md): R2 → Overview, `https://<account-id>.r2.cloudflarestorage.com` |
| | **Bucket** | [03-cloudflare.md](03-cloudflare.md): the bucket you created |
| | **Access key ID** and **Secret access key** | [03-cloudflare.md](03-cloudflare.md): R2 API token (Object Read & Write) |

When you submit, leaf **checks everything live** before writing anything, and
reports every problem at once, each next to its field:

- Discord: the bot token must belong to the application whose ID you entered
  (`GET /applications/@me`), and the application ID and client secret must work
  as a pair.
- R2: leaf writes, reads back and deletes a small test object in your bucket
  (it gives up after 25 seconds).

On success it writes `leaf.conf` (owner-only `0600`) to the data volume, and
switches to run mode **in the same process, with no restart**. The page then
shows what's next: an invite link for the bot, a link to the admin panel, and
the exact OAuth redirects and URL-mapping target for the Public URL you
entered, to compare with the portal. The bot can be invited before or after
this step; no restart is needed either way.

The page's status line follows the bot: "Connecting the bot to Discord…", then
**"The bot is online."** If the bot can't connect, the line says so and gives
the reason (for example that Discord couldn't be reached and leaf keeps
trying). If it reads "The bot is online, but something needs attention", the
bot is connected and the line under it says what is wrong (for example that
Discord hasn't accepted leaf's command list yet).
[07 § Bot offline](07-troubleshooting.md#bot-offline) has the fixes for both.
The same state is at `https://leaf.example.com/api/status` at any later time.

Continue to [04-usage.md](04-usage.md): the first thing to do in Discord is
`/setup`.

> The **Public URL**, the Discord **OAuth redirects**, and the Discord **URL
> mapping target** must all name the **same origin**, or sign-in fails.
> See [02-discord.md](02-discord.md#the-three-hosts-must-match).

## Changing credentials later (`--reconfigure`)

The values from the setup form (tokens, R2 keys, public URL) can't be edited in
the admin panel. To change one, run leaf once with `--reconfigure`, which shows
the setup page again over the existing config:

```sh
docker compose stop leaf
docker compose run --rm --service-ports --use-aliases leaf --reconfigure
# 1. the setup code is printed in this terminal
# 2. open /setup, enter the code, fill the form, submit
# 3. when the page says leaf is set up, press Ctrl-C here
docker compose up -d
```

Each part matters:

- `stop` first. The running container holds port 3777 and the name the tunnel
  points at, and it would keep answering `/setup` with a redirect to the admin
  panel.
- `--service-ports` publishes port 3777 for the one-off container and
  `--use-aliases` lets the tunnel reach it as `leaf`. Without them the setup
  page can't be reached.
- Ctrl-C, then `up -d`. After you submit, the one-off container carries on as a
  running leaf. Stop it before starting the normal one, or two bots connect
  with the same token.

The form starts **empty**: have all eight values to hand, not only the one you
are changing. Nothing is replaced until a submit passes every check, so
pressing Ctrl-C before that leaves the existing configuration as it was.

(Per-server and per-series settings are a different thing: pick channels in
Discord with `/setup`, and edit limits, timezone, creator role and series
privacy in the admin panel. See [04-usage.md](04-usage.md).)

## Data, backups, and updates

- **Everything persistent is the `leaf-data` volume** (`/data`): your config and
  the SQLite database. Back it up by snapshotting that volume (stop the
  container first for a consistent copy, or use SQLite's online backup). Media is
  stored separately in R2; the `/export` command is an index of day numbers, not
  a backup.
- **A copy of the volume holds your credentials.** `leaf.conf` is in it, with
  the bot token, the client secret and the R2 keys in plain text. Store a
  backup like a password, and keep it out of the folder you cloned leaf into:
  anything left there can end up in a commit or in a Docker build. If a copy
  gets out, reset the bot token and the client secret in the Developer Portal,
  make a new R2 API token, and enter them with
  [`--reconfigure`](#changing-credentials-later---reconfigure).
- **Update** to a new version:
  ```sh
  git pull && docker compose up -d --build
  ```
  This rebuilds the gallery and binary from the working tree and restarts,
  reusing the volume, so your config and database persist.

For a public production deploy with the Cloudflare Tunnel sidecar and the admin
panel, [DEPLOY.md](../DEPLOY.md) is the quick reference; this guide set is the
detailed version.
