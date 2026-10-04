# 07 — Troubleshooting

Common failures and fixes. (Discord and Cloudflare dashboard layouts drift; see
the note in the [guide index](README.md#dashboards-change) if a menu isn't where
a guide says.)

## Start here: `leaf doctor`

When something is off, ask leaf to check itself before reading further:

```sh
docker compose exec leaf leaf doctor
```

(With the container stopped: `docker compose run --rm leaf doctor`.)

It checks a configured install live and prints one line per thing it checked:
`ok`, `warn`, `FAIL` or `skip`, the name of the check, a sentence, and for a
warning or a failure the next step to take.

```
ok    config    leaf.conf loads from /data.
ok    config    The public address is https://leaf.example.com.
ok    discord   Discord accepts the bot token (bot user “leaf”).
ok    discord   The token belongs to application “leaf” (ID 123456789012345678), the client ID in leaf.conf.
ok    discord   Discord accepts the client ID and client secret together, so sign-in can work.
ok    commands  All 13 commands leaf registers are listed globally, and nothing else is.
ok    commands  The Activity's Entry Point command (“launch”) is registered.
ok    gateway   Discord's gateway answers, and 998 of 1000 session starts remain.
FAIL  storage   The storage check failed at its write step. R2 has no bucket named “leaf-media” at this endpoint. […] Next: Correct it in the R2 dashboard, or enter new storage details with --reconfigure […]
ok    database  leaf.db opens read-only, and its 3 migrations are exactly the ones this leaf has.
10 checked: 9 ok, 0 warn, 1 FAIL, 0 skip.
```

It starts neither the web server nor the bot, writes nothing to the database,
and never prints a credential, so it is safe to run beside a running leaf and
to paste into a bug report. The one thing it writes is a small test object in
the bucket (a test file, when storage is a folder), which it removes again. The exit status is `0` when nothing
failed, `1` when a check failed, and `2` for a command line it can't read.

(One file beside the database can change. While leaf is running, or after it
was stopped without a clean shutdown, there is a `leaf.db-wal` file, and
SQLite can't read it without its index, `leaf.db-shm`: the doctor's read-only
connection creates that file if it is missing and rewrites it otherwise.
`leaf.db` and `leaf.db-wal` themselves are not written.)

| Check | What it confirms | When it fails |
| --- | --- | --- |
| `config` | `leaf.conf` loads from `DATA_DIR`. The Public URL is `https://` (plain `http://` only on localhost) and is the bare address, with no path. | [`--reconfigure`](01-install.md#changing-credentials-later---reconfigure) |
| `discord` | Discord accepts the bot token (`GET /users/@me`). The token belongs to the application whose ID is configured (`GET /applications/@me`). Discord accepts the client ID and client secret together (what every sign-in needs). | [Bot offline](#bot-offline), [Admin panel](#admin-panel) |
| `commands` | Every command leaf registers is in Discord's list (the global one, or the `DEV_GUILD_ID` server's), nothing is listed that leaf no longer has, and the Activity's Entry Point command exists. | [Commands](#commands) |
| `gateway` | `GET /gateway/bot` answers and the bot has session starts left (each connection uses one of a daily allowance). | [Bot offline](#bot-offline) |
| `storage` | A test object, `leaf-doctor-canary`, can be written, read back, read in part (a byte range, which video needs) and deleted in the bucket. It is removed even when a later step fails, and after a write that got no answer (it may have been stored all the same); if it can't be removed, the line says so. When storage is a folder on this machine, the same is done with a file in that folder (which is created if it is missing, as leaf itself does at start), and the line starts "Storage is a local folder" and names it. | The sentence is the one the [setup page](#setup-page) shows for the same problem. For a folder, see [Storage in a folder](#storage-in-a-folder). |
| `database` | `leaf.db` opens read-only and holds exactly the migrations this version of leaf has. Migrations leaf hasn't applied yet are a `warn` (leaf applies them at its next start); ones it doesn't know, or that differ, are a `FAIL`. | [01 § Data, backups, and updates](01-install.md#data-backups-and-updates) |

A check that depends on something that failed isn't made: it prints `skip`,
and the failure is on a line of its own (a refused bot token is one `FAIL`
under `discord`, and `commands` and `gateway` are skipped).

Two more checks run when you ask for them:

- **`--url [<address>]`** asks a running leaf over HTTP: `/healthz` answers,
  `/api/status` says the bot is `online`, the gallery's page and the script it
  loads are served, an unknown `/api/…` path answers a JSON 404 rather than
  the gallery's page, and (once a day is archived) a stored file answers a
  byte-range request with `206`. Without an address it asks the Public URL,
  which goes through the tunnel like a browser does. To ask the container
  directly, which tells a leaf problem from a tunnel problem:
  ```sh
  docker compose exec leaf leaf doctor --url http://127.0.0.1:3777
  ```
- **`--db-copy <path>`** copies that database file (with its `-wal` and `-shm`
  files, if any) into a temporary directory, applies the migrations to the
  copy, runs `PRAGMA integrity_check` and `PRAGMA foreign_key_check` on it,
  and compares the number of series, posts and media files before and after.
  The original is only ever read. Use it to try an upgrade's migrations before
  the upgrade ([01 § Data, backups, and updates](01-install.md#data-backups-and-updates)).
  If leaf is writing to the database during the copy, the copy can catch a
  write half-way and fail its integrity check: run it again.

`--only <check,...>` limits a run to the checks named, for example
`--only storage` or `--only config,database`. `--json` prints one JSON object
instead of lines, for a script or a CI job: `ok` (false when anything
failed), `summary` (the four counts) and `results` (each with `check`,
`status`, `message` and, for a warning or failure, `next`). `leaf doctor
--help` lists all of this.

The doctor reads `DATA_DIR` and `DEV_GUILD_ID` the way leaf does, so inside the
container it looks at the install the container runs. The technical cause of a
failure (R2's or the network's own error text, with the storage keys masked)
is logged to stderr, apart from the lines above.

## The log

Most other answers start in the log:

```sh
docker compose logs leaf
```

## Setup page

- **"That code isn't right."** The code is printed only to the logs, and a new
  one is printed at every start. Get the current one with `docker compose logs
  leaf | grep -i "setup code"`. After ten wrong tries the page says "Too many
  wrong codes": restart the container for a new code.
- **A message under a field.** leaf checks everything with Discord and R2
  before saving and puts each problem next to the field it belongs to. The
  common ones:
  - *Bot token:* "Discord didn't accept this bot token": reset it (Bot → Reset
    Token) and paste the new one. A token from a **different application** than
    the Application ID you entered is refused too.
  - *Application ID / client secret:* "didn't accept this application ID and
    client secret together": copy both from the OAuth2 tab of the same
    application; a reset secret replaces the old one.
  - *Bucket:* "R2 won't let this API token add files": check the bucket name is
    exact, and that the API token has **Object Read & Write** for that bucket.
  - *Access key ID:* "This isn't an R2 access key ID": you pasted the token
    value or the secret; the Access Key ID is the 32-character one.
  - *S3 endpoint:* "This is a folder, not an S3 endpoint": to keep media in a
    folder, choose **A folder on this machine** at the top of the Media storage
    section and enter the path there.
  - *Folder path:* see [Storage in a folder](#storage-in-a-folder).
  - *Public URL:* the address only (`https://leaf.example.com`), no path.
- **The success page stays on "Connecting the bot to Discord…"**, or says "The
  bot couldn't connect to Discord." The line under it gives the reason when
  leaf knows one; [Bot offline](#bot-offline) has the fixes. The page stops
  checking after 10 minutes, and `/api/status` gives the same answer later.
- **The success page says "The bot is online, but something needs
  attention."** The bot is connected; the line under it says what is wrong.
  leaf reports one such problem: Discord hasn't accepted its command list,
  which is the `registering commands failed` row under
  [Bot offline](#bot-offline).
- **"Setup is already complete" or "leaf is already set up".** You are on a
  setup tab from before. On a configured leaf, `/setup` redirects to `/admin`.
  To change credentials use
  [`--reconfigure`](01-install.md#changing-credentials-later---reconfigure).

## Container and volume

- **`EACCES` / can't write `leaf.conf` or the DB.** The data directory must be
  writable by the non-root `leaf` user. The image pre-owns `/data`; if you
  bind-mount a host path instead of the named volume, `chown` it to the
  container's user or use the named volume from `docker-compose.yml`.

## Storage in a folder

For an install whose media is kept in **a folder on this machine** instead of
R2 ([01 § Storage](01-install.md#storage-r2-or-a-folder-on-this-machine)). The
log line `storing media in a folder on this machine` at each start names the
folder in use.

- **"Enter the folder's full path, starting with /"** (setup page). leaf needs
  the whole path, such as `/data/media`. `media`, `./media` and `~/media` are
  refused, and so is `/` alone.
- **"leaf can't create this folder."** The folder isn't there and leaf couldn't
  make it: a folder above it isn't writable by the user leaf runs as, or a
  file already has that name. Create the folder yourself and make it writable
  by that user, or pick a path inside the data directory.
- **"leaf can't save files in this folder."** The folder exists, but the user
  leaf runs as can't write in it, or the disk is full or mounted read-only. In
  Docker, a folder inside the data volume (`/data/media`) is writable already;
  a bind-mounted host folder has to be `chown`ed to the container's user.
- **leaf stops at start with `storage folder: creating …` or `storage folder:
  opening …`.** The same two causes, met when leaf starts: the line ends with
  the system's own reason (for example `Permission denied`). Fix the folder,
  or choose other storage with
  [`--reconfigure`](01-install.md#changing-credentials-later---reconfigure).
- **Every photo and video is missing after an update or a new container**, and
  the gallery shows "This photo didn't load" for old days. The folder was
  outside the mounted data volume, so it was inside the container that was
  replaced, and the files went with it. They can't be brought back from leaf's
  side: restore the folder from a backup. Then move it into the volume
  (`/data/media`) and enter that path with `--reconfigure`.
- **Old days lost their pictures after switching between a folder and R2.**
  `--reconfigure` changes where leaf looks; it doesn't move the files. Copy
  them from the old place to the new one
  ([01 § Changing credentials later](01-install.md#changing-credentials-later---reconfigure)).
- **`leaf.conf` has `bucket = "local"` (or any other text) under a `file://`
  endpoint.** That is a config from before the setup page offered a folder. It
  keeps working: with a `file://` endpoint the bucket and the two keys are not
  read.

## Not reachable

- **502 / can't reach `leaf.example.com`.** Confirm the `leaf` container is up
  (`docker compose ps`), the tunnel sidecar is running
  (`docker compose --profile tunnel up -d`), `TUNNEL_TOKEN` is set in `.env`,
  and the tunnel's public hostname targets `http://leaf:3777` (the compose
  service name). The DNS record should be **proxied** (orange cloud).
- **The address shows a page saying "leaf is running".** That is correct. The
  gallery only opens inside Discord
  ([04 § Opening the gallery](04-usage.md#opening-the-gallery)); the page links
  to the admin panel.

## Bot offline

Signs: leaf shows as offline in the member list and its commands answer "The
application did not respond", while the gallery and the admin panel still
work.

Ask leaf first. `/healthz` only says the web server is up; the bot's state is
at `/api/status`:

```sh
curl -s https://leaf.example.com/api/status
```

| Answer | Meaning |
| --- | --- |
| `{"gateway":"online"}` | The bot is connected. |
| `{"gateway":"starting"}` | It is connecting. |
| `{"gateway":"starting","notice":"…"}` | It is still connecting, and `notice` says why that is taking long: what the last attempt failed on, or that the bot hasn't connected after 30 seconds. |
| `{"gateway":"error","detail":"…"}` | The last attempt failed; `detail` is a sentence saying why. |
| `{"gateway":"online","notice":"…"}` | Connected, but something needs attention; `notice` says what (for example Discord has not accepted leaf's command list, so commands are missing or out of date). |

A healthy start logs `gateway connected` and then `commands registered`. What
leaf does after a failure depends on the cause:

| In the log | Cause and fix |
| --- | --- |
| `gateway exited with error` with `Sent invalid authentication`, then `gateway will not be retried` | The bot token was reset or revoked. leaf doesn't retry this one. Enter the new token with [`--reconfigure`](01-install.md#changing-credentials-later---reconfigure). |
| `gateway exited with error` or `gateway stopped unexpectedly`, then `reconnecting to the gateway after a wait` | Discord or the network was unreachable. leaf reconnects by itself, after 5 seconds at first and at most every 5 minutes. If it never gets through, check the container's outbound network. |
| `gateway exited with error` with `Discord did not answer within 30 seconds` | Nothing came back at all. Something between leaf and Discord is holding the connection: check the machine's network, and any firewall that filters outgoing connections (one that asks before it lets a new program out holds leaf until you allow it). leaf tries again by itself. |
| `gateway exited with error` with `Discord rate-limited the bot` | Discord answered, and told the bot to wait longer than 30 seconds. Nothing on your machine is in the way. leaf tries again by itself; if it goes on for hours, the address leaf connects from has sent Discord too many requests (a shared host, or another bot on the same address). |
| `gateway not connected yet; still trying`, and `/api/status` says `starting` with "The bot hasn't connected to Discord yet" | leaf's first requests to Discord went out, but the bot's live connection (a WebSocket to `gateway.discord.gg`) has not come up after 30 seconds. Check that a firewall or proxy lets WebSocket connections out. leaf keeps trying, and the notice goes away once it connects. |
| A line starting `registering commands failed` | The bot is online, but Discord didn't take its command list, so new or changed commands don't work yet (Discord keeps the list of an earlier run). `/api/status` shows a `notice` while this lasts. leaf tries again by itself (after 30 seconds, then at growing intervals up to 30 minutes); the line's `error` field has Discord's reason. With `DEV_GUILD_ID` set, the usual cause is that the bot isn't in that server or the ID is wrong ([01](01-install.md#command-registration-and-dev_guild_id)). |

## Commands

- **Slash commands or "Archive to Series" are missing.** They are registered
  globally, so they exist in every server the bot is in once the log shows
  `commands registered`. They appear at once; waiting doesn't help, though a
  Discord app that was already open may need a restart to refresh its list. A
  member also needs **Use Application Commands** in the channel. `/setup`,
  `/export` and `/import` are hidden from members without Manage Server. With
  `DEV_GUILD_ID` set, the commands exist only in that one server: remove it
  from `.env` for a real install
  ([01](01-install.md#command-registration-and-dev_guild_id)).
- **Every command appears twice.** One set is global and one belongs to the
  server, left from a start with (or without) `DEV_GUILD_ID`. leaf removes the
  set it isn't using each time it registers its commands, so restart leaf, wait
  for `commands registered`, then restart your Discord app. If they stay, the
  log has `could not clear the global command list` or `could not clear
  guild-scoped commands` with Discord's reason.
- **`leaf doctor` says commands are not registered, or lists commands leaf no
  longer has.** leaf's last registration didn't go through, or another program
  uses the same application. Restart leaf and wait for `commands registered`
  in the log; if the log has `registering commands failed` instead, that row
  under [Bot offline](#bot-offline) applies.
- **`leaf doctor` says the Entry Point command is gone.** Activities is
  enabled, but the command Discord created for it (Launch) was deleted, so leaf
  is missing from the app launcher. leaf never creates this command; it only
  sends the existing one back when it registers its own. Discord's guide
  "Setting Up an Entry Point Command" gives the request that creates it again
  (leaf itself has never sent it, so treat it as Discord's instructions, not a
  tested path):
  ```sh
  read -rs TOKEN   # paste the bot token and press Enter; it isn't shown
  printf 'Authorization: Bot %s\n' "$TOKEN" |
    curl -X POST "https://discord.com/api/v10/applications/<application id>/commands" \
      -H @- -H "Content-Type: application/json" \
      -d '{"name":"launch","description":"Launch leaf","type":4,"handler":2,"integration_types":[0],"contexts":[0]}'
  unset TOKEN
  ```
  The token reaches curl on its standard input (`-H @-` reads a header from
  there; curl 7.55 or newer), not on its command line, where anyone else on
  the machine could read it from the process list while curl runs. Then run
  `leaf doctor --only commands` again.
- **"That command is no longer part of leaf."** Your Discord app is showing a
  command from an older leaf. Restart Discord to refresh the list.
- **"This command changed in an update and Discord hasn't caught up yet."**
  Discord sent the command in a shape this version of leaf doesn't register.
  Restart Discord. If it keeps happening, leaf's command list wasn't accepted
  after the update: look for `registering commands failed` in the log
  ([Bot offline](#bot-offline)).
- **"You need the Manage Server permission in this server to use that
  command."** `/setup`, `/export` and `/import` need Manage Server, even where
  the server's Integrations settings show them to other members.
- **"Something went wrong on leaf's side … Reference for the server owner:
  `<number>`."** leaf hit an error it has no better words for. The log has a
  `command failed` or `command panicked` line carrying the same number as
  `reference`, with the cause.
- **"Discord didn't answer leaf just now … Try again in a moment."** Discord
  was slow, rate-limiting or unreachable for that one request. Trying again is
  the fix.
- **"Archive to Series" isn't where I look.** On a phone: press and hold the
  message, tap **Apps** (scroll down in the menu to find it), then your bot's
  name in the list of apps, then **Archive to Series**. On desktop: right-click
  the message, choose **Apps**, then **Archive to Series**. `/leaf` shows these
  steps to anyone.
- **No greeting when leaf joined.** leaf greets a server once, the first time
  it sees it, in the system channel or else the top-most text channel it can
  post in. With no channel it can post in, the greeting is skipped (the log
  says `no channel I can speak in`). Nothing depends on it: run `/setup`.
- **"leaf isn't set up in this server yet."** Expected until an admin
  completes [`/setup`](04-usage.md#first-setup).
- **"This prompt has expired. Run the command again."** You pressed a button
  or a menu on a prompt leaf has stopped waiting on: it was left for longer
  than it lasts (5 to 15 minutes, depending on the command), or leaf restarted
  since it sent it. Nothing was changed. Run the command again. (A prompt
  normally takes its own buttons away when its time is up; this is the answer
  for one whose buttons stayed on screen.)
- **"This form has expired, so what you entered wasn't saved."** The same,
  for a form (a day number, a timezone) that was left open past that time, or
  was open while leaf restarted. Run the command again.
- **"This interaction failed" on a button.** leaf didn't answer the press. The
  bot is offline or was restarting just then ([Bot offline](#bot-offline)), or
  it was a second press on a prompt that had just finished (the first press is
  the one that counted). Wait a moment; if the button is still there, press it
  again, otherwise run the command again. **Open gallery** on a card or on the
  how-to and **Turn off reminders** on a reminder don't expire, restarts
  included.
- **"Only the person who created this series can turn its reminders off."**
  Someone else pressed **Turn off reminders** on a reminder posted in a
  channel. Nothing changed.
- **No reaction on an archived post.** leaf needs **Add Reactions** and **Read
  Message History** in that channel. The archive result says so when it
  couldn't react, and `/setup` warns about series channels where a permission
  is missing.
- **Nothing in the log channel.** leaf can't post there. Run `/setup`: the form
  warns when leaf can't see or post in the chosen log channel. When you change
  the log channel, **Save** posts a test line there first and says what's wrong
  if it can't.
- **"…and 1 that leaf can no longer see", or "one leaf can no longer see
  (deleted, or hidden from leaf)".** A channel leaf has stored (a series
  channel, the log channel, a series' own channel) is no longer in the list
  Discord gives the bot: it was deleted, or it is hidden from leaf (Discord
  stops listing a channel the bot may not view). leaf can't tell which, and it
  doesn't mention such a channel, because Discord would show the mention as
  `#unknown`. For a server's series channels or its log channel, run `/setup`:
  the form drops the channel on **Save** (pick it again in the menu if it still
  exists and you want to keep it). For one series, its creator picks another
  channel in **Series settings**, in the gallery. leaf asks Discord for the
  channel list each time it writes such a message; the log says `channel list
  refused` or `channel list timed out` when it had to fall back on what the
  gateway told it.
- **A reminder never comes.** Reminders go out once per missing day, within 6
  hours of the chosen time. A DM needs the creator to allow direct messages
  from the server's members; a channel ping needs leaf to be able to post in
  the series channel. When a reminder is refused for one of those reasons, the
  creator sees it in the gallery: **Series settings** shows "leaf couldn't
  deliver your last reminder" with the reason. The log says why as well
  (`reminder refused`, `reminder skipped`, `reminder send failed`).

## Gallery

- **leaf isn't in the app list on my phone.** Most likely **iOS** and
  **Android** are not ticked under Activities → Supported Platforms; they are
  off by default ([02 § 5](02-discord.md#5-activities-the-gallery)). Otherwise
  the member may lack **Use Activities** in that channel, or their Discord app
  is out of date.
- **The gallery won't open in this channel**, but does in others. Likely causes,
  from other Activity developers' reports rather than Discord's documentation:
  the member can't **Send Messages** in that channel (a read-only or
  announcement channel), or lacks **Use Activities** there. Try a channel they
  can post in. The gallery also doesn't open in DMs: leaf needs a server.
- **`/gallery` or an Open gallery button answers "I couldn't open the gallery
  from here."** Discord refused to open it from that message, most likely for
  one of the reasons above. The answer spells out the app-launcher path, which
  still works where the member may use Activities.
- **Open gallery opened the gallery, but not on the series or day.** The
  destination is kept for two minutes and only for someone allowed to see that
  series. Opened later, or by someone else, the gallery starts where it
  normally would. The gallery asks leaf for the destination as it opens and
  waits about three seconds for it. If the answer is lost on the way, the
  gallery asks once more and leaf gives the same answer again (it remembers it
  for half a minute). On a connection slower than that, the gallery opens in
  its usual place: press the button again.
- **"The gallery can't open … for you."** An admin asked for a series the
  gallery wouldn't show them as a member (someone else's private series, a
  sprout, a revoked series). Manage Server doesn't change what the gallery
  shows; use the admin panel or `/search`
  ([04 § Opening the gallery](04-usage.md#opening-the-gallery)).
- **"Couldn't load the gallery": "leaf's bot isn't in this server, so the
  gallery can't open here."** Either Discord says the bot isn't a member of
  that server (it was never added, or it was removed: invite it with the
  install link, [02 § 7](02-discord.md#7-invite-the-bot)), or leaf has never
  seen the server: the bot was added while leaf was offline and hasn't
  connected since. leaf learns of a server when the bot is online in it, so
  check [Bot offline](#bot-offline). Once the bot is back, press **Try
  again**; after the bot was removed and added again it can take a minute.
- **The gallery stops on an error screen while opening.** Almost always the
  **three hosts not matching**
  ([02 § the three hosts](02-discord.md#the-three-hosts-must-match)): the URL
  mapping target, the OAuth redirect, and the Public URL must be the same
  origin. The error screen has a **Details** section with the reason.
- **"Your session has ended."** The gallery's sign-in ran out: it lasts 6 hours
  and renews itself while the gallery is in use, for at most 7 days. It can't
  sign in again without being reopened, so close leaf and open it again.
- **A day says "No file was saved for this day".** No file is stored for it: a
  day made by `/import`, or a migrated day whose original message was gone
  ([05](05-migration.md#reading-the-gaps-report)). `/status` lists these days
  under "No media stored". For a day made by `/import`, running `leaf-migrate`
  with the same file fetches the files if the original post still has them.
- **"This video didn't play."** Press **Try again** first: a dropped
  connection looks the same. If it never plays on that device, the format is
  the cause: leaf stores videos exactly as posted and doesn't convert them, so
  one the device can't decode (some `.mov` files, for example) won't play
  there. **Open original post** opens the original.
- **"This photo didn't load."** The connection dropped or the link went stale
  while the gallery was open. **Try again** loads it afresh.
- **Open original post shows a link to copy instead.** Discord didn't open the
  message link from inside the gallery. Copy the link the viewer shows and
  paste it into Discord. (Discord may first ask whether to leave; cancelling
  there does nothing, on purpose.)
- **Media is slow for everyone, every time.** Cloudflare is not caching it:
  check the cache rule with the `cf-cache-status` test in
  [03 § 4](03-cloudflare.md#4-cache-media-at-cloudflares-edge-recommended).

## Admin panel

- **Sign-in comes back to the sign-in card with a message.** The card says what
  happened; press **Sign in again** once the cause is dealt with.
  - *Sign-in cancelled*: you pressed Cancel on Discord's screen.
  - *That sign-in took too long*: more than 10 minutes passed between opening
    the sign-in and finishing it.
  - *Discord rejected the sign-in*: Discord wouldn't trade the sign-in code
    leaf was handed (the log has `admin oauth exchange failed`). A code works
    once, so reloading or going back to the callback page does it, and signing
    in again is enough. If it happens every time, check that the redirect the
    card names is listed under OAuth2 → Redirects in the portal; if it is, the
    client secret was reset since setup, so enter the new one with
    [`--reconfigure`](01-install.md#changing-credentials-later---reconfigure).
  - *Discord isn't answering*: leaf couldn't reach Discord. Try again shortly.
  - *No server to manage*: you don't have Manage Server in any server leaf is
    in, or you are signed in to Discord with another account. leaf knows a
    server once the bot has been online in it.
- **Discord shows "Invalid OAuth2 redirect_uri"** instead of its sign-in
  screen. The `…/admin/callback` redirect is missing in the portal, or it isn't
  on the same address as leaf's Public URL
  ([02 § 3](02-discord.md#3-oauth2-client-info-and-redirects)).
- **Signed out after a while.** A sign-in lasts one hour. Unsaved settings are
  kept and offered back after you sign in again.
- **Creator role or Log channel is a box asking for an ID.** leaf couldn't get
  the server's roles or channels from Discord just then. Press **Load the roles
  again** (or **channels**), or paste the ID
  ([04 § The admin web panel](04-usage.md#the-admin-web-panel)).
- **A setting is refused when saving.** The reason is under the field: a
  timezone leaf doesn't know, a role or channel that isn't in this server, or a
  number out of range. Nothing is saved until every field is accepted.

## Migration

- **`deferred > 0` in the summary.** Read one `fetch_deferred` row in the gaps
  report first: `401` and `403` mean the token or the bot's channel access is
  wrong and won't fix themselves; anything else is temporary, so **re-run** the
  same command ([05](05-migration.md#running-it-docker)).
- **Many `message_deleted` rows.** Originals are gone; they are kept as
  placeholders. If the number is unexpectedly high, confirm the bot still has
  **Read Message History** on the source channel(s).
- **A day made by `/import` still has no picture after a run.** The tool fills
  such a day only when the day still points at the same message and that
  message still has its files; otherwise it leaves the day as it is and counts
  it under `skipped`. If the message couldn't be fetched at all, the day is
  counted under `deferred` instead and has a `fetch_deferred` row in the gaps
  report: fix the cause and re-run.
- **The migrated series' settings won't open**, or commands say leaf isn't set
  up. `/setup` was never run in that server
  ([05 § Prerequisites](05-migration.md#prerequisites)).
- **New posts of the migrated series can't be archived** ("leaf doesn't archive
  from this channel"), and Series settings shows "Its current channel (no
  longer allowed)". The server's series channels don't include the series'
  channel: add it with `/setup`.
