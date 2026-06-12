# leaf

A peaceful, configurable photo/video archive bot and gallery for Discord.

**Philosophy**: nostalgic, passive, yours. The bot is unobtrusive. The gallery
is the product. Every day you add one thing; years later you scroll back and
see who you were.

> **Terminology**: the gallery is a Discord **Embedded App** (built with the
> Embedded App SDK). Discord's user-facing name for an Embedded App is an
> "Activity" — the thing you launch from the app launcher or a voice channel
> that opens in an iframe. Throughout this doc, **"the embedded app"** =
> **"the gallery"** = what Discord's UI calls an Activity. They are one and
> the same; we just call it the embedded app to avoid confusion.

**Partner bot**: blade (separate product — community engagement, competitive
mechanics, loud; integrates with leaf optionally and one-way).

---

## What leaf is

leaf lets creators run a **Series** — a named, ongoing archive of daily (or
regular) posts — inside a Discord server. Anyone can apply. Each creator owns
their series. **The gallery lives entirely inside the Discord embedded app**:
rather than the bot posting image embeds into chat, you open leaf's embedded
app and browse a real gallery — calendar, day viewer, streaks — without ever
leaving Discord. The bot handles capture; the embedded app handles viewing.

leaf is a real product. It should be useful to any server, not just ours.
"Our server's inside jokes" go in blade, not leaf.

---

## The Series concept

Everything in leaf centers on a **Series**. A server can have many; a creator
can have many. This is the core abstraction.

```
Series
  ├── creator (Discord user)
  ├── name + description + cover image
  ├── channel(s) to watch for posts
  ├── posting cadence (daily / weekdays / weekly / freeform)
  ├── detection mode (context menu only | passive + creator-confirm)
  ├── privacy (public to server | role-gated | creator-only)
  ├── start date + start day number
  ├── reminder (on/off, time, timezone, DM vs channel ping)
  ├── milestone text template  e.g. "Day {day} — {creator} keeps going 🍃"
  └── reaction emoji (default 🍃, configurable)
```

---

## Multi-creator onboarding: self-serve, no admin in the loop

There is no application review. **`/series create` is the application** — the
guided setup flow (name, channel, cadence, reminder preference) doubles as the
form, and the bot auto-approves against passive server policy:

- allowed channels (where series may watch)
- optional required role
- max series per user
- minimum account age / server membership age

Pass the checks → series exists immediately. Fail → bot explains which policy
blocked it. Admins hold **revoke** power (`/series remove @user <name>`)
instead of approve power: default-allow, revoke-able.

**Sprout probation (optional, per-server toggle):** a new series starts as a
🌱 *sprout* — posts archive normally, but the series isn't listed publicly in
the gallery until it has N archived posts (default 3). Spam series never
sprout and silently age out. Self-moderating, zero admin involvement, and the
sprout → leaf metaphor is on-brand.

---

## Archive detection: context menu first

The v2 session state machine (passive watching → confidence parsing → admin
escalation) does not scale to multiple creators and is difficult to maintain.
leaf replaces it with **context menu archiving** as the primary path.

**Primary: context menu**
1. Creator posts their image/media however they want.
2. Right-click the message → `🍃 Archive to Series`.
3. Bot shows a modal: series picker (if multiple), day number (pre-filled with
   next expected day, editable).
4. Creator submits → archived → 🍃 reaction on original message.

Zero session state. Zero timers. Zero false positives. Zero admin escalation
for normal flow. The creator owns the action.

**Secondary: passive + creator-confirm (optional per series)**
For creators who want zero friction:
1. Bot watches the series' configured channel(s) for the creator's media posts.
2. On detection, sends an **ephemeral** to the creator only: "Day 43? [✅ Yes]
   [✏️ Set day] [✗ Not a post]"
3. Creator confirms within 10 minutes or it's silently dropped (no drama, no
   admin ping — use the context menu to recover).
