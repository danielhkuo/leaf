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
dev, either point a tunnel at the Vite dev server (hot reload; the steps are
in activity/README.md) or let `scripts/dev-up` build the gallery and serve it
from leaf itself
([below](#running-against-real-discord-locally)).
`http://localhost:5173/mock.html` shows every screen with fixture data and
needs neither Discord nor a tunnel.

## Tests

| Where | Command | What it runs |
| --- | --- | --- |
| repo root | `cargo test --workspace` | The Rust tests. Loopback only: no Discord, no R2 |
| `activity/` | `npm run test:run` | Vitest: components, stores and logic, in jsdom |
| `activity/` | `npm run e2e:mock` | Playwright on the mock screens: layout, tap targets, accessibility, flows |
| `activity/` | `npm run e2e:app` | Playwright on the built gallery against the real API, in a stand-in Discord |
| `activity/` | `npm run e2e` | Both Playwright halves, plus the screenshot comparison |
| repo root | `scripts/tests/run` | The dev scripts ([below](#running-against-real-discord-locally)), against stand-ins for leaf, cloudflared, the emulator, npm and cargo |

CI runs all of them except the last (`.github/workflows/ci.yml`; it saves the
screenshots instead of comparing them). The Playwright suites need
their browsers once: `npx playwright install chromium webkit` in `activity/`
(on Linux add `--with-deps`). `scripts/tests/run` needs `python3` and `curl`,
takes about two minutes, and starts nothing real: run it after changing
anything in `scripts/`.

`npm run e2e:app` is the one test that needs both halves of the repo. It
builds the gallery, starts leaf-server's `e2e_server` example and opens the
gallery inside a page that plays the Discord client, so the SDK handshake, the
sign-in, the API and the media proxy all run for real. It also asks the API
for every response the gallery and the admin panel parse and compares field
names with the client's schemas, so a field renamed on one side fails here
before it goes missing from a screen:

```sh
cargo run -p leaf-server --example e2e_server   # what Playwright starts, on :3811 and :3812
```

The example is the real router over a fresh SQLite database with a fixed seed
(two servers, six people, six series, 150 days of posts), an in-memory object
store and a stub in place of Discord's API. It listens on 127.0.0.1 only and
nothing in it reaches another machine. Its doc comment
(`crates/leaf-server/examples/e2e_server/main.rs`) lists the seed and the
`/__e2e/…` routes a test uses to reset it, archive a day, stall Discord or
mint a session. The first run compiles leaf-server; a Rust change is picked up
by the next run. What the suites check, and how to write one, is in
[activity/README.md](../activity/README.md#testing).

The same server is a quick way to see the admin panel with data in it and no
Discord application: build the gallery (`npm run build` in `activity/`), start
the example from the repo root (it serves `activity/dist` on :3799 by
default), and open `http://127.0.0.1:3799/__e2e/admin/login?persona=admin`.

## Running against real Discord locally

Discord loads the gallery over HTTPS through its own proxy, so even a local
leaf needs a public address. The setup below is done once; after it, three
scripts bring the whole environment up and down:

| Command | What it does |
| --- | --- |
| `scripts/dev-up` | Builds the gallery and the `leaf` binary, starts leaf on `127.0.0.1`, starts the tunnel, and waits until each one answers |
| `scripts/dev-status` | Says what is running, on which port and address, where the logs are, whether the bot is connected, and which command to run next |
| `scripts/dev-down` | Stops what `dev-up` started, and nothing else |

They need `bash` (the 3.2 that macOS ships is enough), `curl`, and what the
build needs: Node and the Rust toolchain ([Toolchain](#toolchain)), with the
gallery's packages installed once (`npm --prefix activity install`).

### One-time setup

**1. A dev Discord application.** Make a second application for development;
don't reuse the production one. Follow [02-discord.md](02-discord.md) with the
hostname you choose in step 2: both OAuth **redirects**
(`https://leaf-local.example.com` and
`https://leaf-local.example.com/admin/callback`), the Activities **URL
mapping** (`/` → `leaf-local.example.com`), and iOS and Android ticked under
Supported Platforms. Invite its bot to a test server.

**2. A named tunnel on your own domain.** A named tunnel keeps one hostname for
good, so the redirects and the URL mapping are entered once. With
`cloudflared` installed (`brew install cloudflared`, or your system's package)
and signed in to the domain's zone (`cloudflared tunnel login`, once per
machine), two commands create it:

```sh
cloudflared tunnel create leaf-local                              # prints the tunnel's ID and writes ~/.cloudflared/<ID>.json
cloudflared tunnel route dns leaf-local leaf-local.example.com    # the DNS record for the hostname
```

Then describe it in a config file of its own, `~/.cloudflared/leaf-local.yml`,
so it stays apart from any other tunnel on the machine:

```yaml
tunnel: leaf-local
credentials-file: /Users/you/.cloudflared/<ID>.json
ingress:
  - hostname: leaf-local.example.com
    service: http://127.0.0.1:3777
  - service: http_status:404
```

> No domain? `cloudflared tunnel --url http://127.0.0.1:3777` prints a
> throwaway `https://….trycloudflare.com` address. It changes on every run, and
> the redirects, the URL mapping and leaf's Public URL have to change with it,
> so `dev-up` does not manage that kind: run `scripts/dev-up --no-tunnel` and
> the quick tunnel beside it.

**3. `scripts/dev.env`.** Copy the example and name the tunnel in it:

```sh
cp scripts/dev.env.example scripts/dev.env
```

```sh
LEAF_DEV_TUNNEL_CONFIG=~/.cloudflared/leaf-local.yml
LEAF_DEV_TUNNEL_NAME=leaf-local
LEAF_DEV_PUBLIC_URL=https://leaf-local.example.com
```

`scripts/dev.env` is git-ignored, and
[`scripts/dev.env.example`](../scripts/dev.env.example) documents every
setting: the port, the data directory, the log level, `DEV_GUILD_ID`, the
emulator, and how long each wait lasts. No credential goes in it. Without the
file, `dev-up` starts leaf on `127.0.0.1:3777` with `./data` and no tunnel.

`DEV_GUILD_ID` in that file is optional. Set to your test server's ID (the dev
bot must be a member), it registers the commands in that server only, which
keeps them apart from the application's global list while you change them.
Unset, they are registered globally, as in production
([01 § Command registration](01-install.md#command-registration-and-dev_guild_id)).

**4. The setup page, once.** Start everything:

```console
$ scripts/dev-up
gallery   ok       built into activity/dist
binary    ok       built target/debug/leaf
leaf      ok       setup mode on http://127.0.0.1:3777 (pid 41213)
                   setup code: ABCD-EFGH
                   open http://localhost:3777/setup and enter it to finish setup
tunnel    ok       https://leaf-local.example.com/setup answers (pid 41240)

Up. scripts/dev-status shows this again; scripts/dev-down stops it.
```

Open the setup page, enter the code, and fill the form as in
[01 § First-run setup](01-install.md#first-run-setup): the dev application's
bot token, application ID and client secret, and the tunnel's address as the
**Public URL**. leaf checks everything, writes `data/leaf.conf` and switches to
run mode in the same process, so there is nothing to restart. From then on
`dev-up` finds the configuration and starts leaf in run mode.

For storage, choose **A folder on this machine** under **Media storage**. The
**Folder path** field then fills in with the suggested folder, `media` inside
the data directory (`<data dir>/media`, by its full path); keep it. That needs
no bucket and no keys, and the test media stays in the git-ignored data
directory. R2 works too, with a separate dev bucket
([03-cloudflare.md](03-cloudflare.md)), so test media never lands in
production.

### Day to day

```sh
scripts/dev-up        # build, start leaf and the tunnel, wait for each
scripts/dev-status    # what is running, and the next command
scripts/dev-down      # stop the tunnel and leaf
```

Every step prints one line: `ok`, `skip` or `failed`, and what happened. A
failed step is followed by the last lines of its log, and `dev-up` exits with
status 1.

| Option | Effect |
| --- | --- |
| `scripts/dev-up --no-build` | Start what is already built |
| `scripts/dev-up --no-tunnel` | Start leaf only; it is reachable on this machine and nowhere else |
| `scripts/dev-up --port N` | Listen on `127.0.0.1:N` for this run (the tunnel's config has to send to the same port) |
| `scripts/dev-up --emulator` | Also boot the Android emulator named by `LEAF_DEV_AVD` and wait until Android has finished booting |
| `scripts/dev-down --emulator` | Also stop that emulator; without the option it is left running, because booting it is the slow part. It gets up to two minutes to save its snapshot (`LEAF_DEV_WAIT_STOP_EMULATOR`) before it is killed |

What to expect from them:

- **`dev-up` is safe to run again.** Whatever it started earlier is reported as
  already running and left alone. To pick up a Rust change, run
  `scripts/dev-down` and then `scripts/dev-up`. A gallery change needs no
  restart: `npm --prefix activity run build`, then reopen the gallery.
- **They only stop what they started.** `dev-up` records each process's pid,
  and `dev-down` signals a pid only while its command line is still the one
  `dev-up` started. A port held by anything else is an error that names the
  process (`port 3777 is held by leaf (pid 55924)`), never a kill, whether it
  was there before `dev-up` ran or took the port during the build. A
  `cloudflared` or an emulator you started by hand for the same tunnel or
  device is used as it is and left running, even when it does not answer.
- **What they start outlives the terminal.** Each process gets a process group
  of its own and ignores the hang-up signal, so Ctrl-C and closing the window
  do not reach it. Ctrl-C during `dev-up` stops `dev-up` only: what had
  started keeps running, and `scripts/dev-down` stops it.
- **The binary is looked for where cargo builds.** `dev-up` asks cargo for its
  target directory, so `CARGO_TARGET_DIR` or a `target-dir` in a cargo config
  file is followed.
- **Logs and pid files are in `.dev-run/`**, which is git-ignored:
  `leaf.log`, `tunnel.log` and `emulator.log` (the log of the run before is
  kept as `leaf.log.1` and so on), and `build-gallery.log` and `build-leaf.log`
  from the build. `scripts/dev-status` prints the paths.
- **In run mode the bot's state is reported too.** `dev-up` waits for
  `/api/status` to leave `starting`. A `gateway ok` line means the bot is
  connected to Discord; anything else is a failed step with leaf's own
  sentence about why.
- **A setting can be overridden for one run** from the shell:
  `LEAF_DEV_PORT=3999 scripts/dev-up`. For a second environment beside the
  first, write another settings file with its own `LEAF_DEV_PORT`,
  `LEAF_DEV_DATA_DIR` and `LEAF_DEV_RUN_DIR`, and name it to all three
  scripts: `LEAF_DEV_ENV_FILE=path scripts/dev-up`. A file named that way has
  to exist; a misspelt path is an error, not a fall back to the first
  environment.
- **They don't handle credentials.** The scripts never open `leaf.conf`, and
  of the environment they print only their own settings (the port, the paths
  and the addresses above). The one thing they repeat from leaf's log is the
  setup code, which works once.

### Check it: `leaf doctor`

`scripts/dev-status` ends with the command to run next. Once leaf is in run
mode that is the doctor:

```sh
cargo run -p leaf -- doctor --url
```

It checks the configuration, the bot token and client secret, the registered
commands and the Entry Point command, the gateway, a write, read and delete in
the storage, and the database, and then asks the Public URL for `/healthz`,
`/api/status`, the gallery and a stored file, the way Discord reaches it. It
starts neither the server nor the bot and prints no credential, but it is
live: it talks to Discord and to the storage. The line `dev-status` prints
already carries `DATA_DIR` and `DEV_GUILD_ID` when yours differ from the
defaults, and names `http://127.0.0.1:3777` after `--url` when no tunnel
answers. Every check and option is in
[07 § Start here](07-troubleshooting.md#start-here-leaf-doctor).

### On a phone: the Android emulator

The iOS Simulator cannot install Discord, so a phone check on one machine
means the Android emulator. Create a virtual device with the Play Store on it,
install Discord in it and sign in once, then set `LEAF_DEV_AVD` (and
`ANDROID_HOME`, the SDK directory that holds `emulator/` and
`platform-tools/`) in `scripts/dev.env`. `scripts/dev-up --emulator` boots it,
waits for `sys.boot_completed` and prints the serial `adb` reaches it by
(usually `emulator-5554`).

**Use a throwaway Discord account for any automated pass against a real
client**, never a personal one. Taps and keystrokes sent over `adb` go to
whatever is on screen: a step that misses its target can type into a real
conversation or open a delete prompt on a real message. An account that is
only in the test server has nothing to damage.

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

`.gitignore` and `.dockerignore` also skip `scripts/dev.env` and `.dev-run/`:
the dev scripts' settings, and their logs and pid files. `dev-up` creates the
runtime directory readable by your account only, and the scripts refuse to
use the data directory, or anything inside it, as the runtime directory,
under any spelling of the path.

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
| `scripts/` | `dev-up`, `dev-down`, `dev-status` and their tests (`scripts/tests/run`) |
| `docs/` | **local-only** (gitignored, absent from a clone): code standards, phase plan, design specs such as `docs/design/app-series-management.md`. `PLAN.md` at the repo root is local-only too. |
