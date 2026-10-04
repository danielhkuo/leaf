# 05 — Migration runbook (`leaf-migrate`)

`leaf-migrate` imports an old **walpurgisbot-v2** archive into leaf as a Series
with its posts and media, so leaf becomes the system of record and v2 can
retire. It ships in the same image (`/usr/local/bin/leaf-migrate`).

This is more than a JSON copy: v2 stored **Discord CDN URLs, which expire**, so
the tool re-fetches each original message from Discord (while it still exists)
and re-uploads the bytes through leaf's media pipeline into R2.

## What it does, precisely

Per source post it writes a leaf post under the target series, then for media:

- **Message fetched OK** → each attachment is downloaded and stored in R2
  (original + thumbnail); the caption comes from the live message.
- **Message returns 404 (deleted)** → the post is still written, media marked
  `media_missing=true`, with the **attachment id recovered from the expired CDN
  URL** (the gallery shows a placeholder; the proxy already handles missing
  media).
- **Any other fetch error** (a network failure, a rate limit that doesn't
  clear, and also a rejected token or a channel the bot can't read) → the day
  is **deferred**: left unwritten, so a later re-run retries it instead of
  recording recoverable files as missing.

It is **idempotent**: days that already hold their files are skipped and each
day is committed in its own transaction. **Re-running is the resume mechanism**:
stop it, run it again, and it continues where it stopped and retries deferred
days. There is no checkpoint file.

Four things to know before you start:

- **The series.** If no series with `--series-name` exists in the server, the
  tool creates one: public, daily, owned by `--creator`, already past any
  sprout stage, with its first day set to the lowest day in the source. If one
  exists, it is reused, whoever owns it.
- **Existing days that hold a file are never touched. Days with no stored file
  are tried again.** A day created by the bot's `/import` command, or by an
  earlier run in which every download failed, has no stored file. For such a
  day the tool fetches the source message again and, if the day still points
  at that same message and at least one file can be stored now, replaces the
  day with one that has its media (the summary counts it as `repaired`; its
  date stays). In every other case the day is left exactly as it is: the
  message was deleted, none of its files could be downloaded, or someone has
  since archived a post as that day themselves. If the message couldn't be
  fetched at all (wrong token, no access to the channel, Discord down), the
  day is counted as `deferred` and listed in the gaps report, so you know a
  re-run after fixing that will fill it.
- **Leave the series alone while the tool runs.** The bot can stay online, but
  ask the creator not to archive, replace or remove days of this series until
  the run has finished. The tool never overwrites a day that got its files in
  the meantime, but two writers on one day make the result harder to read.
- **One message, several days.** If the old archive recorded the same message
  under more than one day, each of those days gets **all** of that message's
  attachments.

## Prerequisites

- **leaf is already set up** ([01](01-install.md)–[03](03-cloudflare.md)): the
  tool reads R2 and bot credentials from `leaf.conf` and writes the same `leaf.db`
  the bot reads.
- **`/setup` has been run in the target server**
  ([04](04-usage.md#first-setup)), and its **Series channels include the
  channel(s) the old posts are in**. The tool does neither for you. Without
  `/setup` the migrated series shows in the gallery, but the archive and
  look-up commands answer "leaf isn't set up in this server yet" and the
  series' settings can't be opened.
  Without the channels, new posts there can't be archived ("leaf doesn't
  archive from this channel"), and Series settings shows the series' channel
  as "Its current channel (no longer allowed)". The other settings still
  save.
- The **bot is still in the source server with Read Message History** on the
  archive channel(s) — re-fetching needs the original messages to exist and be
  readable. **Do the migration before retiring v2's access.**
- The **source archive**: either the v2 **SQLite database file**, or a v2 **JSON
  export** (`/export` from v2). The format is auto-detected.
- IDs you'll pass (enable **Developer Mode**,
  [02 § 8](02-discord.md#8-enable-developer-mode-to-copy-ids), and Copy ID):
  the **guild ID** and the **creator's user ID** (Johan's).

## Flags

```
--from <PATH>        v2 SQLite DB or JSON export (auto-detected by content)
--to <PATH>          target leaf SQLite DB (created if absent, dry run included)
--guild <ID>         target guild snowflake
--creator <ID>       creator/owner snowflake for the imported series
--series-name <STR>  series to create or reuse (e.g. "Daily Johan")
--channel <ID>       the series' channel (default: every channel seen in source)
--day-offset <N>     added to every v2 day number (default 0)
--dry-run            count what would be imported; contacts nothing (see below)
--gaps-report <PATH> write a Markdown follow-up table
--config <PATH>      leaf.conf location (default $DATA_DIR/leaf.conf)
--fetch-delay-ms <N> politeness delay between Discord fetches (default 250)
```

Run `leaf-migrate --help` to confirm these against your build.

Pass `--channel` when the old posts are spread over several channels: it names
the one channel the series belongs to from now on (the one you'll keep posting
in). Without it the series is tied to every source channel, and its settings in
the gallery show the first of them when sorted by ID.

## Running it (Docker)

Run it as a one-off container that shares leaf's `/data` volume (so `--to` and
`--config` point at the real database and config), bind-mounting your source
file in:

```sh
# DRY RUN FIRST: no days are written; it prints counts and a gaps report.
docker compose run --rm \
  -v /path/to/walpurgis.db:/import/source.db:ro \
  --entrypoint /usr/local/bin/leaf-migrate \
  leaf \
  --from /import/source.db \
  --to   /data/leaf.db \
  --config /data/leaf.conf \
  --guild <GUILD_ID> \
  --creator <CREATOR_USER_ID> \
  --series-name "Daily Johan" \
  --dry-run \
  --gaps-report /data/migrate-gaps.md
```

`docker compose run` mounts the service's `leaf-data` volume at `/data`
automatically; `-v` adds your source file. For a JSON export, mount the `.json`
and point `--from` at it instead.

### What the dry run checks, and what it doesn't

The dry run reads the source file and the target database, and nothing else. It
logs `total`, `would_import`, `would_retry_without_media` (existing days with
no stored file, which the real run will try to fill), `already_present` and
`predicted_gaps`, and the gaps report lists the source days that have no media
links at all.

It does **not** contact Discord or R2, and it doesn't read `leaf.conf`. So it
can't tell you whether the bot token works, whether the bot can read the source
channel, which messages have been deleted, or whether R2 accepts uploads. Those
only show up in the real run. It does open the `--to` database, creating the
file if the path doesn't exist yet, so check that path. Against the real
`/data/leaf.db` it writes no series and no days.

When the counts look right, **drop `--dry-run`** to do the real import:

```sh
docker compose run --rm \
  -v /path/to/walpurgis.db:/import/source.db:ro \
  --entrypoint /usr/local/bin/leaf-migrate \
  leaf \
  --from /import/source.db --to /data/leaf.db --config /data/leaf.conf \
  --guild <GUILD_ID> --creator <CREATOR_USER_ID> \
  --series-name "Daily Johan" \
  --gaps-report /data/migrate-gaps.md
```

The run logs nothing per day and can sit silent for several minutes (it waits
250 ms between messages by default); it is working. The summary at the end logs
`imported`, `repaired`, `skipped`, `deferred`, `media_stored`, `media_missing`,
and `gaps`.

If `deferred > 0`, look at one `fetch_deferred` row in the gaps report before
running it again:

- `message fetch returned 401 Unauthorized`: the bot token in `leaf.conf` is
  wrong or was reset. Fix it
  ([01 § `--reconfigure`](01-install.md#changing-credentials-later---reconfigure)).
- `message fetch returned 403 Forbidden`: the bot can't read that channel. Give
  it **View Channel** and **Read Message History** there.
- Anything else (`request failed`, `rate limited`): temporary. **Run it again.**

Re-running repeats the first two until the cause is fixed.

> SQLite WAL makes a run safe even while the bot container is up, but for a clean
> cutover prefer running the real import during the shadow window below.

## Reading the gaps report

`--gaps-report` writes a Markdown table; each row's `reason` is one of:

| reason | meaning | action |
| --- | --- | --- |
| `message_deleted` | source message is gone; media recorded as missing (ids recovered from the old URLs) | expected for deleted posts; nothing to do |
| `media_unfetchable` | message fetched but an attachment couldn't be downloaded or stored (the detail says which) | the day is written with a placeholder for that file. If **no** file of the day was stored, a re-run tries the day again. If some were, the re-run skips it: to retry, `/delete` that day in Discord, then re-run |
| `fetch_deferred` | the message couldn't be fetched; the day was **not** written (or, for a day that had no stored file, not filled in) | read the detail (above), then **re-run** |
| `no_media_recovered` | message gone and the source had no media URLs to recover | nothing recoverable; informational |

## Verify

After the real run:

1. Open the gallery ([04 § gallery](04-usage.md#for-viewers-the-gallery)) and
   spot-check about 20 migrated days: thumbnail in the calendar, full image in
   the day viewer, caption, and **Open original post**. Confirm the migrated day
   **count matches v2** minus the documented gaps (`/status` gives the count and
   the missing day numbers).
2. As the series' creator, open **Series settings** from the gear on the series'
   page. If it won't open, `/setup` hasn't been run. If the **Channel** field
   reads "Its current channel (no longer allowed)", `/setup` doesn't list the
   series' channel (see Prerequisites).
3. Archive one new post with **Archive to Series** to confirm the series takes
   posts in its channel.

## Cutover sequence (recommended)

1. **Dry-run against a copy** of the production v2 DB; review the plan + gaps.
2. **Real run** into the live leaf DB; re-run until `deferred = 0`.
3. **Shadow week**: v2 still primary. leaf doesn't watch channels, so archive
   each new post in leaf as well (**Archive to Series**) and compare that
   nothing is missed.
4. **Swap** — stop v2; leaf is now primary.
5. Keep the v2 export as a **cold backup**; tag `v1.0`.

> Stopping v2 and the announcement are operational steps — yours to run when the
> shadow week looks clean.