4. In-memory session only: `HashMap<UserId, PendingPost>` with a 10-min TTL.
   No DB session table. Bot restart during the window = post dropped, context
   menu recovers it. Acceptable trade for the complexity eliminated.

Day number parsing (the v2 high/low confidence regexes) is kept as the
**suggestion engine** for the day field pre-fill. It no longer gates archiving.

**Catch-up posts** (something from last week, a missed day) use the same
context menu — right-click any message, archive it, edit the day number in
the modal. There is no separate `/manual-archive` command: the context menu
already *is* the manual path, works on any message you can see in any channel,
and a redundant message-ID command would only duplicate it.

---

## Configuration: two tiers, deliberately separated

The hard lesson from Docker bots: **runtime config must not be env vars.**
Changing "which channel does this series watch" should never mean editing a
compose file and recreating the container. So config splits into two tiers
with different storage and different lifecycles.

### Tier 1 — Bootstrap secrets (rarely change; needed before anything runs)

The credentials the process needs just to connect and serve. Stored in a
persisted file on the data volume (`/data/leaf.conf`), **not** baked into the
image, and **not** required as env vars (see First-run setup below).

```
DISCORD_TOKEN, CLIENT_ID, CLIENT_SECRET   # bot + OAuth
R2_ENDPOINT, R2_BUCKET, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY
PUBLIC_URL                                # the embedded-app origin (tunnel domain)
```

Only truly machine-level knobs stay as optional env vars: `DATA_DIR`,
`BIND_ADDR`/`PORT`, `LOG_LEVEL`.

### Tier 2 — Runtime config (changes often; lives in the DB)

Everything an admin or creator tunes during normal operation. Stored in
SQLite, edited via slash commands **or** the admin web panel (both write the
same rows). No redeploy, ever.

```
Server-level (admins):
  watched channels        — one OR MANY channels series may post in
  log channel             — quiet confirmation of each archived post
  creator role (optional) — gate who may run /series create
  default timezone
  creation policy         — max series/user, min account age, sprout on/off

Series-level (creators, within server bounds):
  all Series fields (name, channels, cadence, privacy, reminder, emoji, ...)
```

Note: **multiple watched channels** is first-class. A server can let series
post across several channels (e.g. `#art-daily`, `#sketches`); the server
admin whitelists the set, each series picks which of them it watches.

---

## First-run setup

Two onboarding moments, because the bootstrap secrets and the per-server
config have a chicken-and-egg relationship: you cannot Discord-OAuth into a
web panel before the bot that powers OAuth is even running.

### Moment 1 — Owner bootstrap (self-hoster, once per deployment)

On first boot with no `/data/leaf.conf`, leaf starts in **setup mode**: the
bot does *not* connect to the gateway yet; leaf-server serves a single local
setup page and prints a one-time **setup code** to the container logs.

```
$ docker compose up
leaf  | No configuration found. Starting setup.
leaf  | → Open http://localhost:8080/setup
leaf  | → Setup code: 7QF2-IDT4   (also shown: docker logs leaf)
```

The page collects: bot token, client ID/secret, R2 credentials, public URL.
leaf **validates them live** (test gateway login, test an R2 put/get,
confirm the OAuth pair) before writing `/data/leaf.conf`, then boots normally
into the gateway. The setup code (not Discord auth, since the bot isn't up
yet) is what protects this page; it's single-use and expires on success.

This is the "minimal UI just to enter tokens" — no editing env vars, no
redeploy. Re-running setup later (to rotate a token) is `/data/leaf.conf` +
a `leaf --reconfigure` flag, or the admin panel once running.

### Moment 2 — Per-server config (server admin, once per guild)

When the bot is invited to a guild, it has nothing configured for that
server. It posts a short greeting in the system channel (or first writable
channel) prompting the admin to run **`/setup`**, a guided flow:

1. Pick watched channel(s).
2. Pick the log channel.
3. Set timezone + creation policy (or accept defaults).
4. Optionally set a creator role.

