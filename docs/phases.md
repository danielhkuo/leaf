# leaf — Phase Guide (20 phases)

Expanded from [PLAN.md](../PLAN.md). Each phase is small enough to land as a
reviewable unit and ends with the project in a working, shippable state.
Phases are ordered by dependency; later phases assume everything before them
is merged and green.

## Testing philosophy (applies to every phase)

Two kinds of verification, deliberately separated:

| Kind | Owner | Covers | Mechanism |
| --- | --- | --- | --- |
| **Correctness** | test suites, CI | logic, data integrity, API contracts, regressions | `cargo test`, Vitest + Svelte Testing Library, integration tests; **required to merge** |
| **Experience** | Daniel, by hand | timing, visual feel, animation smoothness, "does it feel right" | per-phase manual checklist below; advisory, not CI-gated |

Rules:

- Automated tests assert **correctness only** — never timing, pixels, or
  aesthetics. No flaky screenshot diffs, no `sleep`-based assertions.
- Every bug found by hand gets a regression test before the fix merges,
  if it is a correctness bug. Feel bugs get an issue, not a test.
- Pure logic (day parser, streak math, policy checks, key layout) is unit
  tested exhaustively. Discord-coupled code is integration tested against
  fakes/mocks; we do not test Discord itself.
- DB tests run against real SQLite (in-memory or temp file), never mocks —
  SQLite is cheap enough to use the real thing.
- R2-coupled code tests against `object_store`'s `InMemory` backend; one
  optional ignored test hits a real dev bucket (`cargo test -- --ignored`).
- CI gates from Phase 1 onward: `cargo fmt --check`, `cargo clippy` (strict,
  see [rust-guidelines.md](rust-guidelines.md)), `cargo test`, and once the
  frontend exists: `prettier --check`, `eslint`, `svelte-check`, `vitest`,
  bundle-size budget.

---

> **Status (2026-06-13, later)**: Phase 13 `/wrapped` added — yearly recap
> embed over pure, tz-aware `wrapped::summarize` (year bucketing, busiest
> month with earliest-tie-break, in-year longest streak; table-tested).
> Personas/dialogue **deliberately deferred** (plan flags them heavy and
> cuttable; a switch that changes nothing is worse than none — its own
> pass). Also hardened a flaky Phase-5 media test (lying-server cap now
> deterministic). 94 tests, all gates green, `/wrapped` registered live.
> **Bot side is now feature-complete for v1; next is Phase 14 — the REST
> API — then the Svelte gallery.**
>
> **Earlier — Status (2026-06-13)**: Phases 1–10 committed; Phases 12–13(part) on top.
> Phase 12 (scheduling): pure `reminder_due` predicate (cadence-aware,
> ISO-week weekly, DST-safe, structural downtime catch-up, at-most-once via
> mark-before-send + rollback), one-minute scheduler tick sharing the
> gateway HTTP client, `/series reminder` config command, and milestone
> announcements (`milestone::classify` — first/years/hundreds — posted in
> the channel on archive). 88 tests, clippy/deny/fmt green. **Phase 11
> (passive watcher) still deferred.** Phase 13 (personas, `/wrapped`) and
> Phase 14+ (API + gallery) remain. Awaiting human verification of a live
> reminder firing and a milestone post.
>
> **Earlier — Status (2026-06-12, evening)**: Phases 1–5 committed and live-verified
> against a real Discord app + R2 bucket. Phases 6–9 implemented and green
> (69 tests): `/setup` (channel-select flow), `/settings` (show/policy/
> timezone), `/series` (create/edit/list/remove with policy + sprout),
> 🍃 Archive-to-Series context menu (modal-first, parser-prefilled day,
> R2 upload + transactional insert), `/search` `/status` `/random`
> `/delete` + 🗑️ context menu, privacy enforced via `policy::can_view`.
> Deviations from the guide, accepted: passive watcher + channel→series
> index deferred to Phase 11 (their consumer); series creation takes one
> channel (multi-channel via later `/series edit`). **Awaiting human
> verification of the full creator loop in the dev guild.**

