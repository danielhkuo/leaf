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

It's **re-runnable** and pre-fills from existing values, so it's also how you
change the watched/log channels later. Everything else — timezone, creation
policy, creator role, sprout probation — is edited in the **admin web panel**
(below).

### Sprout probation

If enabled, a new series starts as a 🌱 **sprout**: posts archive normally, but
the series isn't listed publicly in the gallery until it has N archived posts
(default 3). Spam series never sprout and quietly age out — self-moderating, no
admin review needed.

### Revoking a series

Admins hold **revoke** power (not approval power): use the **admin web panel** to
take a series down (it stays stored but becomes hidden and read-only) or restore
it. Default-allow, revoke-able.

### The admin web panel (click instead of type)

Browse to **`https://leaf.example.com/admin`** and **Sign in with Discord**. You
need **Manage Server** on a server that has leaf. This is where admins manage the
server: guild settings (timezone, sprout, limits, creator role) and series
(privacy, revoke/restore). Every field has an inline **ⓘ** that explains what it
does with an example. First-time channel selection still happens in Discord with
`/setup`.

## For creators

Creators use **two surfaces**:

- **Activity gallery** — start a series, change settings, reminders, see your
  series list.
- **Discord chat** — archive posts (context menu), optional passive capture, and
  query/delete/export commands.

Launch the gallery from the **app launcher** or the **Entry Point command** (see
[02-discord.md § Entry Point](02-discord.md#5-entry-point-launch-command-recommended)).

### Start a series — in the gallery

A **Series** is your ongoing archive (PLAN.md § The Series concept). Open the
leaf Activity and choose **Start a series** when the server allows it.

The wizard collects:

1. **Name** and **description**
2. **Channel** — one of the server's watched channels
3. **Cadence** — daily / weekdays / weekly / freeform
4. **Privacy** — public, role-gated (pick a role), or creator-only
5. **Start day** (default 1)

Policy checks run on submit. If one blocks you, the app explains why (watched
channels, required creator role, max series per user, min account or membership
age). There is no separate approval step.

After creation, post in your channel and archive with the context menu (below).
If sprout probation is on, the series stays hidden in the public gallery until
it reaches the configured post count.

### Manage your series — in the gallery

- **Manage my series** — list of series you own (name, emoji, day count, sprout
  state, channel, cadence). Open any series to edit settings.
- **Series settings** (from the list or a ⚙ control on your series calendar) —
  description, reaction emoji, cadence, privacy, channel, passive mode, and
  reminders.
- **Reminders** — enable in settings: time of day (24h `HH:MM`), DM or channel
  ping, optional timezone override. Disabled for **freeform** cadence. The bot
  delivers reminders on schedule; you do not configure them in chat.

Admins revoke series from the **admin web panel**, not from the gallery.

**Privacy** options: **public** to the server, **role-gated** (a role you pick),
or **creator-only**. Privacy is enforced everywhere — gallery and chat commands.

### Archive a post — the 🍃 context menu

The primary capture path. **Right-click (or long-press) any message → Apps →
`🍃 Archive to Series`.**

1. Pick the series (skipped if you have exactly one).
2. A modal opens with the **day number pre-filled** (parsed from the message,
   falling back to the next expected day) — editable.
3. Submit → the post is archived (original + thumbnail copied into R2), the
   message gets the 🍃 reaction, and a line lands in the log channel.

A duplicate day gets a ⚠️ reaction and an ephemeral note, with no write. This is
also the **catch-up** path: right-click an old message, archive it, set the day
in the modal — no separate command needed.

### Passive mode (optional, per series)

For zero-friction capture, turn on **passive mode** in that series' **settings**
in the gallery. leaf then watches the series channel for **your** media posts
and sends **you** an ephemeral "Day 43? [✅] [✏️ Set day] [✗]". Confirm within
10 minutes or it's silently dropped. It's in-memory only — a bot restart during
the window drops the pending post (recover it with the context menu). Off by
default.

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

The gallery is how you **browse** a series and, if you create one, **manage** it
(the bot captures in chat; the embedded app views and configures). Launch it from
the **app launcher** or the **Entry Point command** (see
[02-discord.md § Entry Point](02-discord.md#5-entry-point-launch-command-recommended)) —
prefer these over the voice-channel Activities shelf so no "Game Invitation"
cards appear.

Inside the gallery:

- **Series picker** (skipped if the server has one series; remembers your last
  choice). **Start a series** when you're allowed to create one.
- **Manage my series** — your owned series and settings (creators).
- **Calendar heatmap** — the whole archive at a glance; missing days visible.
- **Day viewer** — tap a day for the full image/video, caption, timestamp, day
  number; prev/next, keyboard arrows, swipe, a random-day jump, and **"jump to
  original message"**.
- **Stats** — current/longest streak, totals, days missed.

Only members who pass the series' privacy rule see it; the server validates
guild membership before serving anything.

→ Migrating an old archive in? See **[05-migration.md](05-migration.md)**.