Until `/setup` completes, leaf refuses series creation in that guild with a
clear "an admin needs to run /setup first" message. `/setup` is re-runnable
and is just a friendly front-end over the same `/settings` rows.

**The admin web panel** (served by leaf-server, authed via Discord OAuth +
a Manage Guild permission check) mirrors `/setup` and `/settings` for admins
who'd rather click than type. Same DB rows; strictly optional. Unlike Moment
1, this *can* use Discord OAuth because the bot is now running and in the
guild.

---

## Command suite

**Creator commands:**
- `/series create` — guided self-serve series creation (this is the "application")
- `/series edit` — modify series config
- `/series list` — your series in this server
- `/delete day:<n>` or `/delete link:<url>` — remove an entry (confirm flow)
- `/search series:<name> day:<n>` — retrieve a specific day's embed
- `/status series:<name> [start] [end]` — paginated coverage audit
- `/random [series]` — retrieve a random day
- Context menu: `🍃 Archive to Series`

**Admin commands:**
- `/setup` — guided per-guild first-run config (watched channels, log channel,
  timezone, policy); re-runnable; required once before series can be created
- `/settings channel` — set log/notification channel and watched channel(s)
- `/settings policy` — series creation policy (allowed channels, required role,
  max per user, min account age, sprout probation on/off)
- `/series remove @user <name>` — revoke a series (confirm flow)
- `/import` / `/export` — bulk JSON, format-compatible with walpurgisbot-v2 export
- Context menu: `🗑️ Remove Archive Entry`

---

## The gallery (embedded app)

The embedded app is the primary — and only — way to browse a series: the
full photo/video gallery lives here, not in chat embeds. Launched from the
app launcher or via `/launch` (Entry Point command). (This is what Discord's
UI labels an "Activity"; see the Terminology note up top.)

**MVP (v1):**
- Series picker on entry (if server has multiple)
- Calendar heatmap — whole archive at a glance, missing days visible
- Day viewer — click a day, see the full image, caption, timestamp
- Prev / next navigation, swipe on mobile
- "Jump to original message" button (`openExternalLink`)
- Stats panel: current streak, longest streak, total days, days missed
- Random day button

**Gallery v2 (backlog):**
- **The Vine** — alternate timeline view where posts are leaves on a growing
  vine; lush stretches = long streaks, bare branches = gaps
- **Evolution mode** — side-by-side or scrub comparison of any two days
- **On this day** — surfaced automatically: "2 years ago, you posted this"
- **Synced viewing** — multiplayer mode; everyone in voice sees the same day,
  anyone can flip (watch-party for the archive)
- **Guess the Day** — show an image, participants guess the day number; closest
  wins; trivially multiplayer via instance participants API
- **Share a moment** — `openShareMomentDialog` to post a day's image into chat
- **Memory capsule** — creator leaves a note revealed after N days of posting
- **Timelapse export** — downloadable gif/video of all posts in sequence
- **Seasons** — gallery aesthetic shifts with real-world seasons

---

## Architecture

Cargo workspace, two binaries: `leaf` (bot + server in one process, one
SQLite pool) and `leaf-migrate` (one-shot CLI):

```
leaf/
├── crates/
│   ├── leaf/          # composition root: the `leaf` binary, two-state boot
│   ├── leaf-core/     # domain: Series, repositories, day parser, streak logic, R2 media client
│   ├── leaf-bot/      # serenity + poise: events, commands, session map, cron
│   ├── leaf-server/   # axum: setup UI, OAuth token exchange, REST API, media proxy
│   └── leaf-migrate/  # one-shot CLI: walpurgisbot-v2 → leaf migration
├── activity/          # Svelte 5 + Vite + TS, @discord/embedded-app-sdk
├── migrations/        # sqlx migrations
├── Dockerfile
└── docker-compose.yml
```