## Phase 1 — Workspace, toolchain & CI

**Goal**: an empty but rigorously configured monorepo where every later phase
inherits quality gates for free.

**Tasks**
- Cargo workspace: `leaf-core`, `leaf-bot`, `leaf-server`, `leaf-migrate`
  (empty lib/bin skeletons that compile).
- `rust-toolchain.toml` (pin stable), `rustfmt.toml`, workspace
  `[workspace.lints]` per [rust-guidelines.md](rust-guidelines.md) —
  clippy pedantic+nursery, `unsafe_code = "forbid"`, deny warnings.
- `cargo-deny` config (licenses, advisories, duplicate deps).
- GitHub Actions: fmt → clippy → test → deny, on every push/PR.
- Dockerfile (multi-stage, distroless or slim runtime, non-root user) and
  `docker-compose.yml` (leaf + cloudflared sidecar, `/data` volume) — build
  works even though the binary does nothing yet.
- `tracing` + `tracing-subscriber` wired in `main` (env-filter via
  `LOG_LEVEL`).

**Deliverables**: green CI on an empty workspace; `docker compose build` passes.

**Automated tests**: a trivial smoke test per crate (proves harness works).

**Manual verification**: none.

**Exit criteria**: CI red on a deliberately introduced clippy warning
(verify the gate actually bites), then green again.

---

## Phase 2 — Database foundation

**Goal**: the full v1 schema and a typed repository layer everything else
calls. No Discord, no network.

**Tasks**
- sqlx + SQLite pool setup (WAL, foreign keys ON, busy timeout) in
  `leaf-core`.
- Migration `0001`: `guild_settings`, `series`, `posts`,
  `media_attachments`, `personas_active` (active-persona per guild). Posts
  key: `(series_id, day)` composite; `media_attachments` stores
  `attachment_id`, `channel_id`, `message_id`, `content_type`, R2 keys
  (original + thumb), `media_missing` flag.
- Repository structs: `GuildSettingsRepo`, `SeriesRepo`, `PostRepo` — all
  queries via `sqlx::query!`/`query_as!` (compile-checked), all multi-write
  operations transactional.
- Domain types (`Series`, `Post`, `Cadence`, `Privacy`, `SproutState`, …)
  with serde derives where needed.
- Streak/stats logic as pure functions over post rows (current streak,
  longest streak, totals, missing days in range).

**Deliverables**: `leaf-core` with schema, repos, domain types, stats.

**Automated tests**: repo CRUD round-trips on in-memory SQLite; transaction
rollback on mid-write failure; streak math table-driven tests (gaps,
single-day, empty, multi-year); migration applies cleanly twice (idempotent
runner).

**Manual verification**: none.

**Exit criteria**: `sqlx prepare` artifact committed so CI compiles queries
offline.

---

## Phase 3 — Two-state boot + bootstrap setup UI

**Goal**: the foundation UX. Clone → `docker compose up` → browser → paste
credentials → bot connects. No env-var editing, ever.

**Tasks**
- Tier-1 config module in `leaf-core`: load/validate/write `/data/leaf.conf`
  (TOML; restrictive file permissions 0600).
- Boot state machine in the binary: axum **always** starts first; if config
  absent → *setup mode*, else → *run mode*.
- Setup mode: serve one plain HTML/CSS page (no JS framework, embedded in
  the binary via `include_str!`); generate one-time setup code, print to
  logs; form POST validates code then credentials **live**:
  - Discord: REST `GET /users/@me` with the bot token + verify client
    ID/secret via OAuth client-credentials grant.
  - R2: put + get + delete a canary object.
- On success: write config, invalidate code, transition to run mode
  in-process (no restart).
- `--reconfigure` flag: boot into setup mode despite existing config
  (pre-filled except secrets).
- Run mode for now: log "would connect gateway" (gateway lands in Phase 4).

**Deliverables**: the two-state boot, working end to end in Docker.

