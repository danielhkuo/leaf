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
check` and a Docker build are enforced in CI; see
[docs/rust-guidelines.md](../docs/rust-guidelines.md) (local-only) for the
standards.

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

In production leaf-server serves the built `activity/dist` from `STATIC_DIR`; in
dev you run the Vite server and point a tunnel at leaf (below).

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
- Set **`DEV_GUILD_ID`** to your test server's ID so slash commands register
  **instantly** (global registration can take ~1h). In Docker that's an env var;
  from `cargo run` export it in your shell.
- Use a **separate dev R2 bucket** so test media never touches production.

Everything else (creating the dev app, intents, R2 bucket/token) follows
[02-discord.md](02-discord.md) and [03-cloudflare.md](03-cloudflare.md) — just
with dev-scoped resources.

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
| `docs/` | **local-only** (gitignored): PLAN expansion, code standards |