**Runtime**: tokio. One process. leaf-server (axum) always starts first; the
gateway/bot task only spawns once Tier-1 config exists (setup mode vs run
mode, see First-run setup). Both tasks share the same `Arc<SqlitePool>`. This
ordering is deliberate — the server must serve the bootstrap page *before*
any credentials exist, so it can't depend on the bot being up.

**DB**: SQLite via sqlx (async, compile-checked queries, migrations). Schema:
`series`, `posts`, `media_attachments`, `notification_settings`, `personas`,
`dialogue`. Drops `archive_sessions` (no more DB-backed session state).

**Commands**: poise on top of serenity (reduces command boilerplate; still
"the serenity framework").

**Scheduling**: tokio-cron-scheduler + chrono-tz. Reminder cron with
downtime catch-up logic (ported from v2). Milestone check on archive write.

### Embedded app stack

Discord runs the app as an iframe and is **framework-agnostic** — anything
that builds to HTML/CSS/JS works. Hard constraints: it must (a) be a web app
over HTTPS through Discord's proxy, (b) use `@discord/embedded-app-sdk` (npm,
JS/TS — so the frontend can't be Rust/WASM; the SDK has no Rust binding),
(c) respect Discord's CSP (external fetches go through URL mappings). Within
that, optimize for lightweight + visually appealing.

**Chosen stack: Svelte 5 + Vite + TypeScript.**

- **Svelte** compiles to tiny vanilla JS with no virtual-DOM runtime tax —
  the smallest, fastest bundle of the appealing options. Bundle size matters:
  mobile users load this in an iframe over a phone connection.
- Its **built-in transitions/motion** primitives are the reason it wins for a
  *gallery* specifically — crossfades, slides, and spring animations come free
  and make the day-to-day browsing feel premium without an animation library.
- **Scoped `<style>` blocks** mean styling needs zero CSS framework. A small
  set of CSS custom properties (design tokens) themed to match Discord's
  light/dark mode (`prefers-color-scheme` + the SDK locale/theme) gives a
  native feel with no dependency weight.
- First-class TypeScript; shares types with nothing on the Rust side but keeps
  the API client honest.

**Runner-up: SolidJS + Vite** — equally tiny, fastest runtime, React-like JSX;
loses to Svelte only on built-in animation ergonomics. **Avoid React** here:
heaviest of the lightweight options, and we gain nothing from its ecosystem
for a focused single-purpose gallery.

**Dependency discipline**: every dep is CSP + audit + bundle surface. Prefer
platform APIs over libraries — the **View Transitions API** for slick
day-to-day crossfades, **CSS grid** for the calendar heatmap, **Intl** for
date/locale formatting, pointer events for swipe. Pull a library only when a
platform API genuinely won't do (e.g. list virtualization for a multi-year
heatmap, via a framework-agnostic core like `@tanstack/virtual`).

> **⚠️ Performance is a first-class requirement, not a nice-to-have.** Discord
> embedded apps run in a sandboxed iframe inside an already heavy Electron/
> mobile client and are *notorious* for being sluggish. On low-end phones the
> webview is fighting Discord itself for CPU and RAM. We design for that
> hostile environment from the start:
>
> - **RAM budget is the hard constraint.** A gallery loads images — the fast
>   way to OOM a mobile webview. Never hold full-resolution images in memory:
>   serve **thumbnails** for the heatmap/grid (small WebP/AVIF, generated
>   server-side at archive time and cached in R2), fetch full-res only for the
>   one day actively open, and **release it on navigate away**. Virtualize any
>   long list so only on-screen nodes exist in the DOM.
> - **Responsiveness target**: interactions feel instant (<100 ms), animations
>   hold 60fps. Use CSS transforms/opacity only (GPU-composited, no layout
>   thrash); avoid animating width/height/top/left. Debounce scroll/resize.
> - **Idle cost near zero.** No polling loops, no perpetual timers, no
>   reactive recomputation when nothing changed. Svelte's compiled reactivity
>   helps here; don't undo it with careless `$effect`/store churn.
> - **Ship less JS.** Lazy-load non-MVP views (evolution mode, minigames)
>   behind dynamic `import()` so first paint is just the gallery. Set a CI
>   **bundle-size budget** and fail the build if the initial chunk exceeds it.
> - **Images do the heavy lifting, so**: `loading="lazy"`,
>   `decoding="async"`, explicit width/height to avoid reflow, modern formats,
>   responsive `srcset` sized to the device. An `IntersectionObserver` frees
>   off-screen image elements during long scrolls.
> - **Measure on the worst device, not your desktop.** Profile in the actual
>   Discord mobile client on a mid-range Android; desktop numbers lie.

