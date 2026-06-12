# leaf — Rust Code Quality Guidelines

Strictness is the default; exceptions are explicit, local, and justified.
These rules are enforced by CI from Phase 1 — they are not aspirational.

## Toolchain

- Pinned stable via `rust-toolchain.toml` (with `rustfmt`, `clippy`
  components). Bump deliberately in its own PR.
- Edition 2024.

## Lint configuration (workspace-wide)

Declared once in the root `Cargo.toml`; every crate inherits via
`[lints] workspace = true`.

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"            # public items get doc comments
unused_must_use = "deny"
rust_2018_idioms = { level = "deny", priority = -1 }

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "deny", priority = -1 }
nursery = { level = "warn", priority = -1 }

# The big three — panics are not error handling:
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
indexing_slicing = "deny"

todo = "deny"
dbg_macro = "deny"
print_stdout = "deny"            # tracing, not println
print_stderr = "deny"
allow_attributes_without_reason = "deny"
```

CI runs `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

### Exception protocol

- `#[allow(...)]` must be **as local as possible** (expression/fn, never
  module/crate) and must carry a reason:
  `#[allow(clippy::cast_precision_loss, reason = "day counts fit in f64")]`.
- `unwrap`/`expect`/`panic` are permitted **in tests and build scripts
  only** (scoped via `#[cfg(test)]` module-level allows in a single place,
  not sprinkled).
- A pedantic lint that fights us repeatedly for no value may be demoted to
  `warn`/`allow` **workspace-wide in the root Cargo.toml with a comment**,
  via PR — never silently inline. Expected early candidates:
  `module_name_repetitions`, `must_use_candidate`,
  `missing_errors_doc` (decide once, document the verdict).

## Error handling

- Library crates (`leaf-core`): `thiserror` enums per module. Errors are
  values; variants carry what the caller needs to act. No `anyhow` in
  `leaf-core`'s public API.
- Binary/edge crates (`leaf-bot`, `leaf-server`, `leaf-migrate`):
  `anyhow::Result` at the outermost handler layer is acceptable; convert
  to user-facing messages at the boundary (dialogue service for Discord,
  problem-details JSON for the API).
- **No error is silently dropped.** `let _ = fallible()` requires an
  `#[allow]` with reason. Log-and-continue paths use
  `tracing::warn!/error!` with context fields, not bare `.ok()`.
- A user must never see a `Debug` dump; an operator must always find the
  full chain in logs.

## Async & concurrency

- tokio only; no blocking calls on the runtime — file/image/ffmpeg work
  goes through `spawn_blocking` (thumbnailing, EXIF) or a dedicated
  worker.
- Every spawned task is owned: held `JoinHandle` or registered in a
  `JoinSet`/`TaskTracker`; graceful shutdown awaits them. No
  fire-and-forget `tokio::spawn` without a stated reason.
- Shared state: prefer message passing; otherwise `Arc<RwLock/DashMap>`
  with the lock held for the shortest possible scope — never across an
  `.await` (clippy `await_holding_lock` denies this).
- Timeouts on every outbound network call (Discord REST, R2, OAuth).

## Database (sqlx + SQLite)

- All queries via `sqlx::query!` / `query_as!` (compile-checked). Raw
  `query()` with string SQL requires a reason (e.g. dynamic ORDER BY) and
  a test.
- `sqlx prepare` artifact (`.sqlx/`) committed; CI verifies it's current.
- Multi-statement writes are transactions, no exceptions. Helpers take
  `&mut Transaction` so composition is explicit.
- Migrations are append-only once merged; never edit a shipped migration.
- Repos return domain types, not row tuples; SQL stays in `leaf-core`.

## Logging & observability

- `tracing` exclusively. Structured fields over interpolation:
  `info!(guild_id = %gid, series = %name, day, "archived post")`.
- Spans around units of work (command invocation, archive write, API
  request — tower-http `TraceLayer` for axum).
- **Never log secrets** — tokens, setup codes, R2 keys. The Tier-1 config
  struct's `Debug` impl is manually written to redact.
- Levels: `error` = operator should look; `warn` = degraded but handled;
  `info` = state change worth an audit trail; `debug` = development.

## Dependencies

- Every new dependency is justified in the PR description (what it does,
  why std/tokio/existing deps can't, its own dep footprint).
- `cargo-deny` in CI: advisories (RustSec), license allowlist
  (MIT/Apache-2.0/BSD/ISC/Zlib), duplicate-version warnings.
- Prefer already-present transitive deps over new direct ones.

## Style & structure

- `rustfmt` defaults (committed `rustfmt.toml` even if near-empty); CI
  `--check`.
- Public items documented (`missing_docs` warns); doc comments say *what
  and why*, constraints and invariants — not restating the signature.
- Comments explain what the code cannot: invariants, "why not the obvious
  way", protocol quirks (Discord rate-limit behavior, R2 consistency).
- Module layout mirrors the domain (`series`, `archive`, `media`,
  `settings`), not technical layers-for-their-own-sake.
- No `mod.rs` sprawl: prefer `foo.rs` + `foo/` only when a module has
  true submodules.
- Magic values are named consts with units in the name
  (`SESSION_TTL_SECS`, `MAX_MEDIA_SIZE_MB`).

## Testing (correctness — the suite's job)

- Unit tests live beside the code (`#[cfg(test)]`); integration tests in
  `tests/` per crate.
- Pure logic (parser, streaks, policy, cadence, key layout) is
  **table-driven and exhaustive** — these are the cheapest, most valuable
  tests we have.
- SQLite: real in-memory DB in tests, never mocked.
- R2: `object_store::memory::InMemory` in tests; real-bucket tests are
  `#[ignore]`d and run manually.
- Discord: never called in tests. Discord-touching code is structured so
  the logic is testable behind traits/pure functions; the thin serenity
  glue is covered by manual phase verification.
- Time: injected (`Clock` trait or passed `now`); no test sleeps on the
  wall clock, no timing-based assertions (timing is human-verified, per
  the phase guide's testing philosophy).
- Every fixed correctness bug lands with its regression test in the same
  PR.

## CI gates (every PR)

1. `cargo fmt --workspace --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. `cargo test --workspace`
4. `cargo deny check`
5. sqlx prepare freshness check
6. `docker build` (Dockerfile stays green from Phase 1)
