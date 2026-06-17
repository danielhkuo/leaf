# Deploying leaf

> **Quick reference.** For the detailed, step-by-step version — Discord +
> Cloudflare dashboards, in-Discord usage, migration, troubleshooting — see the
> **[setup guide](guide/README.md)**.

leaf is **one self-hosted process** (bot + REST API + gallery + admin panel),
plus a way to put it on a public HTTPS origin (Discord activities require
one). This guide covers production: exposing it, first-run setup, the Discord
Developer Portal wiring, the **Entry Point launch command**, and the admin
panel. For local iteration see [activity/README.md](activity/README.md).

> Throughout, replace `leaf.example.com` with your own public hostname.

## 1. Run the container

```sh
docker compose up -d            # leaf on :3777
docker compose logs -f leaf     # watch startup / grab the first-run setup code
```

The image builds the gallery and serves it; app and API are one origin.

## 2. Expose it on HTTPS

Pick one (both put leaf behind Cloudflare, which is where signed media gets
edge-cached — keep the record **proxied / orange-cloud ON**):

- **Cloudflare Tunnel (bundled sidecar).** Create a tunnel in Cloudflare →
  Zero Trust → Networks → Tunnels, add a public hostname routing
  `leaf.example.com` → `http://leaf:3777`, copy the connector **token** into a
  `.env` next to `docker-compose.yml` as `TUNNEL_TOKEN=…`, then:
  ```sh
  docker compose --profile tunnel up -d
  ```
  No port-forwarding required.

- **Your own reverse proxy** (e.g. nginx proxy manager). Add a proxy host
  `leaf.example.com` → `http://127.0.0.1:3777`, enable SSL (Let's Encrypt),
  Force SSL, and HTTP/2. Point a Cloudflare DNS record at it (orange-cloud on).
  No special headers needed — just don't force `X-Frame-Options: DENY` on this
  host.

## 3. First-run setup

With no config, leaf boots into **setup mode** and prints a one-time code.
Open `https://leaf.example.com/setup`, enter the code, and provide:

| Field | Where it comes from |
| --- | --- |
| Application ID, Client Secret, Bot Token | Discord Developer Portal (step 4) |
| R2 bucket + access keys | Cloudflare → R2 |
| **Public URL** | `https://leaf.example.com` (your origin) |

Saving validates everything and switches leaf to run mode. To change Tier-1
values later, run the container once with `--reconfigure`.

## 4. Discord Developer Portal

At <https://discord.com/developers/applications> → your app:

- **Bot** → reset the token (used in setup) and enable **Message Content
  Intent**.
- **OAuth2 → Redirects** → add **both**:
  - `https://leaf.example.com` — the gallery's token exchange
  - `https://leaf.example.com/admin/callback` — the admin panel login
- **Activities → Settings** → enable Activities (and the platforms you want).
- **Activities → URL Mappings** → add **Prefix** `/` → **Target**
  `leaf.example.com` (host only, no scheme). This is what lets Discord's
  `discordsays.com` proxy fetch your origin.
- **Install** the app to your server: **OAuth2 → URL Generator**, scopes
  `bot` + `applications.commands`.

The **URL Mapping target**, the **OAuth redirect**, and the **Public URL** must
all name the same origin, or auth fails.

## 5. Entry Point launch (retire the "Game Invitation" cards)

Discord Activities launched from a **voice channel** post "Game Invitation /
Game ended" cards in chat. leaf's gallery is a solo viewing experience, so the
better entry is an **Entry Point command**: when you enable Activities, Discord
provides a default launch command whose handler is *"let Discord launch the
activity"* (no bot code). Launch the gallery from that command / the app
launcher rather than the voice-channel Activities shelf, and the invite cards
don't appear. (You can rename the default Entry Point command in the portal;
leave its handler set to Discord-handled.)

## 6. Admin panel

Browse to `https://leaf.example.com/admin` and **Sign in with Discord**. You
need **Manage Server** on a server that has leaf. From there you can edit guild
settings (timezone, sprout probation, limits, creator role) and manage series
(privacy, revoke / restore). Everything here is also available via the bot's
`/settings` and `/series` commands — the panel is the click-don't-type option.

## Updating

```sh
git pull && docker compose up -d --build
```

This rebuilds the gallery and the binary from the working tree and restarts,
reusing the `leaf-data` volume (your config and database persist).
