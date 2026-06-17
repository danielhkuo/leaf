# 02 — Discord application setup

This produces the three Discord credentials the setup form needs (**bot token**,
**application ID**, **client secret**) and wires the application so the bot, the
OAuth logins, and the embedded-app gallery all work.

Everything here happens in the **Discord Developer Portal**:
<https://discord.com/developers/applications>.

> ⚠️ **[VERIFY] Discord portal layout** — Discord reorganizes the developer
> portal's tabs and labels regularly (the "Installation", "Activities", and
> "OAuth2" sections have all moved/renamed before). The tab names and paths
> below were accurate at writing; if a tab isn't where described, look for the
> equivalently-named section. Every portal step here is a thing to confirm.

<!-- screenshot: the application list / "New Application" button -->

## 1. Create the application

**New Application** → name it (e.g. "leaf") → create. You're now in the app's
settings. The credentials below all belong to this one application.

## 2. Bot: token + the message-content intent

Open the **Bot** tab.

- **Token** — click **Reset Token** and copy it. This is the **Bot token** for
  the setup form. Discord shows it **once**; if you lose it, reset again.
- **Privileged Gateway Intents** — enable **Message Content Intent**. leaf uses
  it for the optional passive watcher (`crates/leaf-bot/src/lib.rs` requests
  `GUILDS | GUILD_MESSAGES | MESSAGE_CONTENT`). The other two intents are not
  privileged and need no toggle.

> ⚠️ **[VERIFY] Bot tab + intent location** — confirm where "Reset Token" and
> the "Message Content Intent" toggle live in the current Bot tab.

> ⚠️ **[VERIFY] Privileged-intent gating** — for apps in **many** servers
> (historically 100+) Discord requires **verification + intent approval** before
> privileged intents work. A self-hosted leaf in a handful of servers is
> typically under that threshold, but confirm the current rule and whether your
> app needs to request Message Content access.

<!-- screenshot: Bot tab showing Reset Token and the intents toggles -->

## 3. OAuth2: client info + redirects

Open the **OAuth2** tab.

- **Application (client) ID** — copy it (this is the **Application ID** field in
  the setup form; it's also shown on the General Information tab).
- **Client secret** — **Reset Secret** and copy it (the **OAuth client secret**
  field). Shown once.
- **Redirects** — add **both** of these, using your real hostname:
  - `https://leaf.example.com` — the gallery's OAuth token exchange.
  - `https://leaf.example.com/admin/callback` — the admin panel login.

> ⚠️ **[VERIFY] OAuth2 tab** — confirm the locations of "Client information",
> "Reset Secret", and the "Redirects" list in the current OAuth2 tab.

leaf's gallery requests the `identify` scope (server-side it checks guild
membership with the bot token, so the user grants nothing extra); the admin
panel login additionally requests `guilds` to find which servers you manage.
You don't configure scopes here — they're requested at login time — but the
**redirect URLs above must exist** or those logins fail.

<!-- screenshot: OAuth2 redirects list with both URLs -->

## 4. Activities: enable the embedded app

The gallery is a Discord **Activity** (embedded app). Find the **Activities**
section and **enable** Activities for this app, choosing the platforms you want
(desktop / mobile).

> ⚠️ **[VERIFY] Activities enablement** — whether Activities are on by default,
> behind a toggle, or gated by eligibility requirements (a dev team, accepting
> developer/monetization terms, etc.) has changed over time. Confirm the current
> requirements and where the enable switch is.

<!-- screenshot: Activities settings / enable toggle -->

### URL mapping

Discord serves the Activity through its own proxy (`discordsays.com`) and needs
to know where to fetch your content. In the Activity's **URL Mappings**, add:

- **Prefix** `/` → **Target** `leaf.example.com` (host only — **no** `https://`,
  no trailing slash).

This single mapping covers the app, the API, and the media proxy (all one
origin).

> ⚠️ **[VERIFY] URL Mappings UI** — confirm where URL Mappings live and the
> exact prefix/target format (host vs full URL) the current portal expects.

## 5. Entry Point launch command (recommended)

Activities launched from a **voice channel** post "Game Invitation / Game ended"
cards in chat. The gallery is a solo, contemplative view, so the nicer entry is
the **Entry Point command**: when you enable Activities, Discord auto-provisions a
default `PRIMARY_ENTRY_POINT` command whose handler is the built-in
`DiscordLaunchActivity` (*"let Discord launch the activity"*) — no bot code
involved. Launch the gallery from that command (or the app launcher) rather than
the voice-channel shelf, and the invite cards don't appear.

> ⚠️ **[VERIFY] Entry Point command** — confirmed auto-provisioned and
> Discord-handled (2026-06). Confirm where it appears, that you can rename it, and
> that its handler stays **Discord-handled** (not "app handles the interaction").

## 6. Install (invite) the bot to your server

Generate an invite with the right scopes and install it.

> ⚠️ **[VERIFY] Install/URL Generator** — recent portals have an **Installation**
> tab (Install Link / default install settings) that may supersede the older
> **OAuth2 → URL Generator**. Use whichever your portal presents.

Required when generating the invite:

- **Scopes:** `bot` and `applications.commands`.
- **Bot permissions:** enough to read and post in the watched channels and add
  reactions — at minimum **View Channels**, **Send Messages**, **Embed Links**,
  **Attach Files**, **Add Reactions**, and **Read Message History**.

Open the generated URL, pick your server, authorize. On join, leaf posts a short
greeting prompting an admin to run `/setup` (it uses the server's system
channel, or the top-most channel it can speak in). Continue to
[04-usage.md](04-usage.md) for that.

<!-- screenshot: URL Generator / Installation scopes + permissions -->

## 7. Enable Developer Mode (to copy IDs)

You'll need raw IDs later — your **guild (server) ID**, channel IDs, and your
**user ID** (for the migration in [05-migration.md](05-migration.md), and for
`DEV_GUILD_ID` if you test). Turn on Developer Mode, then right-click a
server / channel / user → **Copy ID**.

> ⚠️ **[VERIFY] Developer Mode** — desktop: **User Settings → Advanced →
> Developer Mode**; mobile: **User Settings → Appearance** (confirmed 2026-06).
> "Copy ID" (right-click / long-press) appears only once it's on.

## The three hosts must match

This trips everyone up. These three must name the **same origin**:

1. **Public URL** in the setup form (e.g. `https://leaf.example.com`).
2. The **OAuth2 redirect** base (`https://leaf.example.com` and its
   `/admin/callback`).
3. The **URL mapping target** (`leaf.example.com`).

If they disagree, the gallery's token exchange or the admin login will fail with
opaque OAuth errors. Set up your hostname in
[03-cloudflare.md](03-cloudflare.md), then use that exact host everywhere.

→ Next: **[03-cloudflare.md](03-cloudflare.md)** for the hostname and R2 media
storage.
