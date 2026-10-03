# 04 — Using leaf in Discord

With the bot online and in your server, this is the day-to-day: setting up the
server, running a series, archiving posts, and browsing the gallery. Three
roles (**admin**, **creator**, **viewer**) overlap freely; you can be all three.

## Words leaf uses

| Word | Meaning |
| --- | --- |
| **gallery** | leaf's app inside Discord, where series are browsed and managed. Discord's own word for this kind of app is "Activity"; in Discord's menus it is listed simply as **leaf**. |
| **series** | One person's ongoing, numbered archive: "Daily Sketch", Day 1, Day 2, and so on. A server can have many. |
| **day** | One numbered entry of a series ("Day 42"). A day holds the files of one post. Day numbers count posts, not calendar dates. |
| **post** | The Discord message a day was made from. leaf never edits or deletes it. |
| **archive** | As a verb: copy a post's images and videos into leaf's storage as a day. As a noun: everything a series has stored. |
| **remove**, **delete** | The same thing: take a day out of the archive, stored files included. The post stays in the channel. |
| **series channel** | A channel the server allows posts to be archived from (chosen in `/setup`). |
| **sprout** | A new series that only its creator can see in the gallery until it has enough days. Admins see it in the admin panel and through the chat commands. |
| **revoked**, **taken down** | A series an admin has hidden and frozen. Nothing is deleted. |

## Opening the gallery

The gallery opens inside Discord, in a server channel. There are three ways in:

