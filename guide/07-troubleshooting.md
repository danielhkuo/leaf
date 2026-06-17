# 07 — Troubleshooting

Common failures and fixes. (Discord/Cloudflare dashboard layouts drift; see the
note in the [guide index](README.md#dashboards-change) if a menu isn't where a
guide says.)

## Common failures

### Setup page

- **"Setup code invalid/expired."** The code is single-use and printed only to
  the logs. Get the current one with `docker compose logs leaf | grep -i "setup
  code"`; restart the container to mint a fresh one.
- **Bot token rejected on submit.** Wrong/rotated token. Reset it in the portal
  (Bot → Reset Token) and re-enter; it's shown only once.
- **OAuth pair invalid.** The Application ID and Client Secret are from the
  *same* app (OAuth2 tab). A reset secret invalidates the old one.
- **R2 canary failed.** leaf does a put+get+delete to validate. Check the S3
  **endpoint** (`https://<account-id>.r2.cloudflarestorage.com`), the **bucket**
  name, and that the API token is **Object Read & Write** scoped to that bucket.

### Container / volume

- **`EACCES` / can't write `leaf.conf` or the DB.** The data directory must be
  writable by the non-root `leaf` user. The image pre-owns `/data`; if you
  bind-mount a host path instead of the named volume, `chown` it to the
  container's user or use the named volume from `docker-compose.yml`.

### Not reachable / tunnel

- **502 / can't reach `leaf.example.com`.** Confirm the `leaf` container is up
  (`docker compose ps`), the tunnel sidecar is running
  (`docker compose --profile tunnel up -d`), `TUNNEL_TOKEN` is set in `.env`,
  and the tunnel's public hostname targets `http://leaf:3777` (the compose
  service name). The DNS record should be **proxied** (orange cloud).

### Bot online but commands/greeting missing

- **Slash commands don't appear.** Global registration can take up to ~1h. For
  instant registration on a test server, set `DEV_GUILD_ID`
  ([01 § env vars](01-install.md#environment-variables)).
- **No greeting on join.** leaf posts to the system channel, else the top-most
  channel it can speak in. If it can't speak anywhere, it skips silently — give
  it **View Channel** + **Send Messages** and re-invite or run `/setup` directly.
- **"an admin needs to run /setup first."** Expected until an admin completes
  [`/setup`](04-usage.md#first-setup).
- **Passive watcher does nothing.** It needs the **Message Content Intent**
  ([02 § 2](02-discord.md#2-bot-token--the-message-content-intent)) and passive
  mode enabled on the series.

### Gallery / admin panel

- **Blank gallery or CSP errors.** Almost always the **three hosts not matching**
  ([02 § the three hosts](02-discord.md#the-three-hosts-must-match)): the URL
  mapping target, the OAuth redirect, and the Public URL must be the same origin.
  Also confirm `STATIC_DIR` has the built app (it does in the image).
- **Images show a placeholder / 404.** That media is `media_missing` — typically
  an imported day whose original message was gone ([05](05-migration.md#reading-the-gaps-report)).
  Expected, not a bug.
- **Admin login fails.** Add the `…/admin/callback` redirect
  ([02 § 3](02-discord.md#3-oauth2-client-info--redirects)); you need **Manage
  Server** on a server that has leaf.

### Migration

- **`deferred > 0` in the summary.** Transient fetch errors — **re-run** the same
  command; it skips done days and retries deferred ones
  ([05](05-migration.md#running-it-docker)).
- **Many `message_deleted` rows.** Originals are gone; recoverable ones are kept
  as missing placeholders. If unexpectedly high, confirm the bot still has
  **Read Message History** on the source channel(s).
