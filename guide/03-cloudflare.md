# 03 — Cloudflare: public hostname + R2 media

Cloudflare does two jobs for leaf. The tunnel is free, and R2 is free up to
10 GB of stored media ([what R2 costs](#what-r2-costs)):

1. **Tunnel** — publishes the container at `https://leaf.example.com` with no
   port forwarding and no exposed home IP.
2. **R2** — S3-compatible object storage for all archived media (originals +
   thumbnails). This is where the **S3 endpoint / bucket / access keys** for the
   setup form come from. If you chose **A folder on this machine** instead
   ([01 § Storage](01-install.md#storage-r2-or-a-folder-on-this-machine)), skip
   section 3; the tunnel and the cache rule still apply.

Everything here is in the **Cloudflare dashboard** (<https://dash.cloudflare.com>).

## 1. Add your domain

Add your domain as a zone in Cloudflare and complete nameserver delegation at
your registrar so the zone is **Active**. You need this before a Tunnel hostname
or DNS records will resolve.

## 2. Publish leaf with a Cloudflare Tunnel

This guide uses the **bundled `cloudflared` sidecar** in `docker-compose.yml`
(the `tunnel` profile). It dials out to Cloudflare, so nothing inbound is opened.

1. Create a tunnel and copy its **connector token**. As of writing the tunnel
   screens are under **Zero Trust → Networks → Connectors → Cloudflare Tunnels →
   Create a tunnel** (choose the "Cloudflared" connector). The connector token is
   the long token in the `cloudflared ... run <TOKEN>` example, not a tunnel UUID.

2. Add a **public hostname** (the tab is labeled **Published application routes**;
   select the tunnel → **Edit**) routing your domain to the container:

   | Field | Value |
   | --- | --- |
   | Subdomain / Domain | `leaf` / `example.com` (→ `leaf.example.com`) |
   | Service type | `HTTP` |
   | URL | `leaf:3777` (the compose service name + port) |

   The service target is `http://host:port`. On the compose network the host is
   the service name `leaf`; outside compose, use the reachable host/IP.

3. Put the token in a `.env` next to `docker-compose.yml` and start the sidecar:

   ```sh
   echo 'TUNNEL_TOKEN=eyJ...your-connector-token...' >> .env
   docker compose --profile tunnel up -d
   ```

   The `cloudflared` service exits immediately if `TUNNEL_TOKEN` is unset — that
   blank-token crash is expected, not a leaf bug.

Keep the DNS record Cloudflare creates for the tunnel **proxied (orange cloud
ON)**. Proxying alone does not make Cloudflare cache leaf's media; the cache
rule in [§ 4](#4-cache-media-at-cloudflares-edge-recommended) does.

> 💡 **Alternative: your own reverse proxy.** If you already run one (nginx proxy
> manager, Caddy, Traefik), point `leaf.example.com` → `http://127.0.0.1:3777`
> with TLS, and a **proxied** Cloudflare DNS record at it instead of the tunnel.
> Don't force `X-Frame-Options: DENY` on this host (it must load in Discord's
> iframe).

## 3. Create the R2 bucket and API token

leaf stores every archived original plus a generated thumbnail in R2, and serves
them through its own signed media route. (Discord's own attachment links expire
after about a day, so an archive can't just keep the links.)

1. **Enable R2** on your account. A **payment method on file is required** to
   activate R2, even while your usage stays within the free tier.

2. **Create a bucket** (any name, e.g. `leaf-media`). This is the **Bucket**
   field in the setup form.

3. **Find your S3 endpoint** on **R2 → Overview**. It has the form
   `https://<account-id>.r2.cloudflarestorage.com`. This is the **S3 endpoint**
   field.

4. **Create a scoped API token** at **R2 → Manage R2 API Tokens**, with **Object
   Read & Write** permission scoped to the bucket. Creating it reveals an
   **Access Key ID** and a **Secret Access Key** — these are the matching setup
   fields (the secret is shown once).

You now have all four R2 values plus the public hostname. Return to
[01-install.md § First-run setup](01-install.md#first-run-setup) and complete the
form.

## 4. Cache media at Cloudflare's edge (recommended)

Archived media never changes, and leaf sends
`Cache-Control: public, max-age=31536000, s-maxage=<seconds>, immutable` on
every media response: a browser may keep its copy for a year, and a cache
shared between people (`s-maxage`) only until the link expires, which is at
most two days away. Cloudflare still won't cache it by default: it decides
what to cache from the file extension in the URL, and leaf's media URLs have
none (`/api/media/<id>?exp=…&sig=…`). Without a rule every media request goes
to your server and on to R2.

Add one **Cache Rule** (zone → **Caching → Cache Rules → Create rule**):

| Setting | Value |
| --- | --- |
| When incoming requests match | **Hostname** equals `leaf.example.com` **and** **URI Path** starts with `/api/media/` |
| Expression (the same thing, for the editor) | `(http.host eq "leaf.example.com" and starts_with(http.request.uri.path, "/api/media/"))` |
| Cache eligibility | **Eligible for cache** |
| Edge TTL | **Ignore cache-control header and use this TTL**, set to **1 day** |
| Status code TTL (below Edge TTL, if shown) | Add one: **Greater than or equal** `500`, duration **No store**, so an error from a storage hiccup is never kept. |
| Cache key | leave the default. **Do not** ignore the query string: `exp` and `sig` are what make a media link private. |

**Set the Edge TTL yourself; don't pick "Use cache-control header".** leaf
refuses a media link one to two days after issuing it, and stops serving a
deleted day's file at once. Cloudflare answers from its copy without asking
leaf, so the Edge TTL is how long a link to a deleted file can keep working.
Following leaf's header would keep the copy until the link expires, up to two
days; a fixed day is shorter. It costs almost nothing in cache hits: the links
change every day anyway.

Check it. Open the gallery in Discord in a browser (discord.com), open the
browser's developer tools, and copy the address of any thumbnail request: it
looks like `https://<application id>.discordsays.com/api/media/…?thumb=1&exp=…&sig=…`.
Swap the host for your own and ask for it twice:

```sh
curl -sI 'https://leaf.example.com/api/media/<id>?thumb=1&exp=<exp>&sig=<sig>' | grep -i cf-cache-status
```

`MISS` and then `HIT` means the rule works. `DYNAMIC` means the request didn't
match the rule (check the hostname and path in it).

What this buys you, and what it costs, honestly:

- Media links are signed per UTC day, so the address of a picture (and with it
  the cache entry) changes daily. The first request for each file each day
  still travels Cloudflare → your server → R2. The rule helps the second and
  later viewers that day; each device's own browser cache already covers a
  person looking at the same day twice.
- The cost: Cloudflare can keep answering a link for up to a day after leaf
  itself would refuse it. Someone who already holds the link to a deleted
  day's file, or to media in a revoked or newly private series, can reach it
  for up to a day longer than without the rule. To cut that short, empty the
  cache: **Caching → Configuration → Purge Everything** (it clears the whole
  domain's cache; the only effect is a few slower first requests).
- leaf answers byte-range requests and sends `Content-Length`, so Cloudflare
  can serve a video's seeks from its cached copy.
- **Tiered Cache** (one toggle under Caching) makes Cloudflare's regional
  caches ask an upper tier before your server. Worth turning on if viewers are
  spread around the world; optional otherwise.

### What R2 costs

R2's free tier covers 10 GB of storage and millions of reads a month. leaf's
reads stay inside it with or without the cache rule at the size of a Discord
server. **Storage is the number to watch.** leaf keeps every original at full
size, up to 100 MB per file, plus a small thumbnail, and it has no quota and no
usage display. A photo a day is a few hundred megabytes a year; a series of
phone videos can pass 10 GB in months. Past the free tier R2 bills storage by
the gigabyte-month (see Cloudflare's R2 pricing page for the current rate).
Check the bucket's size under **R2 → your bucket → Metrics** now and then.

### Keep the bucket private

leaf always serves media through its own `/api/media/…` route, which checks a
signed link before reading R2. Don't enable the bucket's public `r2.dev` address
or attach a public custom domain to it: leaf would not use either, and both
would expose every stored file, private series included, to anyone with the
link.

→ Next: finish [01-install.md § First-run setup](01-install.md#first-run-setup),
then [04-usage.md](04-usage.md).