**Automated tests**: config round-trip + permission bits; state machine
transitions; setup endpoint rejects wrong/expired/reused codes; validation
short-circuits on first failure with a precise error; axum handler tests via
`tower::ServiceExt` (no real Discord/R2 — trait-mocked validators; the live
validators get one ignored integration test each).

**Manual verification (Daniel)**: fresh `docker compose up` on a clean
volume; the setup page is reachable, looks acceptable, validation errors are
human-readable; total time from clone to "configured" feels < 10 min.

**Exit criteria**: deleting `/data/leaf.conf` and restarting reliably
re-enters setup mode; a completed setup survives container restart.

---

## Phase 4 — Gateway connection + command framework

**Goal**: the bot comes alive in run mode; the poise skeleton every command
phase plugs into.

**Tasks**
- serenity + poise client behind the run-mode gate; intents: guilds, guild
  messages, message content (needed for the passive watcher later).
- Command registration strategy: guild-scoped while `DEV_GUILD` knob set,
  global otherwise.
- Unified command error handler (user-facing message + tracing event;
  never a silent failure, never a raw Debug dump to chat).
- `/ping` (returns version + uptime) as the canary command.
- Guild-join handler: post the greeting + "run `/setup`" prompt (system
  channel, fallback to first writable channel).
- Graceful shutdown: SIGTERM → finish in-flight handlers → close pool.

**Deliverables**: bot online, `/ping` answers, greeting on invite.

**Automated tests**: error-handler formatting; greeting channel-selection
logic (pure function over channel list + permissions); shutdown drains pool.

**Manual verification**: invite dev bot, see greeting, run `/ping`;
`docker compose stop` shuts down cleanly within the compose grace period.

**Exit criteria**: bot reconnects on network blips (observe serenity
auto-reconnect in logs by toggling Wi-Fi) without process exit.

---

## Phase 5 — R2 media pipeline

**Goal**: bytes in, originals + thumbnails durably in R2, keys recorded.
The single most load-bearing non-Discord component.

**Tasks**
- `MediaPipeline` in `leaf-core` over `object_store`: configured S3 client
  → R2; `InMemory` in tests.
- Key layout: `g/<guild>/s/<series>/d/<day>/<attachment_id>` (original) and
  `…/thumb/<attachment_id>.webp`.
