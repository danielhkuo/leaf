# leaf — setup & operation guide

Everything you need to stand up leaf, wire it into Discord and Cloudflare, run
it day to day, and migrate an old archive into it.

leaf is **one self-hosted process** — the Discord bot, the REST API, the
embedded-app gallery, and the admin panel all in a single container sharing one
SQLite database. The only external pieces are a **public HTTPS origin** (Discord
requires one for embedded apps) and **Cloudflare R2** for media storage. The
supported deployment is Docker on a box you control + a Cloudflare account; see
[PLAN.md](../PLAN.md) § Hosting for why this is the only supported shape.

## Start here

Read in this order — each step produces credentials the next one needs:

1. **[01-install.md](01-install.md)** — run the container, reach the setup page,
   and learn the config model. (You'll pause here to gather credentials.)
2. **[02-discord.md](02-discord.md)** — create the Discord application; collect
   the **bot token**, **application ID**, and **client secret**; enable the bot
   intent, OAuth redirects, Activities, URL mapping, and the launch command.
3. **[03-cloudflare.md](03-cloudflare.md)** — put leaf on a public hostname
   (Cloudflare Tunnel) and create the **R2 bucket + API keys**.
4. Back to **[01-install.md § First-run setup](01-install.md#first-run-setup)** —
   paste everything into the setup page; leaf validates it and starts.
5. **[04-usage.md](04-usage.md)** — using leaf in Discord: `/setup`, creating a
   Series, archiving posts, and browsing the gallery.

Reference, read when you need them:

- **[05-migration.md](05-migration.md)** — import an old walpurgisbot-v2 archive
  with `leaf-migrate` (the Daily Johan cutover).
- **[06-local-dev.md](06-local-dev.md)** — build, test, and run leaf locally as
  a contributor.
- **[07-troubleshooting.md](07-troubleshooting.md)** — common failures and fixes,
  plus the dashboard steps to re-check when Discord or Cloudflare change.

```
                 ┌─────────────────┐
   02-discord ──▶│  bot token      │──┐
                 │  application ID │  │
                 │  client secret  │  │
                 └─────────────────┘  │     ┌──────────────────────┐
                                      ├────▶│ 01 First-run setup    │──▶ running
                 ┌─────────────────┐  │     │ (paste into /setup)   │
   03-cloudflare─│ public hostname │  │     └──────────────────────┘
                 │ R2 endpoint     │──┘
                 │ R2 bucket+keys  │
                 └─────────────────┘
```

## ⚠️ About the `[VERIFY]` callouts

leaf's own behavior (config fields, commands, env vars, the migrator) is
documented here as **fact** — it comes from the code in this repo.

The **Discord Developer Portal** and the **Cloudflare dashboard**, however,
change their layouts, labels, requirements, and pricing regularly. Wherever this
guide describes one of *their* screens, you'll see a callout like:

> ⚠️ **[VERIFY] Cloudflare UI** — dashboards drift. As of writing the path is
> *Zero Trust → Networks → Connectors*; confirm the current location/labels.

Treat every `[VERIFY]` as "true when written; double-check it against the live
dashboard." The flagged steps live in guides 02 (Discord), 03 (Cloudflare), 04
(usage), and 06 (local dev); [07-troubleshooting.md](07-troubleshooting.md#what-to-verify-in-current-dashboards)
lists them in one place.

> 📸 Screenshots aren't included. The
> fiddly steps carry a `screenshot` HTML comment marking where you may want to
> add your own.
