# Deploying leaf

> **Quick reference.** For the detailed, step-by-step version (Discord and
> Cloudflare dashboards, in-Discord usage, migration, troubleshooting) see the
> **[setup guide](guide/README.md)**.

leaf is **one self-hosted process** (bot + REST API + gallery + admin panel),
plus a way to put it on a public HTTPS origin (Discord requires one for the
gallery). This page covers production: running it, exposing it, first-run
setup, the Discord Developer Portal settings, and the admin panel. For local
iteration see [activity/README.md](activity/README.md).

> Throughout, replace `leaf.example.com` with your own public hostname.

## 1. Run the container

```sh
docker compose up -d            # leaf on :3777
docker compose logs -f leaf     # watch startup / grab the first-run setup code
```

The image builds the gallery and serves it; app and API are one origin. Nothing
about your Discord application is built into the image.

No environment variables are needed. leaf registers its commands globally by
itself; `DEV_GUILD_ID` is a development setting and stays unset
([details](guide/01-install.md#command-registration-and-dev_guild_id)).

## 2. Expose it on HTTPS

Pick one. Both put leaf behind Cloudflare; keep the DNS record **proxied
(orange cloud ON)**.

- **Cloudflare Tunnel (bundled sidecar).** Create a tunnel in Cloudflare's
  Zero Trust dashboard
  ([guide/03 § 2](guide/03-cloudflare.md#2-publish-leaf-with-a-cloudflare-tunnel)
  has the menu path), add a public hostname routing
  `leaf.example.com` → `http://leaf:3777`, copy the connector **token** into a
  `.env` next to `docker-compose.yml` as `TUNNEL_TOKEN=…`, then:
  ```sh
  docker compose --profile tunnel up -d
  ```
  No port-forwarding required.

- **Your own reverse proxy** (e.g. nginx proxy manager). Add a proxy host
  `leaf.example.com` → `http://127.0.0.1:3777`, enable SSL (Let's Encrypt),
  Force SSL, and HTTP/2. Point a Cloudflare DNS record at it (orange-cloud on).
  No special headers needed; just don't force `X-Frame-Options: DENY` on this
  host.

Cloudflare does not cache leaf's media by default (the URLs have no file
extension). Add the cache rule from
[guide/03 § 4](guide/03-cloudflare.md#4-cache-media-at-cloudflares-edge-recommended).

## 3. Discord Developer Portal

At <https://discord.com/developers/applications> → your app
([details](guide/02-discord.md)):

- **Bot** → reset the token (used in setup). Leave the privileged gateway
  intents off; leaf doesn't use them.
- **OAuth2 → Redirects** → add **both**:
  - `https://leaf.example.com` (the gallery's sign-in)
  - `https://leaf.example.com/admin/callback` (the admin panel's sign-in)
- **Installation** → keep only **Guild Install** ticked; scopes `bot` +
  `applications.commands`; permissions View Channels, Send Messages, Embed
  Links, Attach Files, Add Reactions, Read Message History.
- **Activities → Settings** → enable Activities, and under **Supported
  Platforms** tick **iOS and Android** (off by default; unticked, the gallery
  doesn't exist on phones). Choose a default orientation lock.
- **Activities → URL Mappings** → add **Prefix** `/` → **Target**
  `leaf.example.com` (host only, no scheme).
- Leave the **Entry Point command** Discord created (named Launch) as it is.
  It is what lists leaf in the app launcher. Opening the gallery through it in
  a text channel posts a message with a Join button there.
- **Invite** the bot to your server with the install link, before or after
  step 4.

The **URL Mapping target**, the **OAuth redirects**, and the **Public URL** must
all name the same origin, or sign-in fails.

In the server, members need **Use Application Commands** and **Use Activities**
(and, by other developers' reports, **Send Messages** in the channel they open
the gallery from).

## 4. First-run setup

With no config, leaf boots into **setup mode** and prints a one-time code.
Open `https://leaf.example.com/setup`, enter the code, and provide:

| Field | Where it comes from |
| --- | --- |
| Application ID, Client Secret, Bot Token | Discord Developer Portal (step 3) |
| R2 endpoint, bucket, access keys | Cloudflare → R2 |
| **Public URL** | `https://leaf.example.com` (your origin, no path) |

Saving checks everything live and switches leaf to run mode. The success page
says when the bot is online, or why it isn't; later, the same state is at
`https://leaf.example.com/api/status` and in `docker compose logs leaf`
(`gateway connected`, then `commands registered`). Then run **`/setup`** in
your server; leaf's greeting there asks for the same.

To change these values later:

```sh
docker compose stop leaf
docker compose run --rm --service-ports --use-aliases leaf --reconfigure
# complete /setup in the browser, then Ctrl-C here
docker compose up -d
```

## 5. Admin panel

Browse to `https://leaf.example.com/admin` and **Sign in with Discord**. You
need **Manage Server** in a server leaf is in. From there you edit the server's
settings (timezone, creator role, log channel, limits, the sprout stage) and
manage series (who can see them, revoke / restore, publish a sprout). Series
channels are picked in Discord with `/setup`.

## Updating

```sh
git pull && docker compose up -d --build
```

This rebuilds the gallery and the binary from the working tree and restarts,
reusing the `leaf-data` volume (your config and database persist).