- Download from a (signed) Discord CDN URL with size cap
  (`MAX_MEDIA_SIZE_MB`, default 100) and content-type allowlist
  (image/*, video/mp4, video/webm).
- Thumbnail generation: `image` crate → WebP, ~256px long edge, EXIF
  orientation respected. Video: poster frame via `ffmpeg` subprocess if
  present on PATH, else a deterministic placeholder thumb (documented
  degradation; ffmpeg ships in the Docker image).
- Upload originals + thumb; idempotent (same key overwrite-safe);
  structured errors distinguishing fetch / transform / store failures.

**Deliverables**: `MediaPipeline::archive(url, meta) -> StoredMedia`.

**Automated tests**: pipeline against `InMemory` store with fixture images
(JPEG/PNG/WebP incl. EXIF-rotated); size-cap rejection; content-type
rejection; thumbnail dimensions + format; key layout snapshot; one ignored
test against a real dev R2 bucket.

**Manual verification**: eyeball a handful of generated thumbnails for
quality (WebP quality setting is a feel decision).

**Exit criteria**: pipeline survives a 50MB video and a 10KB sticker
without pathological memory use (stream, don't buffer whole files).

---

## Phase 6 — Per-guild setup & settings

**Goal**: Moment-2 onboarding — `/setup` and `/settings`, the gate in front
of everything per-guild.

**Tasks**
- `/setup`: guided multi-step flow (poise + component interactions):
  watched channels (multi-select), log channel, timezone (autocomplete over
  IANA list), creation policy with defaults; re-runnable, pre-filled from
  existing rows.
- `/settings channel` and `/settings policy` (the granular editors over the
  same rows).
- Guard middleware: any series/archiving command in an un-setup guild →
  friendly "an admin needs to run /setup first".
- Permission checks: `/setup`, `/settings` require Manage Guild.
- Log-channel writer utility (quiet one-line confirmations; used by all
  later phases).

**Deliverables**: a configured guild as a persisted, queryable state.

**Automated tests**: settings repo round-trips; guard logic (set-up vs not,
admin vs not — table-driven); timezone validation; multi-channel
de-dup/validation.

**Manual verification**: run `/setup` end to end on the dev guild; the flow
order and copy feel right; re-running pre-fills correctly.

**Exit criteria**: an un-setup guild cannot reach any later feature by any
path (checked by the guard test matrix).

---

## Phase 7 — Series management

**Goal**: the core abstraction, self-serve. `/series create` *is* the
application.

**Tasks**
- `/series create`: guided flow (name, description, watched channel(s)
  subset, cadence, privacy, start day, reminder pref, emoji) → policy
  checks (allowed channels, required role, max per user, min account/member
  age) → create; failures name the exact policy that blocked.
- Sprout probation: series starts `sprout` when the guild toggle is on;
  auto-promotes to `active` at N archived posts (checked on archive write).
- `/series edit`, `/series list` (creator's own; admins see all),
  `/series remove @user <name>` (admin, confirm flow).
- Channel→series in-memory index (`DashMap` or `RwLock<HashMap>`) for the
  later watcher; invalidated on series CRUD.

**Deliverables**: full series lifecycle by command.

**Automated tests**: every policy check, individually and combined
(table-driven); sprout promotion exactly at N; index invalidation on
create/edit/remove; name-uniqueness per guild.

**Manual verification**: create a series via the flow; the step count feels
acceptable (< 1 min); error copy when violating each policy reads kindly.

**Exit criteria**: two creators, three series, overlapping channels — index
resolves correctly (integration test).

---

## Phase 8 — Context-menu archiving (the core write path)

**Goal**: the primary feature. Right-click → archive → 🍃.

**Tasks**
- Port the v2 day parser (`high`/`low`/`none` confidence regexes) into
  `leaf-core` as the **suggestion engine**; property + table tests ported
  from v2 cases.
- Context menu `🍃 Archive to Series`: series picker (skipped when the user
  has exactly one), modal with day pre-filled from parser → next-expected
  fallback; editable.
- Archive write: transactional post insert + MediaPipeline upload + 🍃
  reaction + log-channel line. Duplicate day → ⚠️ reaction + ephemeral
  explanation, no write.
- Multi-attachment messages: all attachments under the one day.
- Authorization: only the series creator (or admin) may archive into a
  series.
- Failure handling: R2 failure rolls back the DB write (or records
  `media_missing` per decided policy — decide here, test it).

**Deliverables**: end-to-end archiving in the dev guild.

**Automated tests**: parser suite (exhaustive); modal day-validation;
duplicate rejection; authz matrix (creator/other-creator/admin/random);
transactional rollback on storage failure; multi-attachment fan-out.

**Manual verification**: archive real posts of each media type; the
right-click → done loop feels < 10 s; reactions appear promptly.

**Exit criteria**: an archived post is visible in DB + R2 (original and
thumb) + reaction, atomically — kill the bot mid-archive and verify no
half-state survives restart.

---

## Phase 9 — Query commands

**Goal**: chat-side read access (the gallery's lightweight fallback).

**Tasks**
- `/search series day` — embed with thumbnail, caption, timestamp, jump
  link.
- `/status series [start] [end]` — paginated ✅/❌ coverage audit
  (component-based pagination, ephemeral).
- `/random [series]` — random archived day, same embed as `/search`.
- `/delete day|link` — confirm-button flow; deletes DB rows + R2 objects;
  creator-or-admin only.
- Context menu `🗑️ Remove Archive Entry` — same confirm flow from a
  message.
- Privacy enforcement: role-gated/creator-only series invisible to
  non-entitled callers in **all** of the above.

**Deliverables**: complete chat query suite.

**Automated tests**: pagination boundaries (empty range, single page, exact
multiple); delete authz + full cleanup (DB and R2); privacy matrix across
all four commands; link parsing for `/delete link:`.

**Manual verification**: embeds look right in light & dark Discord themes;
pagination buttons feel responsive.

**Exit criteria**: privacy matrix test passes; deleting then re-archiving
the same day works cleanly.

---

## Phase 10 — Import / export

**Goal**: bulk JSON in/out, byte-format-compatible with walpurgisbot-v2
exports — this is the migration safety net and the backup story.

**Tasks**
- `/export [series]` — JSON attachment (v2-compatible shape for
  single-series; leaf-extended shape for multi); DM delivery with 24MB
  guard, like v2.
- `/import` — accepts both v2 exports (mapped into a chosen/new series) and
  leaf exports; dry-run summary first (counts, collisions), then
  confirm-button to commit; transactional; skipped-duplicate report.
- Serde types for both formats with strict (`deny_unknown_fields`)
  validation and precise error paths.

**Deliverables**: lossless round-trip; v2 files import.

**Automated tests**: round-trip equality (export→import→export, byte-stable
modulo ordering); real v2 fixture file imports correctly; malformed JSON
yields pathed errors; collision policy; transaction atomicity on mid-import
failure.

**Manual verification**: export from the real walpurgis dev DB, import,
spot-check ~5 days.

**Exit criteria**: a v2 production export (copy) imports with zero errors
and matching counts.

---

## Phase 11 — Passive watcher + creator-confirm

**Goal**: the optional zero-friction capture mode, kept deliberately dumb.

**Tasks**
- `message_create` listener: media post in a watched channel by a creator
  with a passive-mode series in that channel → in-memory
  `HashMap<(GuildId, UserId), PendingPost>` entry, 10-min TTL (tokio task,
  no DB).
- Ephemeral prompt to the creator only: "Day {n}? [✅] [✏️ Set day] [✗]"
  (day suggested by parser → next-expected).
- Confirm → same archive write path as Phase 8. Edit → modal. Dismiss/TTL
  expiry → silently drop. Bot restart → pending lost (documented; context
  menu recovers).
- Per-series toggle in `/series edit`; passive mode off by default.

**Deliverables**: opt-in passive capture.

**Automated tests**: watcher routing via the channel→series index (right
creator, right channel, media-only); TTL expiry drops state and leaks
nothing; confirm path equals context-menu path (shared function, asserted);
non-creators and non-watched channels never trigger.

**Manual verification**: post as a passive-mode creator; the ephemeral
appears fast and reads unobtrusively; dismissing leaves no trace.

**Exit criteria**: memory steady under a burst of 100 synthetic pending
posts with mass expiry (no task/entry leak — asserted via metrics counter).

---

## Phase 12 — Scheduling: reminders, catch-up, milestones

**Goal**: the time-based half of the bot, with v2's downtime-resilience
kept.

**Tasks**
- tokio-cron-scheduler + chrono-tz; one reminder job per reminder-enabled
  series (rebuilt on series CRUD).
- Reminder check: cadence-aware "is the series behind?" (daily vs weekdays
  vs weekly vs freeform→never); DM or channel ping per series config;
  at-most-once per missing day (port v2's mark-before-send + rollback-on-
  failure policy).
- Catch-up on boot: missed reminder windows during downtime run once
  (port v2 logic, generalized per-series).
- Milestone announcements on archive write (configurable template, fired at
  sprout-promotion and round-number days).

**Deliverables**: reminders that survive restarts without duplicating.

**Automated tests**: cadence math (all four, around DST transitions —
table-driven with fixed clocks); at-most-once under simulated send failure
(rollback then retry next run); catch-up triggers iff a window was missed;
template rendering.

**Manual verification**: set a reminder 2 min out, watch it fire; restart
the bot across a reminder window and watch catch-up fire exactly once.

**Exit criteria**: clock-injected test suite covers every cadence × DST ×
downtime combination we could enumerate.

---

## Phase 13 — Personas / dialogue + `/wrapped`

**Goal**: the bot's voice, and the yearly recap.

**Tasks**
- Dialogue service: persona TOML files embedded at compile time
  (`include_str!` + parse-once), keyed strings with `{placeholder}`
  interpolation; active persona per guild in DB; fallback chain persona →
  default → key-as-string (never panic on a missing key).
- Ship two personas: `default` (quiet, reflective — leaf's voice) and one
  more for fun; port/adapt v2 string keys that survived the redesign.
- Migrate all user-facing strings from Phases 4–12 onto the dialogue
  service (one sweep; new rule in guidelines: no hardcoded user-facing
  strings).
- `/wrapped [series] [year]` — recap embed: totals, streaks, busiest month,
  first/last post links.

**Deliverables**: persona-switchable voice; `/wrapped`.

**Automated tests**: every persona file parses and covers every key used in
code (a test enumerates used keys via a registry and asserts coverage);
interpolation; fallback chain; wrapped stats against fixture data.

**Manual verification**: switch personas, run a few commands, the voice
lands; `/wrapped` on migrated-style data looks worth screenshotting.

**Exit criteria**: missing-key coverage test makes adding a string without
all-persona coverage a CI failure.

---

## Phase 14 — leaf-server API: auth + REST + media proxy

**Goal**: everything the embedded app will call, finished and contract-
tested before any frontend exists.

**Tasks**
- `POST /api/token`: OAuth code exchange (client secret server-side);
  issue our own short-lived signed session token (HMAC) carrying user ID;
  return it + the Discord access token (per SDK `authenticate` flow).
- Auth middleware: session token → user; resolve + cache guild membership
  (short TTL); reject non-members per guild-scoped route.
- REST (all guild-scoped, privacy-enforced):
  `GET /api/guilds/:gid/series`,
  `GET /api/guilds/:gid/series/:sid/days?from&to` (paginated, thumb keys),
  `GET /api/guilds/:gid/series/:sid/days/:day`,
  `GET /api/guilds/:gid/series/:sid/stats`,
  `GET /api/media/:attachment_id[?thumb]` — stream from R2 with
  `Cache-Control: public, max-age=31536000, immutable`; fallback 302 via
  refreshed Discord CDN URL on missing object.
- Blade gate: static bearer token (Tier-1 config, optional) for read-only
  cross-bot access.
- OpenAPI spec (utoipa) — the contract the frontend codes against.

**Deliverables**: complete, documented, authed API.

**Automated tests**: axum integration tests for every route × (no auth /
bad token / non-member / member / privacy-blocked); token exchange against
a mocked Discord OAuth endpoint; media proxy headers + 302 fallback;
pagination contracts; OpenAPI generation compiles and matches routes
(snapshot).

**Manual verification**: none meaningful pre-frontend (curl spot-checks at
most).

**Exit criteria**: the privacy/authz matrix is exhaustive in tests — this
is the security boundary of the entire product.

---

## Phase 15 — Frontend scaffold + SDK handshake

**Goal**: a Svelte app that boots inside real Discord, authenticates, and
hits the API — with all quality gates in place before features.

**Tasks**
- `activity/`: Vite + Svelte 5 + TypeScript strict; ESLint (flat config,
  typescript-eslint strict + eslint-plugin-svelte) + Prettier +
  `svelte-check`; Vitest + Svelte Testing Library; structure per
  [svelte-guidelines.md](svelte-guidelines.md).
- SDK handshake module: `ready()` → `authorize` → `POST /api/token` →
  `authenticate`; typed API client (generated from the OpenAPI spec or
  hand-written with zod-validated responses).
- Design tokens: CSS custom properties, Discord light/dark via
  `prefers-color-scheme`; base layout shell + loading/error states.
- Bundle budget in CI (fail > 50KB gzipped initial JS, revisit later);
  `vite build` artifact served by leaf-server static handler (embedded via
  `rust-embed` or served from a dist dir — decide and document).
- Dev loop documented: Vite dev server + `cloudflared` quick tunnel + dev
  Discord application.

**Deliverables**: "hello, {user}" rendering inside the Discord client,
through the proxy, authed.

**Automated tests**: handshake module with a mocked SDK (state transitions:
loading → authed → error); API client error paths; CI runs lint + check +
vitest + budget.

**Manual verification**: launch in desktop client and mobile client; cold
load time through the proxy feels acceptable on phone.

**Exit criteria**: the app boots in real Discord on desktop + Android;
bundle budget gate demonstrably fails an oversized build.

---

## Phase 16 — Gallery A: series picker, calendar heatmap, stats

**Goal**: the signature view — the whole archive at a glance.

**Tasks**
- Series picker (skipped for single-series guilds); remembers last choice
  (localStorage).
- Calendar heatmap: CSS-grid month blocks, archived days show thumbnails,
  missing days styled quietly; virtualized so multi-year archives render
  only visible months (`@tanstack/virtual` or hand-rolled
  IntersectionObserver windowing — spike both, pick lighter).
- Thumbnails only in this view (`?thumb`); `loading="lazy"`,
  `decoding="async"`, explicit dimensions.
- Stats panel: current/longest streak, totals, days missed (from
  `/stats`).
- Empty states (no series, empty series, sprout-hidden) designed, not
  defaulted.

**Deliverables**: browsable heatmap for a real migrated-scale dataset.

**Automated tests**: month-grid generation (leap years, partial first/last
months, timezone edges — pure functions); virtualization window math;
component tests for picker + empty states; stats rendering from fixture
API responses (mocked client).

**Manual verification (the big one)**: load a 1000-day synthetic series on
a mid-range Android in real Discord — scroll smoothness, memory behavior,
initial paint; heatmap aesthetics in both themes.

**Exit criteria**: 3-year synthetic series scrolls without jank on the
test phone; DOM node count stays bounded while scrolling (assert via
devtools, record numbers in the PR).

---

## Phase 17 — Gallery B: day viewer + navigation

**Goal**: the contemplative core — viewing one day, moving between days.

**Tasks**
- Day viewer: tap a day → full media (image, or video with poster +
  controls), caption, timestamp, day number; full-res fetched **only**
  here and released on close (revoke object URLs / null sources).
- Prev/next + keyboard arrows + swipe (pointer events); preload exactly
  the two adjacent thumbnails, never adjacent full-res.
- Transitions: Svelte crossfade/View Transitions API — transform/opacity
  only.
- "Jump to original message" via `openExternalLink`; random-day button.
- Multi-attachment days: in-day carousel.

**Deliverables**: complete MVP viewing loop (picker → heatmap → day →
back).

**Automated tests**: navigation reducer (prev/next/random across gaps —
day 5 → day 9 when 6–8 missing); full-res lifecycle (component test
asserting release on unmount); carousel bounds; keyboard handlers.

**Manual verification**: the *feel* phase — transition smoothness, swipe
response, perceived image-load latency on phone; long session (5 min of
browsing) without the webview getting warm/laggy.

**Exit criteria**: browsing 50 days in a session keeps memory flat
(Chrome devtools attached to the Android webview; numbers in the PR).

---

## Phase 18 — Admin web panel + Discord app finalization

**Goal**: the optional click-instead-of-type admin surface, and the
production Discord application wiring.

**Tasks**
- Admin panel routes in the same Svelte app (lazy-loaded chunk, zero cost
  to gallery users): Discord OAuth login (normal browser flow, not the
  SDK), Manage Guild check server-side; edit guild settings + policy;
  list/revoke series — same rows as `/setup`/`/settings`.
- Corresponding authed admin REST endpoints (`PATCH /api/guilds/:gid/…`)
  with the same test rigor as Phase 14.
- Production Discord app: enable embedded app, URL mappings (`/` → public
  origin), `/launch` Entry Point command, OAuth redirect URIs; document
  every dev-portal click (becomes the setup guide's screenshots).
- `--reconfigure` and the panel's "rotate credentials" path reconciled
  (panel can edit Tier-2 only; Tier-1 stays setup-mode/CLI).

**Deliverables**: admin panel live; production app config complete.

**Automated tests**: admin endpoint authz matrix (member-not-admin is the
key case); panel components against mocked API; lazy-chunk boundary
(gallery bundle unchanged — budget gate already enforces).

**Manual verification**: full admin session from a phone browser;
`/launch` opens the gallery in production config.

**Exit criteria**: a non-admin guild member cannot reach any admin
endpoint or panel view (tested), and the gallery initial bundle did not
grow (CI).

---

## Phase 19 — Performance & accessibility hardening

**Goal**: a dedicated pass that treats the PLAN's performance callout and
a11y as exit-gated work, not vibes.

**Tasks**
- Profile on the worst target device (mid-range Android, real Discord):
  cold load, heatmap scroll, day-viewer loop; fix the top findings.
- Idle audit: zero timers/polling when idle (performance panel, 60 s
  trace); kill any `$effect` churn.
- Bundle audit: visualize, dedupe, tighten the budget to measured reality.
- Server side: media proxy under burst load (oha/wrk) — verify streaming
  (no full-file buffering), connection limits, R2 client reuse.
- Accessibility sweep: keyboard-complete navigation, focus management in
  viewer/modals, ARIA on the heatmap grid, alt text strategy (captions as
  alt), prefers-reduced-motion honored (transitions off), axe-core run in
  component tests where wirable.
- SQLite under concurrency: archive write + gallery reads simultaneously
  (WAL working as expected, busy timeouts adequate).

**Deliverables**: recorded baseline numbers (committed to docs/) + fixes.

**Automated tests**: axe checks in component tests; reduced-motion
component behavior; a media-proxy streaming test asserting bounded memory
per request.

**Manual verification**: re-run the Phase 16/17 device checks against the
recorded baselines; subjective "does it feel native" pass.

**Exit criteria**: baseline doc exists with numbers (cold load, scroll
FPS, idle CPU, bundle size, memory after 50-day browse); every number has
a stated budget and is within it.

---

## Phase 20 — `leaf-migrate` + cutover

**Goal**: Johan's archive lives in leaf; walpurgisbot-v2 retires.

**Tasks**
- `leaf-migrate` CLI (clap): `--from` (v2 SQLite or JSON export), `--to`,
  `--guild/--channel/--creator/--series-name/--day-offset`, `--dry-run`,
  `--resume` (checkpoint file), rate-limit-aware Discord fetches.
- Per post: fetch original message, resolve attachment IDs, run the
  **same** MediaPipeline (originals + thumbs to R2); unfetchable → row
  kept, `media_missing`, gaps report (CSV/markdown).
- Finalize setup-guide docs (quickstart, Discord app, Cloudflare,
  local dev, migration runbook — per PLAN's "Setup guides").
- Cutover: dry-run against a production-DB copy → review → real run →
  **shadow week** (leaf live, watching, not reacting; v2 still primary) →
  swap (v2 off, leaf reacts) → keep v2 export as cold backup.

**Deliverables**: migrated production archive; v1.0 tag.

**Automated tests**: migrator mapping against a v2 fixture DB (counts,
day numbers, ordering); checkpoint resume mid-run (kill + resume = same
result as uninterrupted); idempotent re-run; gaps report format; dry-run
writes nothing (asserted).

**Manual verification**: review the dry-run plan + gaps report against
the real data; spot-check ~20 migrated days in the gallery (thumb, full
image, caption, jump link); shadow-week observation.

**Exit criteria**: migrated day count matches v2 (minus documented gaps);
gallery browses the full archive on the phone; one week of shadow
operation with no missed posts vs v2; swap executed.

---

## Phase ordering rationale & parallelism

- 1–5 are strictly sequential (each is the next one's foundation).
- 6–13 (bot features) and 14 (API) can interleave once 5 lands; 14 only
  needs Phases 2 + 5.
- 15–18 (frontend) gate on 14 but not on 9–13.
- 19 gates on 16–18. 20 gates on everything (it reuses the pipeline,
  import shapes, and gallery for verification).
- The only hard date-sensitive risk is **media re-fetch in Phase 20**: it
  depends on the original Discord messages still existing. If timeline
  slips badly, run a minimal "fetch + stash to R2" script early (the
  pipeline from Phase 5 makes this a ~50-line tool) and let Phase 20
  consume the stash.