- **The app launcher.** On a phone, tap **+** next to the message box, then
  **Apps**, then **leaf**. On desktop, click the **Apps** button in the message
  box, then **leaf**. Opened this way in a text channel, Discord posts a
  message with a **Join** button in that channel
  ([02 § Entry Point command](02-discord.md#entry-point-command)).
- **`/gallery`**, or `/gallery series:<name>` to open one series directly.
- **An Open gallery button** on one of leaf's messages: the result of an
  archive, a `/search`, `/random`, `/status` or `/wrapped` card, the `/leaf`
  help, the `/setup` summary, the public how-to and the `/import` result. On a
  card or an archive result the button opens the gallery on that series, and on
  that day when the message is about one day. The buttons on cards, the help
  and the how-to keep working for as long as the message exists, for anyone who
  can see it (a member who isn't allowed to see the series gets the gallery
  where it would open anyway). The ones on an archive result, the `/setup`
  summary and the `/import` result work while that reply is live, 10 to 15
  minutes.

If Discord refuses to open the gallery from a command or a button, leaf answers
privately with the app-launcher path above. Nothing opens the gallery from a
DM: leaf needs a server.

The gallery shows an admin what it shows any other member: Manage Server
doesn't open someone else's private series, a series limited to a role they
lack, a sprout or a revoked series there. An admin who asks for such a series
with `/gallery series:<name>` or an Open gallery button gets a private answer
saying so, with the two places that do show it: the admin panel and `/search`.
The gallery isn't opened somewhere else instead.

Without a destination from chat, the gallery opens on the one series that uses
the channel you opened it from, otherwise on the series you looked at last, or
on the only series there is. Failing all of those it shows the list of series.

Opening leaf's address in a normal browser shows a page saying leaf is running,
with these same instructions and a link to the admin panel. The gallery itself
only runs inside Discord.

## For server admins

### First: `/setup`

Until an admin has run **`/setup`**, leaf answers its other commands (all but
`/gallery` and `/ping`) with "leaf isn't set up in this server yet" and nobody
can start a series. `/setup` needs **Manage Server**. It opens one private form
with four menus:

1. **Series channels**: where posts can be archived from, threads under them
   included. Text and announcement channels, up to 25, at least one.
2. **Log channel** (optional): one quiet line for each day that is archived,
   moved or removed. No pings. Series that members can't see are logged as "A
   private series".
3. **Creator role** (optional): pick a role and only members with it can start
   a series. Leave it empty and anyone can.
4. **Timezone**: 24 common zones, or **Other timezone** to type a city or a
   timezone name (`Lisbon`, `Asia/Kolkata`). The dates on the gallery's
   calendar and the year boundaries of `/wrapped` follow it, and so do
   reminders for a series that has no timezone of its own. New servers start on
   UTC, so pick yours.

Nothing changes until you press **Save**. The form shows warnings as you pick,
for example when leaf can't see a channel or is missing Add Reactions there.
When the log channel changes, Save posts one test line in it first; if leaf
can't post there, nothing is saved and the form says why. Left alone for 10
minutes, the form closes without saving.

`/setup` can be run again at any time. It opens filled in with the saved
values, so it is also how you change channels later.

### After Save

The form turns into a summary: the channels, who can start a series, the
timezone. Under it:

- **Open gallery**.
- **Post a how-to in #channel**: leaf posts one public message in the first
  series channel it can write in, telling members how to start a series and how
  to archive a post, with an **Open gallery** button under it. It is the one
  public explanation members get, so it is worth pressing. (Anyone can read the
  same text privately with `/leaf`.)
- **Admin panel**: a link to the web panel for this server.

The buttons work for about 10 minutes after the last press. To get them back,
run `/setup` and press **Save** again.

### Sprouts

With **Sprout stage for new series** switched on in the admin panel, a new
series starts as a 🌱 **sprout**: its creator archives as usual, but in the
gallery only they can see it until it has the set number of days (3 unless you
change it). Then it is published under its own privacy setting. Admins see
sprouts in the admin panel and through the chat commands. It is off by
default. Nothing removes a sprout that never gets there; it stays hidden.
Switching the stage off, or lowering the number of days, publishes the sprouts
that qualify, and the panel says how many.

### Revoking a series

Admins don't approve series, they can take them down afterwards. In the admin
panel, **Revoke** hides a series from the gallery and stops new archives,
renumbering and deletes by its creator. Its days and files are kept, and
admins still reach them through the chat commands. **Restore** brings it back.
Revoking, restoring and publishing each write a line in the log channel.

### The admin web panel

Browse to **`https://leaf.example.com/admin`** and **Sign in with Discord**. You
need **Manage Server** in a server leaf is in. A server where `/setup` hasn't
been run yet is listed too, with a note saying so.
`https://leaf.example.com/admin?guild=<server id>` opens one server directly,
and the `/setup` form links there. Servers are shown with their name and icon.

If the sign-in doesn't finish (you pressed Cancel, it took more than 10
minutes, Discord rejected it or couldn't be reached, or you manage no server
leaf is in), you come back to the panel's sign-in card, which says which of
those it was.

**Settings** (nothing is saved until **Save changes**):

| Field | What it does |
| --- | --- |
| Timezone | Same setting as in `/setup`, picked from a list. |
| Creator role, Log channel | Same settings as in `/setup`, picked from the server's roles and its text and announcement channels. The role list leaves out `@everyone` and roles that belong to a bot, which no member can hold. If leaf can't get those lists from Discord, each field becomes a box for a role ID or channel ID (Developer Mode → Copy ID) with a link to load the list again. |
| Series per member | How many series one member may own (1 or more; 3 by default). |
| Minimum Discord account age, in days | 0 for no minimum. |
| Minimum time in this server, in days | 0 for no minimum. |
| Sprout stage for new series | See [Sprouts](#sprouts). |
| Days before a sprout is published | 1 or more. |

A value leaf won't take (an unknown timezone, a role or channel that isn't in
this server, a number out of range) is refused with the reason under its field,
and nothing is saved. A save writes only the fields you changed, so it doesn't
undo a `/setup` that someone saved in chat while the panel was open.

**Series**, one row each, with the creator's name and the series' state (a
sprout shows how many days it has so far): change who can see it (the row asks
you to **Save** the change), **Revoke** (asks first), **Restore**, and
**Publish now** for a sprout.

The panel can't change series channels (that is `/setup`), can't start or edit
a series (creators do that in the gallery), and can't change leaf's credentials
([01 § `--reconfigure`](01-install.md#changing-credentials-later---reconfigure)).
A sign-in lasts an hour.

### `/export` and `/import`

Both need **Manage Server**; creators don't see them.

- **`/export [series]`** sends you a JSON file named
  `leaf-export-<series>-<date>.json`. It is an **index, not a backup**: each
  day's number, date and the post it came from. No images, videos or captions.
  The real backup is leaf's data folder plus the storage bucket
  ([01 § Data, backups, and updates](01-install.md#data-backups-and-updates)).
- **`/import file:<json> [series]`** reads such a file (or a walpurgisbot-v2
  export) into an **existing** series. It shows what the file holds and asks
  before writing. Imported days have a number, a date and a link to the post,
  and **no media**. **Undo import** is offered for 10 minutes. A file over
  24 MB, or one with more than 20,000 entries, is refused and nothing is
  imported.

If you want the pictures too, have whoever hosts leaf run `leaf-migrate` with
the same file ([05-migration.md](05-migration.md)), before or after the
`/import`. It fetches the files from the original posts, and it also fills in
days that `/import` created, as long as the original post still has its files.

## For creators

Creators use two places:

- **The gallery** to start a series, change its settings and set reminders.
- **Chat** to archive posts and to look up or delete days.

### Start a series

Open the gallery and choose **Start a series**. It is one short form:

- **Series name** (2 to 40 characters) and an optional **Description**.
- **Who can see it?** Everyone in the server, only members with a role you
  pick, or only you. The role list holds every role members can have, the
  booster role and subscriber roles included; `@everyone` and roles that
  belong to a bot are left out.
- Under **More options**: the **Channel** you'll post in (one of the server's
  series channels), **How often you'll post** (daily, weekdays, weekly, or
  freeform with no schedule), and the **First day number** (1 unless you are
  continuing an existing count).

The server's rules are checked before the form opens and again when you submit:
the creator role, the number of series you already own, and how old your
account and your membership are. If one of them stops you, the gallery says
which. There is no approval step.

The new series opens with the steps for archiving your first post.

### Archive a post

Archiving happens in chat, on the message itself:

1. Post your photo or video in a series channel (or a thread under one).
2. **On a phone**, press and hold the message, tap **Apps**, then **Archive to
   Series**. **On desktop**, right-click it, choose **Apps**, then **Archive to
   Series**.
3. If leaf can't tell which of your series it is for, it asks: a button per
   series (a menu when you have more than five). It doesn't ask when you have
   one series, or when exactly one of yours uses that channel.
4. A small form asks for the **day number**, already filled in: the day written
   in the post ("Day 42"), otherwise the series' next day. Correct it if needed
   and submit.
5. leaf answers privately, stores the files, and turns the answer into the
   result: "Day 42 of **Daily Sketch** archived (2 files)." It also reacts to
   the post with the series' emoji and writes a line in the log channel.

Only you can archive into your series, but the message doesn't have to be yours
(the result says whose it was). Forwarded messages work. Old messages work too:
this is also how you catch up on days you missed.

leaf stores uploaded files only: PNG, JPEG, WebP, GIF, MP4, WebM and MOV, up to
100 MB each. Link previews and GIF-picker embeds are not files. A file that is
too large or of another type is named and skipped; the rest of the post is
still archived.

The result carries buttons, which work for about 14 minutes:

| Button | What it does |
| --- | --- |
| **Change day** | Renumbers the day you just archived. |
| **Undo** | Removes the day and its stored files again, and takes leaf's reaction off the post. Not offered after a Replace. |
| **Open gallery** | Opens the gallery on the day you just archived. |

### When leaf asks instead of archiving

| What you see | What to do |
| --- | --- |
| "leaf doesn't archive from this channel." | Post in one of the series channels it lists, or ask an admin to add this one with `/setup`. |
| "Day N … is already archived from another post." | **Use Day N+1** takes the next free day, **Pick another day** reopens the form, **Replace Day N** stores this post instead and deletes the other post's files. |
| "This post is already Day N of …" | The message is archived already. **Archive as another day**, **Change day** (renumbers the entry, also days later), **Remove Day N**, or **Open gallery**. With more than one entry, only **Archive as another day** and **Open gallery** are offered. |
| A question before a day far from the expected one | A day 7 or more past the next one, or before the series' first day, is more often a typo. **Yes, Day N** goes ahead; **Change day** reopens the form. |
| "…couldn't be downloaded" or a storage problem, with **Try again** | Nothing was archived. Try again; if it keeps failing, tell an admin (the log channel gets a line when leaf's storage is the cause). |
| "leaf is still archiving this post from an earlier try" or "Another post is being archived as Day N … right now" | An earlier try, or another post aimed at the same day, is still storing its files. Wait a moment, then press **Check again** or **Try again**. For the second message, **Pick another day** is offered too. |
| "You don't have a series yet." | Start one in the gallery, then archive the post again. |
| "A server admin revoked …" | The series can't take posts until an admin restores it. |

### Milestones

Day 1, every 100th day, and each full year of a **daily** series (Day 365, 730,
…) are milestones. leaf announces one in the channel, as a reply to the post
and without pinging anyone, only when all of this is true: the series is public
and past its sprout stage, the day is the highest so far, and the post is less
than 48 hours old. Catching up on old posts stays quiet. For any other series
the milestone is a line in your private result instead.

### Manage your series

In the gallery, **Manage my series** lists the series you own. Open one, or use
the gear on a series' own page, for **Series settings**: name, description,
reaction emoji, who can see it, how often you post, the channel, the first day
number, and reminders. Only changed fields are saved. The first day number
can't be set higher than the earliest day already archived.

A series an admin revoked stays in your list, marked as such, and its settings
are read-only.

leaf has no way to delete a whole series. Delete days with `/delete`, or ask an
admin to revoke the series.

### Reminders

In Series settings, **Remind me when I'm behind** sends a nudge when the next
day isn't archived by the time you choose (8 PM in your device's timezone
unless you change it):

- You choose a **DM** or a **ping in the series channel**. The ping is a
  public message that mentions you. It names the series only when everyone in
  the server can see it; for a private, role-limited or sprout series it says
  "your series". A DM that can't be delivered is never replaced by a ping.
- One nudge per missing day, sent within 6 hours of the chosen time and
  stopping at midnight (a reminder set after 11 PM gets one hour). Then leaf
  stays quiet until you archive that day.
- Weekly series are nudged on the weekday of their last post. If that nudge
  didn't go out (leaf was offline, say), it goes out on the next day leaf can
  send it. Weekdays series aren't nudged on Saturday or Sunday. Freeform
  series have no reminders, and nothing is sent for a series before its first
  archived day.
- If a reminder can't be delivered (your DMs are closed to the server, the
  series channel is gone, or leaf isn't allowed to post there), leaf doesn't
  keep trying for that day. Series settings then shows "leaf couldn't deliver
  your last reminder" with the reason and what to change, and **Manage my
  series** marks the series "reminders can't reach you".

To stop reminders, press **Turn off reminders** on the reminder itself (only
the series' creator can), or switch them off in Series settings. The time,
timezone and DM choice are kept for when you switch them back on.

## Command reference

Every command leaf registers. `[series]` can be left out when there is only one
series it could mean; when several fit, leaf shows a menu. (`/gallery` is the
exception: without a name it opens the gallery itself, and for a name that
fits several series it lists them and asks for more of the name.) Typed names
match loosely (any case, the start of a name, part of a name). A series you
can't see is never suggested or named.

| Command | Who | What it does |
| --- | --- | --- |
| `/gallery [series]` | anyone | Opens the gallery, on the named series if you give one ([above](#opening-the-gallery)). |
| `/leaf` | anyone | A private how-to for this server: starting a series, archiving a post, the gallery, with an **Open gallery** button. Before `/setup` has been run it says so instead. |
| `/setup` | Manage Server | The server settings form ([above](#first-setup)). |
| **Archive to Series** (on a message: Apps) | creators | Archives the message as a day of your series ([above](#archive-a-post)). |
| `/search day:<number> [series]` | anyone who can see the series | One day: its picture, caption, date and a link to the post. |
| `/status [series]` | anyone who can see the series | A private summary: how many days are archived, the latest post, which day numbers are missing, and which days have no stored media. |
| `/random [series]` | anyone who can see the series | A random day, one with a picture when the series has any. |
| `/delete day:<number> [series]` | the series' creator, or an admin | Shows the day and asks before deleting it and its stored files. |
| **Remove Archive Entry** (on a message: Apps) | the series' creator, an admin, or the person who posted the message | The same delete, starting from the message. |
| `/export [series]` | Manage Server | The series' day index as a JSON file ([above](#export-and-import)). |
| `/import file:<json> [series]` | Manage Server | Imports day numbers and dates, without media ([above](#export-and-import)). |
| `/wrapped [series] [year]` | anyone who can see the series | A year in review: posts, longest run, busiest month, first and last post. |
| `/ping` | anyone | leaf's version and uptime, shown only to you. |

Notes:

- `/search`, `/random` and `/wrapped` answer in the channel for a series that
  is public and past its sprout stage. For any other series only you see the
  answer. `/status`, `/delete`, `/export`, `/import`, `/leaf` and `/ping` are
  always private.
- The cards from `/search`, `/random`, `/status` and `/wrapped` have an **Open
  gallery** button that opens the series (and the day, for a day card).
- A delete can't be undone. The post itself stays in the channel; leaf's
  reaction comes off it once no day is left from that message.
- In a series an admin revoked, only an admin can delete days.
- `/setup`, `/export` and `/import` are hidden from members without Manage
  Server.

## For viewers: the gallery

Open it as described under [Opening the gallery](#opening-the-gallery). You see
the series you're allowed to see: public ones, role-gated ones whose role you
have, and your own.

- **Series list**: every series you can view. **Start a series** is here when
  the server lets you.
- **A series' page**: its numbers (latest run, longest run, days archived,
  skipped day numbers) and a **calendar**, newest month first. Days sit on the
  calendar by the server's timezone (the one from `/setup`), and the calendar
  says which. Tools above the calendar jump to the latest day, a random day, a
  day number or a month, and reload.
- **A day**: tap one for the full image or video, the caption, the date and the
  day number. The arrows, the arrow keys and a swipe step through the files of
  a day with several, then on to the previous or next day; dots show which
  file you are on and jump between them. Pinch, double-tap, the mouse wheel or
  the zoom buttons zoom a photo. **Open original post** opens the post in its
  channel (if Discord doesn't open it, the viewer shows the link to copy), and
  **Random** picks another day.

A day whose media was never stored (an imported day, or a migrated day whose
post was gone) says "No file was saved for this day". A video that doesn't
play says "This video didn't play", with **Try again** and a pointer to
**Open original post**. A photo that doesn't load offers **Try again** too,
and a swipe still moves on from it.

→ Migrating an old archive in? See **[05-migration.md](05-migration.md)**.
