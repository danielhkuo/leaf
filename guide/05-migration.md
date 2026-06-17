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
- **Transient/unknown fetch error** → the day is **deferred** (left unwritten)
  so a later re-run retries it, rather than freezing recoverable bytes as
  missing.

It is **idempotent**: already-imported days are skipped and each day is committed
in its own transaction. **Re-running is the resume mechanism** — kill it, run it
again, it continues where it stopped and retries deferred days. (There is no
checkpoint file by design.)

## Prerequisites

- **leaf is already set up** ([01](01-install.md)–[03](03-cloudflare.md)): the
  tool reads R2 + bot credentials from `leaf.conf` and writes the same `leaf.db`
  the bot reads.
- The **bot is still in the source server with Read Message History** on the
  archive channel(s) — re-fetching needs the original messages to exist and be
  readable. **Do the migration before retiring v2's access.**
- The **source archive**: either the v2 **SQLite database file**, or a v2 **JSON
  export** (`/export` from v2). The format is auto-detected.
- IDs you'll pass (enable **Developer Mode** —
  [02 § 7](02-discord.md#7-enable-developer-mode-to-copy-ids) — and Copy ID):
  the **guild ID** and the **creator's user ID** (Johan's).

## Flags

```
--from <PATH>        v2 SQLite DB or JSON export (auto-detected by content)
--to <PATH>          target leaf SQLite DB (created + migrated if absent)
--guild <ID>         target guild snowflake
--creator <ID>       creator/owner snowflake for the imported series
--series-name <STR>  series to create or reuse (e.g. "Daily Johan")
--channel <ID>       series watched channel (default: channels seen in source)
--day-offset <N>     added to every v2 day number (default 0)
--dry-run            read + print the plan; write nothing
--gaps-report <PATH> write a Markdown follow-up table
--config <PATH>      leaf.conf location (default $DATA_DIR/leaf.conf)
--fetch-delay-ms <N> politeness delay between Discord fetches (default 250)
```

Run `leaf-migrate --help` to confirm these against your build.

## Running it (Docker)

Run it as a one-off container that shares leaf's `/data` volume (so `--to` and
`--config` point at the real database and config), bind-mounting your source
file in:

```sh
# DRY RUN FIRST — writes nothing, just shows the plan + a gaps report.
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

When the plan looks right, **drop `--dry-run`** to do the real import:

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

The summary logs `imported`, `skipped`, `deferred`, `media_stored`,
`media_missing`, and `gaps`. If `deferred > 0`, **just run it again** to retry
those days.

> SQLite WAL makes a run safe even while the bot container is up, but for a clean
> cutover prefer running the real import during the shadow window below.

## Reading the gaps report

`--gaps-report` writes a Markdown table; each row's `reason` is one of:

| reason | meaning | action |
| --- | --- | --- |
| `message_deleted` | source message is gone; media recorded as missing (ids recovered from the old URLs) | expected for deleted posts; nothing to do |
| `media_unfetchable` | message fetched but an attachment couldn't be downloaded/stored | check that attachment manually; re-run won't retry an already-written day |
| `fetch_deferred` | transient fetch error; the day was **not** written | **re-run** to retry |
| `no_media_recovered` | message gone and the source had no media URLs to recover | nothing recoverable; informational |

## Verify

After the real run, open the gallery ([04 § gallery](04-usage.md#for-viewers--the-gallery))
and spot-check ~20 migrated days: thumbnail in the heatmap, full image in the day
viewer, caption, and "jump to original message". Confirm the migrated day **count
matches v2** minus the documented gaps.

## Cutover sequence (recommended)

1. **Dry-run against a copy** of the production v2 DB; review the plan + gaps.
2. **Real run** into the live leaf DB; re-run until `deferred = 0`.
3. **Shadow week** — leaf live and watching, v2 still primary, comparing that
   nothing is missed.
4. **Swap** — stop v2; leaf is now primary.
5. Keep the v2 export as a **cold backup**; tag `v1.0`.

> Stopping v2 and the announcement are operational steps — yours to run when the
> shadow week looks clean.
