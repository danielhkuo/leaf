# 06 — Local development

For working on leaf itself. The production path is Docker
([01-install.md](01-install.md)); this is the from-source loop.

## Toolchain

The Rust toolchain is pinned by `rust-toolchain.toml` (rustup installs the right
version automatically). The frontend needs Node 22+ (matching the Dockerfile's
`node:22`).

## Backend: database + build

`sqlx` checks queries at **compile time** against a real database schema, so you
need a dev DB before building:

```sh
cargo install sqlx-cli --no-default-features --features sqlite

# DATABASE_URL points sqlx at the dev DB (also read from .env)
echo 'DATABASE_URL=sqlite:data/dev.db' > .env
sqlx database create
sqlx migrate run --source migrations

cargo build
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

CI runs with `SQLX_OFFLINE=true` against the committed `.sqlx/` cache (so it
needs no database). **After changing any SQL** (`sqlx::query!` macros), refresh
and commit that cache:

```sh
cargo sqlx prepare --workspace -- --all-targets
```

`.sqlx/` is tracked like a lockfile. The quality gates above plus `cargo deny
check` and a Docker build are enforced in CI. The coding standards live in
`docs/rust-guidelines.md` and `docs/svelte-guidelines.md`, which are local-only
files (the `docs/` folder is gitignored), so a fresh clone doesn't have them.

## Frontend: the gallery

The Svelte app lives in `activity/` and has its own tooling and dev loop —
see **[activity/README.md](../activity/README.md)**. In short:

```sh
cd activity
npm install
npm run dev          # Vite dev server
npm test             # Vitest
npm run check        # svelte-check
```

In production leaf-server serves the built `activity/dist` from `STATIC_DIR`. In
dev, either point the tunnel at the Vite dev server (hot reload; the steps are
in activity/README.md) or run `npm run build` and point it at leaf itself, as
below. `http://localhost:5173/mock.html` shows every screen with fixture data
and needs neither Discord nor a tunnel.

## Running against real Discord locally

Discord embedded apps must load over HTTPS through Discord's proxy, so even local
dev needs a public origin. Use a **throwaway quick tunnel** and a **separate dev
Discord application**:

```sh
# expose your local leaf (default :3777) on a temporary public URL
cloudflared tunnel --url http://localhost:3777
```

Then, in a **dev** Discord application (don't reuse production):

- Set its **URL mapping** target and **OAuth redirects** to the
  `*.trycloudflare.com` host the command prints (it changes each run).
- Use that same host as the **Public URL** in leaf's setup.
- Optionally set **`DEV_GUILD_ID`** to your test server's ID (the dev bot must
  be a member). Commands are then registered in that server only, which keeps
  them apart from the application's global list while you change them. Unset,
  they are registered globally, as in production
  ([01 § Command registration](01-install.md#command-registration-and-dev_guild_id)).
  In Docker it's an env var; with `cargo run`, export it in your shell.
- Use a **separate dev R2 bucket** so test media never touches production.

Everything else (creating the dev app, R2 bucket/token) follows
[02-discord.md](02-discord.md) and [03-cloudflare.md](03-cloudflare.md) — just
with dev-scoped resources.

## Real data stays out of the clone

A local run writes `data/leaf.conf` (the bot token, the client secret and the
R2 keys, in plain text) and `data/leaf.db`. `.gitignore` skips `data/`,
`leaf.conf`, `*.db`, `.env`, `*.tgz` and `*.tar.gz` archives, and
`walpurgis-export-*.json` / `leaf-export-*.json` files, so none of them is
committed by accident. `.dockerignore` keeps the same things at the top of the
repo out of an image build. Both lists are a backstop, not a place to keep
things: put volume backups and export files somewhere outside the clone, and
check `git status` before a commit. A credential that was pushed or shared has
to be replaced
([01 § Data, backups, and updates](01-install.md#data-backups-and-updates)).

## Where things live

| Path | What |
| --- | --- |
| `crates/leaf` | composition root: the `leaf` binary, two-state boot |
| `crates/leaf-core` | domain, DB repos, config, media pipeline, parsers |
| `crates/leaf-bot` | serenity/poise: events, commands, reminders |
| `crates/leaf-server` | axum: setup UI, REST API, media proxy, admin panel |
| `crates/leaf-migrate` | the migration CLI ([05-migration.md](05-migration.md)) |
| `activity/` | the Svelte 5 gallery |
| `migrations/` | sqlx migrations |
| `docs/` | **local-only** (gitignored, absent from a clone): code standards, phase plan, design specs such as `docs/design/app-series-management.md`. `PLAN.md` at the repo root is local-only too. |