**Build & serve**: Vite builds `activity/` → static assets; in production
leaf-server serves them from the same origin as `/api` (one URL mapping).
Dev uses Vite's dev server behind a `cloudflared` quick tunnel.

### Media storage (critical — v2 has a latent bug here)

Discord attachment URLs are signed and expire (~24h) since 2024. v2 stores
raw URLs in `media_attachments` — an archive viewer would show broken images.

**Topology clarification:** the Activity frontend is static JS served through
Discord's proxy — it has no storage. Media lives wherever **leaf-server** can
reach it.

**Decision: Cloudflare R2 is the only supported storage.** One strategy, no
matrix. Rationale:

- The free tier dwarfs our workload: 10GB storage, 1M writes/month (we write
  ~1 object/day), 10M reads/month (~330k image loads/day), free egress.
- Production hosting already requires a Cloudflare account for the Tunnel
  (see Hosting), so R2 adds **zero new accounts**. leaf's stack is one
  opinionated story: Docker + one Cloudflare account + one domain.
- Off-box durability for free: the archive survives the home server dying.
- Decouples media from topology: if bot and server ever split, media is
  already a non-issue (only the DB would need solving).

Pricing honesty: "free egress" means bandwidth. Reads (`GetObject`) are
Class B operations, billed $0.36/M *after* the 10M/month free tier. We stay
free by caching: archive images are immutable, so leaf-server serves them
with `Cache-Control: immutable, max-age=31536000`, and R2 behind Cloudflare's
CDN doesn't bill cache hits as Class B ops at all.

Implementation: the `object_store` crate's S3 client pointed at the R2
endpoint. (Technically any S3-compatible endpoint works via the same env
vars — but R2 is the only configuration we document, test, and support.)

Archive write path: store `attachment_id` + `channel_id` + `message_id`, copy
the original file into R2 at archive time (it's an archive — if the message is
deleted, the image must survive), **and generate a small thumbnail** (WebP/
AVIF, e.g. ~256px long edge) stored alongside it as `…/thumb/<id>`. The
thumbnail is what the gallery grid/heatmap load — non-negotiable for the
mobile RAM budget (see the performance note). For video, grab a poster frame.
Generated once on write, never on read.

Serving path: `leaf-server GET /api/media/:attachment_id` (+ `?thumb` variant)
- Found in R2 → stream it with immutable cache headers (satisfies Activity
  CSP via the `/api/*` proxy mapping; the frontend never sees a raw Discord
  CDN URL).
- Missing → refresh signed URL via Discord API (cached to expiry), 302.
- Later optimization: register the R2 public-bucket custom domain in the
  Activity URL mappings and serve direct from R2, offloading image bandwidth
  from leaf-server entirely.

### Serving through Discord's proxy: the caching architecture

Important for the gallery frontend (Phases 14–16). Discord sandboxes
Activities and routes **all** asset traffic through its proxy
(`discordsays.com`); CSP blocks direct `<img>` links to any external
domain. The request chain we design for:

