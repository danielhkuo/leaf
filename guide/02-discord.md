# 02 — Discord application setup

This produces the three Discord credentials the setup form needs (**bot token**,
**application ID**, **client secret**) and wires the application so the bot, the
OAuth logins, and the embedded-app gallery all work.

Everything here happens in the **Discord Developer Portal**:
<https://discord.com/developers/applications>.

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

Once an app reaches 100+ servers Discord requires verification and intent
approval before privileged intents keep working; a self-hosted leaf in a handful
of servers stays under that threshold.

## 3. OAuth2: client info + redirects

Open the **OAuth2** tab.

- **Application (client) ID** — copy it (this is the **Application ID** field in
  the setup form; it's also shown on the General Information tab).
- **Client secret** — **Reset Secret** and copy it (the **OAuth client secret**
  field). Shown once.
- **Redirects** — add **both** of these, using your real hostname:
  - `https://leaf.example.com` — the gallery's OAuth token exchange.
  - `https://leaf.example.com/admin/callback` — the admin panel login.

leaf's gallery requests the `identify` scope (server-side it checks guild
membership with the bot token, so the user grants nothing extra); the admin
panel login additionally requests `guilds` to find which servers you manage.
You don't configure scopes here — they're requested at login time — but the
**redirect URLs above must exist** or those logins fail.

## 4. Activities: enable the embedded app

The gallery is a Discord **Activity** (embedded app). Find the **Activities**
section and **enable** Activities for this app, choosing the platforms you want
(desktop / mobile). Some accounts have to accept developer terms first.

### URL mapping

Discord serves the Activity through its own proxy (`discordsays.com`) and needs
to know where to fetch your content. In the Activity's **URL Mappings**, add:

- **Prefix** `/` → **Target** `leaf.example.com` (host only — **no** `https://`,
  no trailing slash).

This single mapping covers the app, the API, and the media proxy (all one
origin).

## 5. Entry Point launch command (recommended)

Activities launched from a **voice channel** post "Game Invitation / Game ended"
cards in chat. The gallery is a solo, contemplative view, so the nicer entry is
the **Entry Point command**: when you enable Activities, Discord auto-provisions a
default `PRIMARY_ENTRY_POINT` command whose handler is the built-in
`DiscordLaunchActivity` ("let Discord launch the activity") — no bot code
involved. Launch the gallery from that command (or the app launcher) rather than
the voice-channel shelf, and the invite cards don't appear. Leave its handler set
to Discord-handled, not "app handles the interaction".

## 6. Install (invite) the bot to your server

Generate an invite and install it. Recent portals do this from the
**Installation** tab (Install Link / default install settings); older ones use
**OAuth2 → URL Generator**. Either way you need:

- **Scopes:** `bot` and `applications.commands`.
- **Bot permissions:** enough to read and post in the watched channels and add
  reactions — at minimum **View Channels**, **Send Messages**, **Embed Links**,
  **Attach Files**, **Add Reactions**, and **Read Message History**.

Open the generated URL, pick your server, authorize. On join, leaf posts a short
greeting prompting an admin to run `/setup` (it uses the server's system
channel, or the top-most channel it can speak in). Continue to
[04-usage.md](04-usage.md) for that.

## 7. Enable Developer Mode (to copy IDs)

You'll need raw IDs later — your **guild (server) ID**, channel IDs, and your
**user ID** (for the migration in [05-migration.md](05-migration.md), and for
`DEV_GUILD_ID` if you test). Enable Developer Mode (desktop: **User Settings →
Advanced → Developer Mode**; mobile: **User Settings → Appearance**), then
right-click / long-press a server / channel / user → **Copy ID**.

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
