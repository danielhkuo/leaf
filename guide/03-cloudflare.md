# 03 — Cloudflare: public hostname + R2 media

Cloudflare does two jobs for leaf, both on the free tier:

1. **Tunnel** — publishes the container at `https://leaf.example.com` with no
   port forwarding and no exposed home IP.
2. **R2** — S3-compatible object storage for all archived media (originals +
   thumbnails). This is where the **S3 endpoint / bucket / access keys** for the
   setup form come from.

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
   | URL | `leaf:8080` (the compose service name + port) |

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
ON)** — that's what puts media behind Cloudflare's edge cache.

> 💡 **Alternative: your own reverse proxy.** If you already run one (nginx proxy
> manager, Caddy, Traefik), point `leaf.example.com` → `http://127.0.0.1:8080`
> with TLS, and a **proxied** Cloudflare DNS record at it instead of the tunnel.
> Don't force `X-Frame-Options: DENY` on this host (it must load in Discord's
> iframe).

## 3. Create the R2 bucket and API token

leaf stores every archived original plus a generated thumbnail in R2, and serves
them through its own signed media proxy (Discord CDN URLs expire, so raw URLs
can't be stored — PLAN.md § Media storage).

1. **Enable R2** on your account. A **payment method on file is required** to
   activate R2, though leaf's usage stays within the free tier.

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

## 4. Keep reads free: caching (recommended)

Archived media is immutable, and leaf already sends
`Cache-Control: public, max-age=31536000, immutable` on every media response.
Two settings under the zone's **Caching** section make Cloudflare's edge honor
that, so repeat views don't bill R2 reads:

- **Cache Rule** — ensure media responses under your origin are cached at the
  edge (they're cacheable by default with the immutable header; a cache rule
  makes it explicit/robust).
- **Tiered Cache** — one toggle; regional caches check an upper tier before
  hitting R2, so a global audience costs ~one read instead of one per region.

With the free tier (10 GB storage, ~1 write/day) and cached reads, leaf's R2 bill
is effectively zero.

### Optional: serve media direct from R2

For lower latency/bandwidth you can later attach an R2 **custom domain** (on the
bucket, under **Settings → Public Access → Custom Domains** → Connect domain) and
add it to the Discord URL mappings, offloading image bytes from leaf-server. This
is an optimization, not required for v1.

→ Next: finish [01-install.md § First-run setup](01-install.md#first-run-setup),
then [04-usage.md](04-usage.md).
