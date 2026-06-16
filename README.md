# 🍃 leaf

A peaceful photo/video archive bot and gallery for Discord. Creators run a
**Series** — an ongoing archive of daily posts — and browse it in a gallery
embedded app, without leaving Discord.

- Product & architecture: [PLAN.md](PLAN.md)
- Build plan (20 phases), code standards, guides: [docs/](docs/README.md)

## Run it (Docker)

```sh
docker compose up
# open http://localhost:8080/setup and enter the setup code from the logs
```

First run boots into **setup mode**: a local page collects the bot token,
Discord OAuth pair, public URL, and R2 credentials, validates them live, and
writes `leaf.conf` to the data volume. No env vars to edit, no redeploy.

For a full production deploy — public HTTPS, the Discord app wiring, the Entry
Point launch command, and the web admin panel — see [DEPLOY.md](DEPLOY.md).

## Development

```sh
# toolchain is pinned by rust-toolchain.toml; sqlx-cli generates the dev DB
cargo install sqlx-cli --no-default-features --features sqlite

# create the dev database used by sqlx's compile-time query checking
echo 'DATABASE_URL=sqlite:data/dev.db' > .env
sqlx database create && sqlx migrate run --source migrations

cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

After changing any SQL, refresh the committed offline artifact:

```sh
cargo sqlx prepare --workspace -- --all-targets
```

Quality gates (enforced in CI): rustfmt, clippy `pedantic` at deny, tests,
cargo-deny, sqlx offline freshness, Docker build. See
[docs/rust-guidelines.md](docs/rust-guidelines.md).

## License

BSD-3-Clause.