```
iframe <img src="/media/...">         (relative path, CSP-safe)
  → Discord Proxy (URL mapping: /media → our Cloudflare-proxied domain)
    → Cloudflare edge cache  ← cache HIT ends here: $0, R2 untouched
      → R2 origin            ← only on MISS: 1 Class B op
```

Discord's proxy respects cache headers on non-HTML assets, so the whole
chain stays warm. Rules we commit to:

1. **URL mappings, not absolute URLs**: the frontend always requests
   relative paths; mappings route `/` (app+api) and—if we later serve
   media direct from the bucket—`/media` to the R2 custom domain.
2. **`Cache-Control: public, max-age=31536000, immutable`** on every media
   response (leaf-server sets it; a Cloudflare Cache Rule enforces it if
   serving direct from R2).
3. **Enable Tiered Cache** in Cloudflare (one toggle): regional caches
   check the upper tier before hitting R2, so one global archive view
   costs ~1 Class B op instead of one per region.
4. **Cache busting is structural, not manual**: object keys embed the
   Discord attachment id (`…/d/<day>/<attachment_id>`), and archived media
   is immutable by product design — content never changes under a key.
   Replacing a day = delete + re-archive = new attachment id = new key.
   No versioning strings needed, ever.
5. **Expectation setting**: edge caches evict cold objects, so a
   low-traffic archive still produces a trickle of Class B ops on rarely
   viewed days. With 10M free ops/month this is noise, but it is why the
   bill is "essentially zero" rather than literally zero.

### Activity data flow

```
User launches leaf Activity
  → discordSdk.ready()
  → authorize({ scope: ["identify", "guilds"] })
  → POST /api/token  (code exchange, client secret server-side)
  → authenticate(access_token)
  → GET /api/series         (list series in this guild)
  → GET /api/series/:id/days
  → GET /api/media/:id      (proxied, never raw Discord CDN URLs)
```

Server validates guild membership before serving anything.

### Migration: `leaf-migrate` (walpurgisbot-v2 → leaf)

A one-shot CLI binary, separate from the bot (runs once, shouldn't live in
bot code). More than a JSON import because **the old DB's media URLs are
already expired** — migration must re-fetch every archived message from
Discord *while those messages still exist*.

```
leaf-migrate \
  --from walpurgis.db            # or the v2 JSON export
  --to leaf.db
  --guild <id> --channel <id>
  --creator <johan_user_id>
  --series-name "..."            # the migrated series
  [--day-offset 0] [--dry-run] [--resume]
```

1. Creates the Series row; maps v2 `posts` → leaf posts under it.
2. Per post: fetch the original message via Discord API (bot token), resolve
   real attachment IDs, download media into R2.
3. Deleted/unfetchable message → post row kept, flagged `media_missing`,
   written to a gaps report for manual follow-up.
4. Resumable via checkpoint file (1000+ rate-limited API fetches), idempotent,
   `--dry-run` prints the plan without writing.

Generalizes beyond Johan: anyone migrating an old archive channel into a
series can use it — fits the "leaf is a real product" stance.

### Hosting: one supported strategy

Discord does **not** host Activities. The iframe loads from
`<app-id>.discordsays.com`, which is Discord's *proxy* — the URL mappings in
the dev portal tell it where to fetch your content, and that target must be a
publicly reachable HTTPS URL you control. So a public origin is mandatory.
With bot + server bundled in one process, that's the only new requirement vs.
walpurgisbot-v2.

**The supported deployment** (everything else is unsupported):

```
Home server
└── docker compose
    ├── leaf            # one process: bot (gateway, outbound-only)
    │                   #              + leaf-server (frontend + /api)
    │                   # shared SQLite on a volume
    └── cloudflared     # Cloudflare Tunnel sidecar → public HTTPS
Cloudflare account (free)
    ├── Tunnel          # leaf.yourdomain.xyz → home box; no port forwarding,
    │                   # home IP never exposed, TLS handled by Cloudflare
    └── R2 bucket       # all media (see Media storage)
Domain (~$10/yr)        # the entire infrastructure bill
Discord dev portal
    └── URL mapping: /  →  leaf.yourdomain.xyz
```

