# leaf activity (embedded app)

The gallery: a Vite + Svelte 5 SPA that runs inside Discord's activity iframe
and talks to `leaf-server` over the REST API. The same bundle also holds the
admin panel (`/admin`, a browser page) and the "leaf is running" page a browser
gets at `/`. Code standards are in `docs/svelte-guidelines.md` (a local-only
file; `docs/` is gitignored).

## Scripts

| Command                           | What it does                                            |
| --------------------------------- | ------------------------------------------------------- |
| `npm run dev`                     | Vite dev server on :5173, proxying `/api` → leaf-server |
| `npm run build`                   | `svelte-check` then `vite build` → `dist/`              |
| `npm run check`                   | Type-check (`svelte-check`)                             |
| `npm run lint`                    | ESLint (flat config; strict)                            |
| `npm run format` / `format:check` | Prettier                                                |
| `npm run test` / `test:run`       | Vitest (watch / once)                                   |
| `npm run bundle:check`            | Fail if the initial JS chunk blows the budget           |
| `npm run e2e`                     | Playwright browser suites (see [Testing](#testing))     |

## Running it in development

A Discord Activity is an iframe served from `https://<app-id>.discordsays.com/`,
and Discord's proxy fetches your machine through a public HTTPS **tunnel**. So
the dev loop is: run leaf-server + Vite locally, expose Vite over a tunnel, and
point a **dev** Discord application at that tunnel. There is no localhost-only
way to see the authed gallery — the SDK handshake only completes inside the
Discord client.

> **Just want to look at the screens?** You don't need any of the tunnel /
> Discord setup below. With the dev server running (`npm run dev`), open
> **<http://localhost:5173/mock.html>**: every screen (32 of them), with fixture
> data and no SDK, network, or auth. Pick a screen in the sidebar. The width
> buttons (**320 / 375 / 430 / Wide**) render it in an iframe of that width,
> and **Long text** fills names and captions with worst-case strings. The
> gallery and creator screens are the real views over a mock API, so taps,
> navigation and saves work (state lasts until reload); the boot screens and
> the admin page's header are copies. `mock.html?embed=1&screen=<id>[&long=1]`
> opens one screen on its own; the ids are in
> [`src/mock/screens.ts`](src/mock/screens.ts). Dev-only: it never ships in
> `dist/`. Source: [`src/mock/`](src/mock/).

> Run `cargo` commands from the repo root and `npm` commands from `activity/`.

### What you need (and where each value goes)

| Value                       | Where to get it                               | Where it goes                                                                    |
| --------------------------- | --------------------------------------------- | -------------------------------------------------------------------------------- |
| **Application (Client) ID** | Dev Portal → your app → General Information   | leaf-server setup (the gallery reads it from its `<id>.discordsays.com` address) |
| **Client Secret**           | Dev Portal → OAuth2 → Reset Secret            | leaf-server setup only — never the frontend                                      |
| **Bot Token**               | Dev Portal → Bot → Reset Token                | leaf-server setup                                                                |
| **R2 bucket + keys**        | Cloudflare dashboard → R2 (or none: a folder) | leaf-server setup                                                                |
| **Public URL**              | your tunnel hostname (step 2)                 | leaf-server setup (`public_url`)                                                 |

### 1. Create a dev Discord application

At <https://discord.com/developers/applications> → **New Application** (name it
e.g. "leaf (dev)"; keep it separate from any production app so URL mappings and
redirects don't collide). Then, in the left sidebar:

- **General Information** → copy the **Application ID**.
- **Bot** → **Reset Token** and copy it. Leave the **Privileged Gateway
  Intents** off; the bot connects without them.
- **OAuth2** → copy the **Client Secret** (Reset Secret if blank). Under
  **Redirects**, **Add** your tunnel URL from step 2, e.g.
  `https://leaf-dev.example.com`, and the same with `/admin/callback` if you
  want the admin panel. **Save Changes**.
- **Activities** → enable it; under **Supported Platforms** tick **iOS** and
  **Android** if you test on a phone (they are off by default); under **URL
  Mappings** add **Prefix** `/` → **Target** = your tunnel host **without** the
  scheme, e.g. `leaf-dev.example.com`. **Save**.

Invite the app to a test server: **OAuth2 → URL Generator**, tick **`bot`** and
**`applications.commands`**, open the generated URL, and add it to a server you
can test in. (The bot must be a member so leaf-server can check who may view a
series. The gallery's own `identify` scope is requested at runtime by the SDK,
not here.)

### 2. Set up the tunnel (stable hostname)

A **named** `cloudflared` tunnel gives a fixed `https://leaf-dev.<domain>`, so
you configure Discord once. It connects Cloudflare straight to your laptop and
**does not touch your home server, reverse proxy, or DDNS**.

```sh
brew install cloudflared
cloudflared tunnel login                                    # authorize your domain's zone
cloudflared tunnel create leaf-dev                          # prints a tunnel UUID + creds .json
cloudflared tunnel route dns leaf-dev leaf-dev.example.com  # creates the DNS record
```

Then `~/.cloudflared/config.yml`:

```yaml
tunnel: leaf-dev
credentials-file: /Users/you/.cloudflared/<TUNNEL-UUID>.json
ingress:
  - hostname: leaf-dev.example.com
    service: http://localhost:5173
  - service: http_status:404
```

_Throwaway alternative (no domain):_ `cloudflared tunnel --url
http://localhost:5173` prints a random `https://<x>.trycloudflare.com` — already
allowed by `vite.config.ts`, but you must re-paste that URL into the URL
mapping, the OAuth redirect, and `public_url` every run.

### 3. Configure leaf-server (one-time)

leaf-server stores its secrets in `leaf.conf` in its data directory, written by
a small web setup flow. With `cargo run` from the repo root that directory is
`./data` (`DATA_DIR` changes it); in Docker it is `/data`.

```sh
export DEV_GUILD_ID=<your test server id>   # optional: see guide/01-install.md
cargo run --bin leaf
```

`DEV_GUILD_ID` makes the bot register its commands in that one server (it must
be a member) instead of globally, which keeps a dev bot's commands apart.
Without it the commands are registered globally, as in production. If you use
it, keep it exported for every `cargo run` below.

With no config it starts in **setup mode** and prints a `/setup` URL and a
one-time **setup code** to the terminal. Open `http://localhost:3777/setup`,
enter the code, then fill in the **Application ID**, **Client Secret**, **Bot
Token**, the media storage (**A folder on this machine** needs no bucket or
keys; otherwise the **R2** bucket/keys), and **Public URL** =
`https://leaf-dev.example.com`.
Saving validates the values and flips leaf-server into run mode.

Already configured? Change the public URL with
`cargo run --bin leaf -- --reconfigure`. The form starts empty, so every value
has to be entered again.

### 4. Install the frontend

```sh
cd activity
npm install
```

No `.env` is needed. `.env.example` lists the two optional overrides.

### 5. Run it (three terminals)

```sh
cargo run --bin leaf                                          # API + assets on :3777
LEAF_DEV_HOST=leaf-dev.example.com npm run dev                # Vite on :5173 (from activity/)
cloudflared tunnel run leaf-dev                               # tunnel → :5173
```

`LEAF_DEV_HOST` adds your tunnel host to Vite's `allowedHosts` (Vite blocks
unknown hosts since 5.4). Quick-tunnel users can omit it.

### 6. Launch in Discord

In your test server, in a text channel: on desktop click the **Apps** button in
the message box and pick your app; on a phone tap **+** next to the message
box, then **Apps**, then your app. `/gallery` and the **Open gallery** buttons
on the bot's messages open it too. You land on the series list, or on a series
(the one a button or `/gallery series:` named, the one that uses this channel,
the one you opened last, or the only one there is).

### Troubleshooting

- **A page saying "leaf is running" in a normal browser** — expected. Without
  Discord's launch parameters the app shows that page instead of the gallery;
  the handshake only works inside the Discord client.
- **The bot is offline or has no commands** — `curl localhost:3777/api/status`
  says whether the gateway is `online`, `starting` or in `error` (with the
  reason), and carries a `notice` when the bot is online but Discord refused
  its command list. The terminal running `cargo run` has the detail. With
  `DEV_GUILD_ID` set, the bot must be in that server for its commands to
  register (guide/07-troubleshooting.md, "Bot offline").
- **"Blocked request. This host is not allowed."** — set `LEAF_DEV_HOST` to your
  tunnel host (see step 5).
- **Token exchange fails (400)** — the **URL Mapping** target, the **OAuth2
  Redirect**, and leaf-server's **Public URL** must all name the same origin.
- **"Couldn't load the gallery"** — with "leaf's bot isn't in this server",
  invite the dev bot to the test server and let it connect once (leaf-server
  learns of a server when the bot is online in it). With "You don't have access
  to this", your account isn't a member of that server.
- **"No series here yet"** although one exists — it isn't visible to your
  account (private, role-limited, someone else's sprout, or revoked).
- HMR may not survive the proxy round-trip; close the Activity and open it
  again (reloading only the iframe hangs the handshake).

## Testing

Three layers. `npm run test:run` is Vitest in jsdom: components, stores and
pure logic, with no browser. `npm run e2e` is Playwright, in real Chromium and
WebKit, and has two halves:

- **The mock screens** (`e2e/mock`, `/mock.html` above): every screen with
  fixture data. It is the phone-width pass that used to be done by hand, and
  it needs no Discord, no leaf-server and no network.
- **The real API** (`e2e/app`): the built Activity, booted through the real
  Embedded App SDK inside a stand-in for the Discord client, against the real
  leaf API over a seeded database. It is where the client and the server are
  held to each other. See [Against the real API](#against-the-real-api).

One-time setup, after `npm install`:

```sh
npx playwright install chromium webkit   # on Linux add --with-deps
```

| Command                     | What it runs                                                                      |
| --------------------------- | --------------------------------------------------------------------------------- |
| `npm run e2e:mock`          | The measurements and the flows on the mock screens, in Chromium and WebKit        |
| `npm run e2e:visual`        | Screenshot comparison of every mock screen (Chromium, 375px)                      |
| `npm run e2e:visual:update` | Rewrites the screenshot baselines                                                 |
| `npm run e2e:app`           | The Activity against the real API, in Chromium and WebKit. Needs a Rust toolchain |
| `npm run e2e`               | All of the above. CI runs the same, saving the screenshots instead of comparing   |
| `npm run e2e:check`         | Type-checks `e2e/` and `playwright.config.ts` (`npm run check` does not)          |

Add `-- --ui` for Playwright's own runner or `-- -g "375px.*settings"` to pick
tests. For one engine or one file, name the project to Playwright itself: the
scripts above already name theirs, and Playwright runs every project it is
given, so a `--project` added after `--` widens their run instead of
narrowing it (it does narrow plain `npm run e2e`, which names none).

```sh
npx playwright test --project=mock-webkit
npx playwright test --project=app-chromium e2e/app/boot.spec.ts
```

A failed run leaves an HTML report with a trace per failure:
`npx playwright show-report`.

Playwright starts the servers a run needs, and only those: `npm run e2e:mock`
does not wait for a cargo build.

- The mock screens get their own Vite server on **:5183** (`LEAF_E2E_MOCK_PORT`
  changes it), which leaves `npm run dev` on :5173 alone. It has no API proxy
  and no hot reload ([`e2e/support/vite.mock.config.ts`](e2e/support/vite.mock.config.ts)).
- The app suites get leaf-server's `e2e_server` example on **:3811** for
  Chromium and **:3812** for WebKit (`LEAF_E2E_APP_PORT` changes the first;
  the second follows it), serving a build of the Activity made for the run.

Outside CI a server that is already running on its port is reused.

### What is checked

[`e2e/mock/screens.spec.ts`](e2e/mock/screens.spec.ts) opens every screen in
[`src/mock/screens.ts`](src/mock/screens.ts) with its usual text and with
worst-case text (`&long=1`), at 320, 360, 375 and 430px (phones: touch, a
mobile user agent, 2x or 3x) and at 768 and 1280px. Chromium runs every width;
WebKit, the nearest stand-in for iOS that CI can run, runs the phone widths.
A screen added to `screens.ts` is covered with nothing to add here. Each is
held to:

- **No sideways overflow**: the page does not scroll sideways, and no element
  reaches past the screen edge. Only what a container scrolls sideways on
  purpose (`overflow-x: auto` or `scroll`) is left out; content that an
  `overflow: hidden` ancestor cuts off at the edge is reported, as cut off.
- **44 x 44px tap targets** for every button, link, field, select and summary,
  at every width. What a closed disclosure holds ("More options") is measured
  as it will be once opened.
- **16px text in every field**, so iOS does not zoom on focus.
- **axe-core, on the whole page**, with no rule turned off. This is the run
  that checks colour contrast (the jsdom run in `src/lib/test/a11y.ts` cannot)
  and the page-level rules (one `<main>`, an `<h1>`, nothing outside a landmark).
  An element axe could not judge ("incomplete": text over a picture or a
  gradient, say) fails like a violation, with axe's reason, unless it is one of
  the two named below.
- **A quiet console and network**: no error or warning, no failed or refused
  request, no picture that is missing or broken (Vite answers a missing file
  with the app's page and a 200, so the content type is checked too), and
  nothing sent anywhere but the Vite server: a request or WebSocket to any
  other origin is cut off before it leaves the browser, then reported. This
  one runs for every test in the suite, not only these.

A screen is measured once it has settled: no skeleton or spinner left, the
pictures that are due loaded, the web fonts in, and no fade or other
transition still running.

[`e2e/mock/flows.spec.ts`](e2e/mock/flows.spec.ts) walks the journeys at 375px
in both engines: series list to a day, paging files then days by swipe and by
button, focus back on the calendar cell after Escape or Close; a pinch that
must not turn the page; the create form (an empty name refused on the field, a
valid one landing on the first-post card); a series whose channel was deleted
(the owner's note, steps that name no channel, Series settings taking
another); series settings (Save state, the
leave-without-saving question); the admin panel (pickers, save, the Revoke
question); the buttons each failed start offers; reduced motion on every
screen, on a screen held on its skeletons, and on an animation no rule was
written for; and forced colours, where a select gets its own arrow back
(Chromium only: no WebKit browser forces colours).

[`e2e/mock/tile.spec.ts`](e2e/mock/tile.spec.ts) is leaf minimised: the
"Minimised" screens in a viewport the size of Discord's tile (120 x 120,
160 x 160, 240 x 135), where one card has to be all there is; what is still
there when the tile is opened again; and "Open gallery" pressed in chat. A
tile asks the server for that press every four seconds, opens it behind the
card and says "Tap to open"; at a phone's size, or with no session, it asks
nothing. [`e2e/support/chat.ts`](e2e/support/chat.ts) stands in for the chat
side, and the page's clock is run forward rather than waited on.

[`e2e/mock/checks.spec.ts`](e2e/mock/checks.spec.ts) tests the measurements
themselves: it breaks a healthy screen each way they are meant to catch and
expects them to say so.

### The exceptions

Each is written down where it is applied, with its reason. There are no others.

| Exception                                                           | Where                                        | Why                                                                                                                                                                      |
| ------------------------------------------------------------------- | -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Calendar day cells may be 41px (not 44) in a viewport under 345px   | `TARGET_EXCEPTIONS`, `e2e/support/layout.ts` | Seven columns do not fit 7 x 44px; `Home.svelte` and `MonthGrid.svelte` narrow the padding and gaps to reach 41.7px at 320                                               |
| A checkbox or radio is measured together with its label             | `targetRect`, same file                      | "The row around them supplies the 44px" (`app.css`)                                                                                                                      |
| A link inside a sentence is not held to 44px                        | `inlineLink`, same file                      | WCAG 2.5.8's inline exception. No screen has one today                                                                                                                   |
| Under reduced motion, a slow opacity-only fade may run forever      | `e2e/support/motion.ts`                      | The spinner and the boot mark "breathe" instead of freezing (`Spinner.svelte`, `App.svelte`)                                                                             |
| Contrast is not judged for the labels on a calendar day's picture   | `UNJUDGED`, `e2e/support/axe.ts`             | White text on a photo, over a scrim drawn as a pseudo-element (`DayCell.svelte`): there is no one background to measure against                                          |
| Contrast is not judged for text that wraps in the open day viewer   | same                                         | axe compares what is under each line of a text, through to the inert page the viewer covers, and gives up when the lines differ                                          |
| A select's chevron is taken off while axe scans, then put back      | `axeViolations`, same file                   | axe judges no text with a gradient behind its element; the chevron is two 6px gradients in the padding (`app.css`), so the text is judged on the colour really behind it |
| `viewer-loading` is ready while its spinner is still up             | `HELD_LOADING`, `e2e/support/mock.ts`        | That screen is the loading state                                                                                                                                         |
| `expired`, `load-error` and `viewer-failed` may log one named error | `EXPECTED_OUTPUT`, same file                 | Their scenario fails a request on purpose and the view logs it                                                                                                           |

A new screen that logs on purpose, or stays on a spinner, needs a line in
`HELD_LOADING` or `EXPECTED_OUTPUT`. One thing is evened out rather than
excepted: at the phone and tablet widths the page's scrollbar is hidden,
because a phone draws it over the page and a desktop engine may give it 15px
of the layout.

### Screenshots

`npm run e2e:visual` compares each screen with its baseline in
`e2e/mock/__screenshots__/<os>/`, with the clock fixed at 20 May 2026 and
animations stopped. A picture is the whole screen at 375px, top to bottom, not
only the part a phone shows before scrolling; the day viewer's is the
viewport, which it fills. One pixel off by more than a small colour tolerance
fails. After a change that is meant to look different:

```sh
npm run e2e:visual:update
git diff --stat e2e/mock/__screenshots__   # look at the pictures before committing
```

Baselines depend on the OS's fonts and emoji, so only the macOS set is
committed; a set made on Linux or Windows is gitignored and stays local (make
one with `e2e:visual:update`). CI does not compare: it saves every screen's
picture and uploads them as the `playwright-screenshots` artifact, next to the
HTML report (`playwright-report`). The pictures of the last local run are in
`test-results/gallery/`.

### Against the real API

`npm run e2e:app` runs [`e2e/app`](e2e/app) in two projects: `app-chromium`
(everything) and `app-webkit` (the boot, calendar, viewer and isolation
specs), both at 375px with a phone's touch and user agent. The first run
compiles leaf-server; after that a run takes about half a minute.

What is real and what stands in:

- **The Activity** is the production bundle. Playwright builds it into
  `node_modules/.leaf-e2e/dist` (not `dist/`) with `VITE_DISCORD_CLIENT_ID`
  set to the seed's application id, because off `<id>.discordsays.com` the
  build is where the Activity learns its id.
- **The server** is `cargo run -p leaf-server --example e2e_server`: the real
  router, API, policy and media proxy, serving that bundle from `STATIC_DIR`.
  Its database is a fresh SQLite file filled with a fixed seed (six personas,
  two guilds, six series, 150 days of posts with small real images and
  videos), its object store is in memory, and Discord's API is a stub that
  can be told to fail or stall. The seed, every control route (`/__e2e/…`)
  and the personas are described in the doc comment at the top of
  [`crates/leaf-server/examples/e2e_server/main.rs`](../crates/leaf-server/examples/e2e_server/main.rs).
- **The Discord client** is a page ([`e2e/support/discord.ts`](e2e/support/discord.ts))
  served under the leaf server's own origin, which frames the Activity with
  Discord's launch parameters and answers the SDK's `postMessage` protocol:
  the handshake and READY, AUTHORIZE (grant, decline, or leave the prompt
  open), AUTHENTICATE, layout-mode subscriptions and events, CAPTURE_LOG,
  OPEN_EXTERNAL_LINK and close. The SDK inside the frame is the real one, and
  it checks every answer against its own schemas. A command the page has no
  answer for fails the test that sent it.
- **Discord's consent screen**, for the admin panel's sign-in, is answered in
  place of leaf's `/admin/login` redirect, so the real `/admin/callback` runs
  and nothing goes to discord.com.

Each test starts from a reset server (`POST /__e2e/reset`), and a server has
one seeded world, so the tests of a project run one at a time and each engine
has a server of its own. Specs import `test` from
[`e2e/support/app.ts`](e2e/support/app.ts), which gives them `leaf` (the control
routes, and the API as any persona), `discord.launch(persona, options)` and
`time.pass(ms)`. The page's clock is fixed at 20 May 2026, two days after the
seed's newest post; the server keeps real time for sessions and signatures.
They find things as a person or a screen reader does, by role, label and
text, and take ids from `leaf.seed` and the API's own answers; the few things
the page offers no such handle for (a calendar cell, the name on a series
card) are reached through [`e2e/support/screens.ts`](e2e/support/screens.ts).

What the specs hold the two sides to:

| Spec                | What it checks                                                                                                                                                                                                                                           |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `boot.spec.ts`      | The handshake and the commands leaf sends, on a phone and on desktop; a DM launch (no permission prompt); a declined prompt and Grant access; a prompt left open; no READY and a late READY; Discord slow or down during sign-in, and Try again resuming |
| `privacy.spec.ts`   | Each persona's series list, exactly; the creator's revoked series and its banner; someone outside the server; hidden series, settings and media refused when the API is asked directly                                                                   |
| `calendar.spec.ts`  | Months newest first, the month with no posts, two days on one date, the day with no file, the stats, the day tools, and dates and "today" in the guild's timezone on a device in another                                                                 |
| `viewer.spec.ts`    | Photos through `/api/media` (files before days), a video's `Range` request and its `206`, a day removed after the calendar loaded, the original-post link handed to Discord                                                                              |
| `creator.spec.ts`   | Creating a series end to end and finding it stored; a taken name; the reasons a member cannot start one, with their role and date; settings saved and read back; a refused setting under its field; an undelivered reminder                              |
| `freshness.spec.ts` | A day archived while the gallery is open, on the return from picture-in-picture; "Open gallery" presses before and while leaf is open; an activity link's `custom_id`                                                                                    |
| `session.spec.ts`   | A token renewed at its refresh point; one past the seven-day cap refused renewal, used until it lapses, then the session-ended screen                                                                                                                    |
| `admin.spec.ts`     | Sign-in through `/admin/login` and `/admin/callback`; the server list; the creator role and timezone pickers, saved and still there after a reload; revoke and restore, seen from a member's gallery; the card for someone who manages nothing           |
| `contract.spec.ts`  | Every response the gallery and the admin panel parse, asked of the API with no browser and compared with the client's schema key by key, in both directions                                                                                              |
| `guard.spec.ts`     | The guard itself, on what it is for: a field that changed shape, and a refused response                                                                                                                                                                  |
| `isolation.spec.ts` | A redirect from the server to another machine, which no route handler sees, gets nowhere                                                                                                                                                                 |

The quiet console and network rule above applies to every one of these tests,
and it is half of the contract check: the client logs a warning for an
optional field that arrives in the wrong shape (`opt()` in
[`src/lib/api/schemas.ts`](src/lib/api/schemas.ts)) and an error when a
required one stops a screen, the SDK logs an error for an answer that does not
fit its schema, and any response of 400 or above fails the test. A test that
expects a refusal says so first, with `expectRefusal`.

What that rule cannot see is an optional field that is simply gone. `opt()`
reads an absent field as valid and says nothing, so a server that renames
`total_days` leaves the console quiet and every screen loading, short of one
detail. `contract.spec.ts` is the other half: it asks the real API for every
response the client parses, the gallery's and the admin panel's, reads each
with the client's own schema, and then compares keys
([`e2e/support/contract.ts`](e2e/support/contract.ts)). Three things fail it:
a key a schema names that no answer had, a key the server sent that no schema
names, and a key that only some answers of one shape had (bar the few the
server leaves out when it has nothing to say, which are named in the spec). So
does an exported schema with no answer to read: a response the client starts
to parse needs its sample there. It compares names, not meanings. That a
number is the right number is still for the screens' own tests to say.

The app projects also send every address but 127.0.0.1 to a proxy that is not
there, because a redirect (leaf's own `/admin/login` goes to discord.com) is
followed inside the browser where no route handler can stop it.
`isolation.spec.ts` checks that in both engines, against a control route that
redirects to a name that never resolves.

The exceptions, each written where it is applied:

| Exception                                                                   | Where                                     | Why                                                                                                                  |
| --------------------------------------------------------------------------- | ----------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Chromium's warning about a sandboxed frame on the page's own origin         | the `discord` fixture, `app.ts`           | The stand-in sandboxes the Activity as Discord does, but has to share its origin for the SDK to listen to it         |
| A video's media request may be aborted                                      | `viewer.spec.ts`, the video test only     | A player stops a download it has enough of, and again when the viewer closes                                         |
| "Button failed to load" console errors in WebKit while a video is on screen | same                                      | Playwright's WebKit build lacks some of its own player's artwork                                                     |
| Chromium is given the WebM clip and WebKit the MP4                          | `CLIPS`, same file                        | Not every build of Playwright's Chromium decodes H.264; an iPhone plays MP4                                          |
| The session tests replace the answer to `POST /api/token`                   | `signInWith`, `session.spec.ts`           | The server keeps real time, so a session that is about to lapse or past its cap is minted by a control route instead |
| A violation's `params`, and each key in it, may be absent                   | `VIOLATION_SPECIFICS`, `contract.spec.ts` | The server sends a rule's own numbers or name and no others, and `guild_not_setup` has none                          |
| `sprouts_published` is absent from the settings a `GET` returns             | `SAVE_ONLY`, same file                    | Only the answer to a save counts the sprouts that save published                                                     |
| `namedIdSchema` has no response of its own                                  | `NOT_A_RESPONSE`, same file               | It is only what `roleOptionSchema` is built from                                                                     |
| The admin spec is a desktop browser, not the project's phone                | `test.use`, `admin.spec.ts`               | Admins run servers from a desktop; the mock suite measures the panel at every width                                  |
| Three calendar states are found by CSS class                                | `DRAWN`, `e2e/support/screens.ts`         | An empty date is hidden from screen readers and differs from another only in how it is drawn                         |

To watch a test drive the gallery, add `-- --headed -g "<part of its title>"`
or `-- --ui`. To keep a server up between runs, or to click through the seeded
admin panel by hand:

```sh
# from the repo root: build the Activity for the e2e server, then serve it
(cd activity && VITE_DISCORD_CLIENT_ID=800000000000000001 VITE_API_BASE=/api \
  npx vite build --outDir node_modules/.leaf-e2e/dist --emptyOutDir)
E2E_PORT=3811 STATIC_DIR=activity/node_modules/.leaf-e2e/dist \
  cargo run -p leaf-server --example e2e_server
```

`http://127.0.0.1:3811/__e2e/admin/login?persona=admin` then opens the admin
panel signed in (the panel's own Sign in link goes to discord.com, which knows
nothing of this server). A run reuses a server it finds on its port, with
whatever build that server is serving: stop it to have the next run build
afresh. Set `LEAF_E2E_SERVER_LOG=info` (or any `tracing` filter) to see the
server's log in a run's output.

### What this does not cover

Neither suite is Discord. The mock screens run no handshake and no server. The
app suites run both, but against a stand-in client that was written from the
SDK's source, not from Discord's: what the real client sends for a dismissed
permission sheet, and how its webviews treat focus, visibility and the system
back gesture, still need a device (`docs/ux-device-test-checklist.md`, a
local-only file). The bot is not in the suite either: days are archived and
"Open gallery" presses recorded through control routes that write what the bot
would. Media comes from an in-memory store, not R2. And no phone is involved:
safe-area insets, the on-screen keyboard, native pickers and Discord's own
webviews still need one. WebKit on a Mac or on Linux is not iOS Safari, and its
touch input here is synthetic (pointer events dispatched in the page; Chromium
gets real touch input), see [`e2e/support/touch.ts`](e2e/support/touch.ts).
The screenshots are one width in one engine on one OS. And whether the white
labels on a calendar day read on a very bright photo is a question for eyes,
not for axe: see the first contrast exception above.

## Build

`vite build` emits `dist/`, which leaf-server serves from `STATIC_DIR`
(`activity/dist` when unset). The Docker image does this build itself, with no
build arguments. Production deployment is covered in
[../DEPLOY.md](../DEPLOY.md) and [../guide/](../guide/README.md).
