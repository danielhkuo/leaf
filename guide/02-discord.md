# 02 — Discord application setup

This produces the three Discord credentials the setup form needs (**bot token**,
**application ID**, **client secret**) and configures the application so the
bot, the sign-ins and the gallery work, on phones as well as on desktop.

Everything here happens in the **Discord Developer Portal**:
<https://discord.com/developers/applications>.

The short version, to tick off as you go:

- [ ] Bot token copied; no privileged intents needed ([§ 2](#2-bot-token))
- [ ] Application ID and client secret copied; **both** redirects added ([§ 3](#3-oauth2-client-info-and-redirects))
- [ ] Only **Guild Install** ticked ([§ 4](#4-installation-guild-install-only))
- [ ] Activities enabled, with **iOS** and **Android** ticked, an orientation choice made, and the URL mapping added ([§ 5](#5-activities-the-gallery))
- [ ] Members can use it: **Use Application Commands**, **Use Activities**, **Send Messages** ([§ 6](#6-what-members-need))
- [ ] Bot invited to your server ([§ 7](#7-invite-the-bot))

## 1. Create the application

**New Application** → name it (e.g. "leaf") → create. You're now in the app's
settings. The credentials below all belong to this one application.

## 2. Bot: token

Open the **Bot** tab.

- **Token**: click **Reset Token** and copy it. This is the **Bot token** for
  the setup form. Discord shows it **once**; if you lose it, reset again.
- **Privileged Gateway Intents**: leave all three off (Presence, Server
  Members, Message Content).

leaf connects with the `GUILDS` intent only (`crates/leaf-bot/src/lib.rs`),
which is not privileged. It never reads channel messages on its own: archiving
gets the message from the command you run on it. If you turned **Message
Content Intent** on for an earlier version of leaf, you can turn it off again.

## 3. OAuth2: client info and redirects

Open the **OAuth2** tab.

- **Application (client) ID**: copy it (the **Application ID** field in the
  setup form; it's also on the General Information tab).
- **Client secret**: **Reset Secret** and copy it (the **OAuth client secret**
  field). Shown once.
- **Redirects**: add **both** of these, using your real hostname:
  - `https://leaf.example.com` for the gallery's sign-in.
  - `https://leaf.example.com/admin/callback` for the admin panel's sign-in.

You don't configure scopes here; leaf asks for them when someone signs in. The
gallery asks for `identify` only (leaf checks server membership itself, with
the bot token). The admin panel asks for `identify` and `guilds`, to find the
servers you manage. The **redirects above must exist** or those sign-ins fail.

## 4. Installation: Guild Install only

Open the **Installation** tab.

- **Installation Contexts**: keep **Guild Install** ticked and untick **User
  Install** (new applications may have both ticked). leaf only works in servers
  the bot has joined: it checks membership and reads channels with the bot's own
  access. Its commands are registered for server installs only.
- **Install Link**: choose the Discord-provided link, and under **Default
  Install Settings → Guild Install** set:
  - **Scopes:** `bot` and `applications.commands`.
  - **Permissions:** **View Channels**, **Send Messages**, **Embed Links**,
    **Attach Files**, **Add Reactions**, **Read Message History**.

Older portals have no Installation tab; build the same link under **OAuth2 → URL
Generator** with those scopes and permissions.

## 5. Activities: the gallery

The gallery is a Discord **Activity** (an embedded app). Open the **Activities**
section and work through these. The portal's platform and orientation settings
can't be read by leaf, so nothing warns you later if one is missed.

- [ ] **Enable Activities** (Activities → Settings). Some accounts have to
      accept developer terms first.
- [ ] **Supported Platforms: tick iOS and Android.** Only Web (which covers the
      desktop app) is on by default. On a platform that is not ticked the
      gallery does not exist: members on phones see no leaf entry and no error.
- [ ] **Default orientation lock** (set separately for phones and tablets; it
      applies before leaf loads). leaf's screens are laid out for an upright
      phone. The day viewer has a layout for a phone held sideways as well,
      but landscape has not been tested on a device. Either leave phones
      unlocked and check the gallery sideways on your own phone, or lock phones
      to **portrait** as a stopgap. A lock also stops people who keep their
      phone mounted sideways from rotating it, so treat it as temporary. Leave
      tablets unlocked.
- [ ] **URL Mappings**: add **Prefix** `/` → **Target** `leaf.example.com`
      (host only: **no** `https://`, no trailing slash). Discord serves the
      gallery through its own proxy (`<application id>.discordsays.com`) and
      this tells it where to fetch your content. The one mapping covers the
      app, the API and the media.

### Entry Point command

Enabling Activities makes Discord create an **Entry Point command** for the
app, named **Launch**. It is what puts leaf in the app launcher, and Discord
itself answers it by opening the gallery. Leave it exactly as it is:

- Don't delete it, and don't switch it to an app-handled command through the
  API. leaf has no code that answers it.
- When someone opens the gallery this way in a text channel, Discord posts a
  message in that channel with a **Join** button, visible to everyone there.
  That is Discord's behaviour for this command, not something leaf controls.
  Whether leaf's own ways in (`/gallery` and the **Open gallery** buttons)
  also post one has not been checked on a live client.
- Discord refuses any update of an application's global command list that
  leaves this command out. leaf therefore sends it back, with the name and
  settings it already has, each time it registers its own commands
  ([01 § Command registration](01-install.md#command-registration-and-dev_guild_id)).

## 6. What members need

leaf can't grant these; they are your server's role and channel permissions.

| To do this | A member needs |
| --- | --- |
| See `/search` and the other slash commands, and **Apps → Archive to Series** on a message | **Use Application Commands** in that channel |
| Open the gallery | **Use Activities** in the channel they open it from. Probably **Send Messages** there too: other developers report that launching an Activity fails in channels where the member can't post, such as read-only announcement channels. Discord's documentation doesn't say either way. |

The bot itself needs **View Channels**, **Send Messages**, **Add Reactions**
and **Read Message History** in every series channel, and the first two in the
log channel. The install link from § 4 grants them server-wide; channel
overrides can still take them away. `/setup` warns when one is missing
([04 § First: `/setup`](04-usage.md#first-setup)).

## 7. Invite the bot

Open the install link from § 4, pick your server and authorize. You can do this
before or after [first-run setup](01-install.md#first-run-setup); the success
page of that setup offers an invite link with the same scopes and permissions.
leaf's commands are registered globally, so they are there in any server the
bot joins, with no restart.

The bot shows as offline until first-run setup is done. When it comes online,
run `/setup` in the server ([04](04-usage.md#first-setup)).

leaf greets a server the first time it sees it, with one message asking an
admin to run `/setup`: at once when it is added while leaf is running, or the
next time leaf connects when it was added while leaf was offline (the usual
case on a first install, where the bot is invited before setup is finished).
The greeting goes to the server's system channel, or else to the top-most text
channel leaf can post in. When leaf is added back, while it is running, to a
server that was already set up, the message says instead that leaf is back and
the settings are as they were.

## 8. Enable Developer Mode (to copy IDs)

You only need raw IDs for the migration in
[05-migration.md](05-migration.md) (a **server ID** and a **user ID**), and
for `DEV_GUILD_ID` when developing
([06](06-local-dev.md#running-against-real-discord-locally)).
Enable Developer Mode (desktop: **User Settings → Advanced → Developer Mode**;
mobile: **User Settings → Appearance** or **Advanced**), then right-click or
long-press a server, channel or user → **Copy ID**.

## The three hosts must match

This trips everyone up. These three must name the **same origin**:

1. **Public URL** in the setup form (e.g. `https://leaf.example.com`).
2. The **OAuth2 redirects** (`https://leaf.example.com` and
   `https://leaf.example.com/admin/callback`).
3. The **URL mapping target** (`leaf.example.com`).

If they disagree, the gallery's sign-in or the admin sign-in fails. Set up your
hostname in [03-cloudflare.md](03-cloudflare.md), then use that exact host
everywhere. After first-run setup, the success page lists the exact redirect
and mapping values for the Public URL you entered, each with a Copy button.

→ Next: **[03-cloudflare.md](03-cloudflare.md)** for the hostname and R2 media
storage.