- **Bot**: gateway connection is outbound-only — no ports, no public IP.
- **Server + Activity**: leaf-server serves the static frontend and `/api`
  from the same origin; one URL mapping covers everything.
- **Development**: no domain needed — `cloudflared tunnel --url
  http://localhost:3000` gives a free throwaway `trycloudflare.com` URL
  (this is what Discord's own Activities tutorial uses); paste it into the
  URL mapping of a dev application.
- **DB**: bot and server share one SQLite pool in one process. This is why
  the bundle is the only supported shape — SQLite can't be shared across
  machines. If leaf ever goes multi-tenant hosted, that's the point to
  revisit (server owns DB + bot talks HTTP, or LiteFS/Postgres) — not v1,
  and with media already in R2, the DB would be the *only* thing to solve.

---

## Phases

> **Execution detail lives in [docs/phases.md](docs/phases.md)** — a
> 20-phase expansion of the outline below, with per-phase tasks, required
> automated tests, manual verification checklists, and exit criteria.
> Code standards: [docs/rust-guidelines.md](docs/rust-guidelines.md) and
> [docs/svelte-guidelines.md](docs/svelte-guidelines.md). The 5 macro-phases
> below remain the conceptual map; the 20-phase guide is what we build from.

**Phase 0 — Scaffolding + bootstrap setup UI**
- Cargo workspace, poise skeleton, sqlx + migrations, tracing logging,
  Dockerfile. Env vars only for `DATA_DIR`/`PORT`/`LOG_LEVEL`.
- **Two-state boot, built now** (this is the foundation everything sits on):
  - *Setup mode* — when `/data/leaf.conf` is absent, leaf-server comes up
    serving a single static bootstrap page; it prints a one-time **setup
    code** to the logs. The page collects token / client secret / R2 creds /
    public URL, leaf **validates them live** (gateway login, R2 put+get, OAuth
    pair), writes `/data/leaf.conf`, and transitions to run mode. Gateway is
    NOT connected in this state.
  - *Run mode* — config present → connect the gateway, start the bot. (Bot
    features come in later phases; Phase 0 just proves the boot transition.)
- The bootstrap page is plain HTML/CSS served by axum (no Svelte build needed
  for it — it must work before any app bundle exists and with zero secrets).
- Outcome: clone, `docker compose up`, open the URL, paste credentials, and
  the bot connects — no env-var editing, no redeploy. Exactly the onboarding
  you need from day one.

**Phase 1 — Series & archiving**
- Series model in DB + `/series create` self-serve flow with policy checks
- `/setup` per-guild flow; gate series creation on it; watched channel(s) model
- Context menu: `🍃 Archive to Series` with modal (this is the manual path too)
- Day parser (v2 regexes, same logic) as suggestion engine only
- R2 media storage (`object_store` S3 client); archive write: posts +
  media_attachments + original copy into R2 + thumbnail generation (WebP/AVIF,
  video poster frame) stored alongside
- `/search`, `/status` (paginated), `/delete` (confirm)
- `/import` / `/export` (format-compatible with walpurgisbot-v2)
- 🍃 reaction on archive, ⚠️ on duplicate
- Passive watcher + creator-confirm (optional per series, in-memory sessions)

**Phase 2 — Services**
- `/settings` (creation policy, log channel, timezone), sprout probation
- Reminder cron + downtime catch-up
- Milestone announcements (configurable text template)
- `/random`, `/wrapped` (yearly recap embed)
- Personas / dialogue (TOML-embedded strings per persona, DB stores active name)

**Phase 3 — Embedded app (gallery)**
- leaf-server (already exists from Phase 0): add OAuth token exchange endpoint
  and the series/days/stats/media REST API alongside the existing setup server
- Media proxy endpoint (`/api/media/:id`) + URL mapping registration
- Gallery frontend MVP (Svelte): series picker, calendar heatmap, day viewer,
  prev/next, stats panel, random day
- Admin web panel (Discord OAuth + Manage Guild) mirroring `/setup`/`/settings`
  for runtime config — distinct from Phase 0's setup-code-gated bootstrap page
- Discord app config: embedded app enabled, URL mappings, Entry Point command

**Phase 4 — Cutover & docs**
- Write the setup guides (see "Setup guides" section)
- Build `leaf-migrate`; dry-run against a copy of the production walpurgis DB
- Run migration: series created, posts mapped, media re-fetched into the
  R2 while the original messages still exist; review the gaps report
- Shadow mode (leaf watches but doesn't react) for one week alongside v2
- Swap

---

## Setup guides (documentation deliverables)

leaf is a product, so deployment must be reproducible by someone who isn't us.
Written alongside Phase 4, versioned in `docs/`:

1. **Quickstart** — the golden (and only) path: docker-compose up on a home
   box, then the **setup-mode flow** (browser → setup code → enter token + R2
   creds → validated → running), Tunnel sidecar. No env-var editing. Target:
   working bot + gallery in under 30 minutes.
2. **Discord application setup** — create the app, bot token + intents,
   enable Activities, URL mapping, Entry Point command, OAuth redirect URI.
   The fiddliest part for newcomers; screenshots.
3. **Cloudflare guide** (one account, two features) — domain onboarding,
   Tunnel via `cloudflared` compose sidecar, R2 bucket + scoped API token,
   cache rule so Class B reads stay in the free tier. Includes the honest
   cost model (free egress ≠ free reads; why we stay at $0 anyway).
4. **Local development** — `cloudflared` quick tunnels (no domain needed),
   separate dev Discord application, dev R2 bucket.
5. **Migration runbook** — `leaf-migrate` end-to-end: export from
   walpurgisbot-v2, dry-run, real run, reading the gaps report, verification
   checklist before shadow-mode cutover.

---

## Personas / dialogue

Kept from v2, simplified. Dialogue strings ship as embedded TOML files per
persona (not 400 rows of seed SQL). DB only stores the active persona name per
server. Adding a new persona = add a TOML file + redeploy. No migration needed.

Default persona: quiet, reflective, slightly literary (matches leaf's vibe).
Additional personas: configurable by server admins.

---

## blade integration

blade can optionally read from leaf's REST API to power competitive features
(leaderboards, streaks as competition, milestone shoutouts). leaf does not know
blade exists. The API has an optional auth token gate for this use case.

---

## What we are explicitly not building

- **Multi-guild bot hosting / SaaS dashboard** — self-host or run your own
  instance. Managed hosting is a future concern, not v1.
- **Web gallery outside Discord** — Discord-only for now.
- **AI captioning / semantic search** — adds ML dependency to a reliability-
  first archiver. Revisit if lightweight enough (possibly blade's territory).
- **Sharding / Postgres** — SQLite is fine for this scale indefinitely.
- **Video archiving beyond Discord attachments** — images + short clips only.
- **Per-message engagement (likes, comments)** — that's blade's lane.

---

## Open questions

1. ~~Hosted vs. self-hosted Activity~~ — **resolved**: self-hosted only, one
   supported strategy (home Docker + Cloudflare Tunnel + R2). Managed hosting
   is a non-goal until/unless leaf goes multi-tenant.

2. **Passive watcher scope** — watch all series channels with one
   `message_create` listener and look up series by channel, or register
   per-series listeners? Latter is cleaner but serenity doesn't work that way;
   former requires a channel→series index in memory.

3. **Media download quota** — one image/day is cheap. Video files could be
   large. Cap per-file size? Configurable `MAX_MEDIA_SIZE_MB`?

4. **Collaborative series** — multiple creators contributing to one shared
   gallery (group sketchbook, etc.). Interesting feature, scope TBD.
