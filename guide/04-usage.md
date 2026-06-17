# 04 — Using leaf in Discord

With the bot running and invited, here's the day-to-day: configuring a server,
running a Series, archiving posts, and browsing the gallery. Three roles —
**admin**, **creator**, **viewer** — overlap freely (you can be all three).

## For server admins

### First: `/setup`

When leaf joins, it posts a greeting asking an admin to run **`/setup`**. Until
that's done, leaf refuses series creation in the server with a friendly message.
`/setup` is a guided flow (requires **Manage Server**) covering:

1. **Watched channel(s)** — where series are allowed to post (one or many).
2. **Log channel** — quiet one-line confirmation of each archived post.
3. **Timezone** (IANA) + **creation policy** (or accept the defaults).
4. Optionally a **creator role** that gates who may create series.

It's **re-runnable** and pre-fills from existing values, so it doubles as the
editor. Granular edits are also available via **`/settings`**:

- `/settings channel` — change the log/watched channels.
- `/settings policy` — series-creation policy: allowed channels, required role,
  max series per user, minimum account age, and **sprout probation** on/off.

### Sprout probation

If enabled, a new series starts as a 🌱 **sprout**: posts archive normally, but
the series isn't listed publicly in the gallery until it has N archived posts
(default 3). Spam series never sprout and quietly age out — self-moderating, no
admin review needed.

### Revoking a series

Admins hold **revoke** power (not approval power): `/series remove @user <name>`
takes a series down (confirm flow). Default-allow, revoke-able.

### The admin web panel (click instead of type)

Browse to **`https://leaf.example.com/admin`** and **Sign in with Discord**. You
need **Manage Server** on a server that has leaf. The panel mirrors
`/setup`/`/settings` and `/series` — edit guild settings (timezone, sprout,
limits, creator role) and manage series (privacy, revoke/restore) from a browser.
Same database rows as the commands; strictly optional.

## For creators

### Create a Series — `/series create`

A **Series** is your ongoing archive (PLAN.md § The Series concept). **`/series
create` is the application** — the guided flow *is* the form, auto-approved
against the server policy:

1. Name, description, watched channel(s) (a subset of the server's allowed set).
2. Cadence (daily / weekdays / weekly / freeform), privacy, start day.
3. Reminder preference and reaction emoji (default 🍃).

Policy checks run on submit; if one blocks you, the bot names exactly which
(allowed channels, required role, max-per-user, min account/member age). Manage
your series with `/series edit`, `/series list`, and (admins) `/series remove`.

**Privacy** options: **public** to the server, **role-gated** (a role you pick),
or **creator-only**. Privacy is enforced everywhere — commands *and* gallery.

### Archive a post — the 🍃 context menu

The primary capture path. **Right-click (or long-press) any message → Apps →
`🍃 Archive to Series`.**

> ⚠️ **[VERIFY] context-menu location** — message context-menu commands live
> under the **Apps** submenu on right-click / long-press; confirm the current
> path on your client.

1. Pick the series (skipped if you have exactly one).
2. A modal opens with the **day number pre-filled** (parsed from the message,
   falling back to the next expected day) — editable.
3. Submit → the post is archived (original + thumbnail copied into R2), the
   message gets the 🍃 reaction, and a line lands in the log channel.

A duplicate day gets a ⚠️ reaction and an ephemeral note, with no write. This is
also the **catch-up** path: right-click an old message, archive it, set the day
in the modal — no separate command needed.

### Passive mode (optional, per series)

For zero-friction capture, enable passive mode on a series (`/series edit`). leaf
then watches that series' channel(s) for **your** media posts and sends **you**
an ephemeral "Day 43? [✅] [✏️ Set day] [✗]". Confirm within 10 minutes or it's
silently dropped. It's in-memory only — a bot restart during the window drops
the pending post (recover it with the context menu). Off by default.

### Query & manage from chat

| Command | What it does |
| --- | --- |
| `/search series:<name> day:<n>` | Show one day's embed (thumbnail, caption, timestamp, jump link). |
| `/status series:<name> [start] [end]` | Paginated ✅/❌ coverage audit. |
| `/random [series]` | A random archived day. |
| `/delete day:<n>` or `/delete link:<url>` | Remove an entry (confirm flow; deletes DB rows + R2 objects). |
| `🗑️ Remove Archive Entry` (context menu) | Same delete, from a message. |
| `/wrapped [series] [year]` | Yearly recap: totals, streaks, busiest month, first/last links. |
| `/export [series]` / `/import` | Bulk JSON in/out (walpurgisbot-v2-compatible). |

Delete and the privacy rules are creator-or-admin only; role-gated / creator-only
series stay invisible to non-entitled callers in **every** command above.

## For viewers — the gallery

The gallery is the **only** way to browse a series (the bot captures; the
embedded app views). Launch it from the **app launcher** or the **Entry Point
command** (see [02-discord.md § Entry Point](02-discord.md#5-entry-point-launch-command-recommended)) —
prefer these over the voice-channel Activities shelf so no "Game Invitation"
cards appear.

> ⚠️ **[VERIFY] launching an Activity** — where the app launcher lives differs by
> client (desktop vs mobile) and Discord changes it; confirm how to start an
> Activity on your client.

Inside the gallery:

- **Series picker** (skipped if the server has one series; remembers your last
  choice).
- **Calendar heatmap** — the whole archive at a glance; missing days visible.
- **Day viewer** — tap a day for the full image/video, caption, timestamp, day
  number; prev/next, keyboard arrows, swipe, a random-day jump, and **"jump to
  original message"**.
- **Stats** — current/longest streak, totals, days missed.

Only members who pass the series' privacy rule see it; the server validates
guild membership before serving anything.

→ Migrating an old archive in? See **[05-migration.md](05-migration.md)**.
