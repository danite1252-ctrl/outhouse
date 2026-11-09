# Outhouse

A small, self-hosted Discord-style chat app for our WoW group and family, written in Rust.

> Chat out the back.

## What works right now (v0.5)
- **Accounts:** sign up, sign in, sign out (Argon2 password hashing, 30-day login tokens)
- **Outhouses (servers):** create, invite, join, leave, delete (owner)
- **Invite links:** **Invite people** gives a link (`https://<your server>/invite/<code>`) with **Copy link**, lasting 30 minutes to 7 days or forever, for 1–100 people or unlimited; the link opens a page showing the outhouse, who invited you and who's online (plus a preview card in Discord, Messenger and WhatsApp). Signed in, **Join** puts you straight in; signed out, **Accept invite** signs you up or in and then joins. On Windows and Android, **Open in the Outhouse app** opens it in the app (Android opens invite links in the app automatically). Outhouse settings → **Invites** lists and revokes them; the join box takes links too
- **Channels:** text channels with unread markers; owner/managers can create and delete them
- **Live chat:** messages, edits, deletes, replies, reactions, typing indicator, @mentions highlighted
- **Uploads:** images, video, audio and files (up to 10 × 25 MB) via the + button, drag and drop, or paste
- **Link previews:** title, description and image for links (fetched safely by the server)
- **GIF picker** (needs a free Giphy API key in `.env`; results are cached for 10 minutes so the free key goes a long way)
- **Pinned messages:** pin up to 50 messages per channel (needs Manage Messages) from a message's hover buttons or right-click menu; **📌 Pins** in the channel header lists them
- **Search:** the search box in the channel header searches every channel you can read in the outhouse, with `from:@name`, `in:#channel` and `has:image|video|file|link` filters; click a result to jump to it
- **Direct messages:** the Home button (top of the left rail) holds your DMs; click someone's name and pick **Message**, or press **+** to start a group DM with up to 10 people (anyone you share an outhouse with). Groups can be renamed, people added, and you can leave. Unread DMs show a red count on the Home button
- **Threads:** start a thread from any message (🧵 hover button) or from **🧵 Threads** in the channel header; replies open in a side panel and the message shows "N replies". Threads with new replies get a dot, and @mentions in them (you, your roles, @everyone/@here) a red count, in the thread list, on the "N replies" link and the 🧵 Threads button; mentions add up into the channel, the outhouse icon and the tab title and send push notifications. Plain replies don't make the channel unread. Opening a thread marks it read on all your devices
- **Polls:** the 📊 composer button or `/poll` asks a question with 2–10 options, one or many picks, and an optional end time; votes update live and "See votes" shows who picked what
- **Custom emoji:** Outhouse settings → Emoji (needs Manage Outhouse) uploads PNG/GIF/WebP up to 256 KB; use them as `:name:` in messages (and DMs) and as reactions, from the ⭐ Outhouse tab in the emoji picker
- **Scroll-back:** older messages load as you scroll up in channels, DMs and threads
- **Unread and mentions:** unread channels light up, and red counts show mentions of you, your roles, @everyone and @here (and replies to you) on channels, outhouses and the browser tab
- **Roles and permissions:** Discord-style roles with colours, 28 permissions, "show separately" in the member list, and per-channel allow/deny overrides for roles or members (e.g. private or read-only channels)
- **Moderation:** kick, ban (with reasons, unban list), time out (5 min to 1 week)
- **Voice channels** (via LiveKit): join/leave, who's-in-voice list with speaking rings, mute, deafen, push-to-talk or voice activity, mic/speaker picker, noise suppression; right-click (or long-press) someone in voice for their volume (0–200%) and **Mute for me** (both only on your device)
- **Voice moderation:** with Mute / Deafen / Move Members, right-click someone in voice to **server mute** or **server deafen** them (enforced by LiveKit, shown with a red icon, and it sticks when they rejoin until a moderator undoes it), **move** them to another voice channel, or **disconnect** them
- **Screen sharing:** the 🖥 button in the voice panel shares a screen or window (optionally with its sound) in browsers and the Windows app; everyone in the channel sees it above the chat (click to enlarge, double-click for full screen) and a red **LIVE** badge in the voice list. Needs the Share screen permission (on for everyone by default). Not on phones (they can watch)
- **Soundboard:** each outhouse has its own clips (MP3/OGG/WAV up to 10 seconds and 1 MB, with a name and emoji); everyone in the voice channel hears them, with their own soundboard volume and mute; ☆ marks your favourites, shown at the top; a short cooldown stops spam
- **Entrance sounds:** User settings → Voice & audio: upload a sound (up to 5 seconds and 200 KB) that plays for everyone in a voice channel when you join it; anyone can switch other people's entrance sounds off or turn them down
- **News feeds:** follow any RSS/Atom feed in a channel (presets for Wowhead news), with keyword filters; checked every 10 minutes
- **Webhooks:** Discord-compatible, so any app with a "Discord webhook" option can post into Outhouse (rich cards with colours, fields, thumbnails)
- **Presence:** online / gone home in the member list, live
- **Right-click menus** on messages, channels, outhouses and members, plus hover buttons on messages
- Shortcuts: Enter sends, Shift+Enter new line, ↑ edits your last message, Esc cancels, Shift-click 🗑 deletes without asking
- **Raid planner:** the 📅 button plans a raid (time, length, tank/healer/DPS slots); people sign up from the card with class and spec, as late, tentative or "can't make it"; organisers can bench or remove people and copy the roster; everyone signed up gets an @mention 15 minutes before
  - **Repeating raids:** weekly on chosen days or every 2–4 weeks, ending after N raids, on a date, or never. The server makes each raid four weeks ahead (each with its own sign-ups). Edit or cancel "this raid" or "this and future raids". "Edit this and future raids" can also change the days, how often it repeats, the end, the start time and length from that raid on: a preview shows the next four dates and warns how many raids (and sign-ups) would go; raids still on a matching day keep their sign-ups, the rest are cancelled and the people signed up get a DM from the Outhouse Helper. Earlier raids, their attendance and cards stay as they were
  - **When sign-ups go up:** "Post sign-ups" picks when the raid's card is posted in chat: right away, 1–14 days before, or on a set weekday and time before the raid (repeating raids default to 9am the day after the last raid day, e.g. Monday 9am for a Sunday raid). Until then the raid shows in the calendar as "Sign-ups open …" and only its leaders can add people. Worked out in the planner's time zone, so daylight saving doesn't shift it
  - **Channel and game:** the plan dialog asks which text channel the sign-ups go in (handy from the calendar) and, optionally, which game it's for (character picks are filtered to that game)
  - **Old sign-up cards tidy themselves away:** "Remove the chat card after the raid" in Plan/Edit raid: keep it, 3 hours after it ends, or (repeating raids, the default) when the next raid's sign-ups go up (never before 3 hours after it ends if those went up early). Only the chat message goes: the raid stays in the calendar with its sign-ups, roster, invite list and attendance. Pinned cards are kept; a thread started on a card stays in the channel's thread list. Outhouse settings → Raids sets what new raids start with
  - **Raid managers and sign-up channel:** Outhouse settings → Raids (Manage Outhouse) picks roles whose members run every raid (move people between roster and bench, add and remove sign-ups, take attendance, edit or cancel raids, export invite lists) without Manage Events, and the channel "Plan a raid" starts on (you can still pick another per raid). Changes are audit-logged
  - **Characters on sign-ups:** pick one of your characters when signing up and the roster shows its name ("First Last" for WoW: Forever) with your outhouse name alongside; the name is copied onto the sign-up, so renaming or removing the character later doesn't break old rosters. Raid managers can add someone (picking one of their characters) or change someone's character or role from the card
  - **Invite list:** "📨 Invite list" on a raid card (raid managers) copies the roster for in-game invites: one name per line (Method Raid Tools, VortexInviter, RInvite…), comma-separated, MRT raid groups (groups of five, tanks → healers → DPS) or `/invite` macros split under 255 characters; with or without the bench and tentative/late people, and a First Last / First-Last / First only switch for WoW: Forever names. People without a character are flagged
  - **Calendar:** month view from the raid list (an agenda list on phones), in your own time zone; click a day to see its raids, sign up, or plan one
  - **Sign-up deadline:** optionally lock sign-ups 1 hour to 2 days before the start; after that only the raid leader (or anyone with Manage Events) can change the roster
  - **Bench:** when a role is full, new sign-ups go on the bench in sign-up order; when a spot opens the first one moves up automatically and gets a live notice plus a DM from the Outhouse Helper. Leaders can move people between roster and bench by clicking their name (people benched by hand aren't moved up automatically)
  - **Attendance:** once a raid starts its leader marks present / late / absent / benched (with "mark all signed-up as present"); attendance % and recent raids show on profile cards, and the raid list has a sortable attendance table for the last 5–50 raids. Present, late and benched count as turning up
  - **Composition:** every raid card has a fold-out **🧩 Composition**: tanks / healers / DPS against the slots (short roles in red), the class mix in class colours, armour types, and a checklist of raid buffs, boss debuffs, utility and dispels worked out from the classes and specs signed up (Fortitude, Mark of the Wild, Arcane Intellect, Blessings, Shaman totems, Trueshot Aura, Battle Shout, Warlock curses and stones, Innervate and battle res, Shadow Weaving, Moonkin Aura, Leader of the Pack, Faerie Fire, Sunder Armor, Improved Scorch / Winter's Chill, who can remove curses, poison, disease and magic; TBC raids add Bloodlust/Heroism, Misery, Wrath of Air and more). ✓ / ✗ with who brings it on hover (tap on phones). The roster counts by default; "Count tentative & bench" adds the rest. Raid leaders get a one-line "Need: 1 healer · No Mage (Arcane Intellect) · No decurse" at the top of the card. Factions aren't assumed (WoW: Forever mixes them). The buff list is one table in `crates/common/src/raidplan.rs`
  - **Soft reserves:** Plan/Edit raid picks the raid's **Loot list** (Molten Core, Onyxia, BWL, ZG, AQ20, AQ40, Naxxramas, the TBC raids, or "Other…" for a new WoW: Forever raid) and **Soft reserves: off / 1 / 2 per person**, locking when sign-ups close (or at the start), when the raid starts, or only by hand. People signed up pick from the list (a few famous items per raid to start, plus whatever leaders add) or any item by Wowhead link, ID or name, with quality colours and Wowhead tooltips, and see "2 SR" when someone else wants it too. Raid leaders get the full table by item or by player, lock/unlock, remove reserves, add items to a raid's list for the whole outhouse, and export a CSV or "Item — Player, Player" text
  - **Loot log:** from a raid's card (💰 Loot won, once it starts) raid leaders log who won what (with "reserved" marked and a note); the outhouse's **📜 Loot** log (raid list) filters by person, raid and text, and profile cards show someone's last few items. Leaders' loot changes are audit-logged
- **WoW tools:**
  - **Group finder:** the 🔎 composer button or `/lfg` posts a "looking for group" card for a Classic dungeon or raid (levels and group size fill in) or anything else, with tank / healer / DPS spots, an optional start time and a note. People join as a role with their character from the card; it shows Full when every spot's taken, the leader can remove people or close it, and the listing ends on its own (2 hours by default)
  - **Item cards:** `/item Thunderfury` (or an item ID) posts the item with its Wowhead tooltip and quality colour; Wowhead links in chat get the same card. If Wowhead can't be reached you get a search link instead
  - **Link cards** for Wowhead, Warcraft Logs and Raider.IO links, even when the site can't be fetched
  - **Characters:** add your characters in Settings → Profile & picture, per game: **WoW: Forever** (no realms: first and last name, ruleset Normal/PvP/Roleplay/Roleplay PvP, level 60, plus the Skyborne race as Alliance or Horde; race/class combinations aren't enforced since Forever changed them), **TBC** (name and realm, level 70, Jewelcrafting, Blood Elves and Draenei) or **Classic Era** (name and realm, level 60), plus class, race, level, main/alt, professions and armory link. `/char` finds Forever characters by first, last or full name (`Name-Realm` for TBC/Era). They show on your profile card, fill in your class when you sign up for a raid or group, and `/char Name` shows a character card. Optional Battle.net keys add WoW: Forever import, **Look up on the Armory** (class, race, level, ruleset, guild, item level, portrait), Refresh (and automatic refreshes for upcoming raids), "View on Armory" links and portraits on cards and rosters (see `deploy/README.md`)
- **Profiles:** profile picture, banner, "about me", accent colour and ten built-in backgrounds; click anyone's name or picture for their card
- **Bots and slash commands:** Server settings → Bots makes a bot with a token; bots can use the API and offer commands like `/roll` (example in `web/bots/`)
- **Bot buttons and menus:** bots can put Discord-style components under their messages (up to 5 rows of buttons, or a select menu), answer clicks by changing the message, replying, or replying so only the clicker sees it ("Only you can see this · Dismiss"). Buttons spin until the bot answers and say "This interaction failed" after 3 seconds. See `web/bots/README.md`
- **Automod (per channel, opt-in):** Edit channel → Automod: blocked words (whole words, `*` wildcards, leetspeak, plus an outhouse-wide list from Outhouse settings → Automod) that either block the message (the sender keeps their text) or hide it behind a "Filtered" notice; spam limits (messages per N seconds, the same message 3 times a minute, most mentions / links / attachments) with an optional timeout after repeat offences; exempt roles (and moderators); alerts to a mod-log channel; blocks go in the audit log with a short snippet
- **Quiet threads tidy themselves away:** threads archive after 1 hour, 24 hours, 3 days (default) or a week without replies (picked when you start one; channel default under Edit channel → Overview). Archived threads sit under "Archived threads" in the list, show a banner with Unarchive, and come back when someone replies. Moderators can lock a thread so only they can bring it back
- **Desktop app (Windows):** push to talk that works in game, notifications, tray (`desktop/`)
- **Phones:** the site works on phones (slide-out channel and member lists, hold-to-talk button) and installs as an app from Chrome; an experimental Android app builds in CI
- **Backups:** nightly database + uploads backup on the server, with a one-command restore (`deploy/backup.sh`, `deploy/restore.sh`), optionally copied to a Windows share and/or an rclone remote
- **Push notifications:** Settings → Notifications turns on Web Push per device (phones via Chrome's "Install app", desktops, iPhone from the Home Screen) for DMs, @mentions, replies, raid reminders and bench promotions while you're away; each outhouse can notify about all messages, only mentions or nothing, and channels and DMs can be muted (right-click, or ⋯ in a group DM) for 15 minutes, 1, 8 or 24 hours, or until you turn it back on. Muted DMs are greyed out and don't count towards the Home badge or tab title; @mentions in a muted group DM still show a count but never notify
- **Audit log:** Outhouse settings → Audit log shows who changed what (roles, channels, permissions, kicks, bans, timeouts, deleted messages, pins, emoji, sounds, voice moderation, bots, webhooks, cancelled raids, raid settings, badges), filterable by action and person, kept 90 days. Needs View audit log or Manage Outhouse
- **Badges:** automatic (Outhouse owner, Admin, Raid leader, Perfect attendance, Founder, Early member, Bot) plus custom badges admins make and hand out (Outhouse settings → Badges); shown on profile cards and next to names
- **Animated profile backgrounds:** Ember drift, Starfield and Aurora (still for people who prefer less motion)
- **Server status and uptime alerts:** Outhouse settings → Server status (version, uptime, database size, last backup); `deploy/healthcheck.sh` sends ntfy / webhook alerts when the server, voice or backups go wrong (see `deploy/README.md`)

## Run it on Windows

### 1. Install the tools (once)
1. **Rust**: download and run `rustup-init.exe` from https://rustup.rs (accept the defaults; it may ask you to install the Visual Studio C++ Build Tools, say yes).
2. **Docker Desktop**: https://www.docker.com/products/docker-desktop/ (used to run Postgres).
3. **Git** (optional but recommended): https://git-scm.com/download/win

### 2. Start the database and voice server
Open PowerShell in this folder:
```powershell
copy .env.example .env
docker compose up -d db livekit
```
Already have a `.env` from an older version? Add the `LIVEKIT_...` lines from `.env.example` to it.

### 3. Run the server
```powershell
.\start.ps1
```
This starts Postgres and the voice server (advertising your PC's home-network IP so other devices can join voice), then runs Outhouse. Or do it by hand: `docker compose up -d db livekit` then `cargo run -p outhouse-server`.

If PowerShell says scripts are disabled, run this once: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`
The first build downloads crates and takes a few minutes. When you see
`Outhouse is open for business on http://0.0.0.0:8080`, open **http://localhost:8080**.

### 4. (Optional) Turn on GIFs
Create a free API key at https://developers.giphy.com/dashboard/ and put it in `.env`:
```
GIPHY_API_KEY=your-key-here
```
Restart the server.

### 5. Try it
1. Create an account, then press **+** to build an outhouse.
2. Open a second browser (or a private window), create another account.
3. In the first window press the invite icon next to the server name and **Copy link**.
4. Open the link in the second window (or press the join icon under **+** and paste it).
5. Chat between the two windows.

To let family on your home network try it, they can open `http://<your-PC's-IP>:8080`.

### Handy commands
```powershell
cargo test --all          # run the tests
cargo fmt --all           # format the code
cargo clippy --all-targets
docker compose down       # stop Postgres (data is kept)
docker compose down -v    # stop Postgres and DELETE all data
```

## Project layout
```
crates/
  common/     shared types: models, gateway events, snowflake IDs
  server/     axum server: REST API, WebSocket gateway, Postgres
    migrations/   SQL schema (applied automatically at start-up)
web/          the web client (HTML/CSS/JS, served by the server)
docker-compose.yml, Dockerfile
ci/github-ci.yml   GitHub Actions workflow: move it to .github/workflows/ci.yml when you push to GitHub
```

## API overview
All endpoints except register/login need `Authorization: Bearer <token>`.

| Method | Path | What it does |
|---|---|---|
| POST | `/api/auth/register` | `{username, password, display_name?}` → `{token, user}` |
| POST | `/api/auth/login` | `{username, password}` → `{token, user}` |
| POST | `/api/auth/logout` | Ends the current session |
| GET | `/api/users/@me` | The signed-in user |
| GET/POST | `/api/guilds` | List your outhouses / create one `{name}` |
| GET/POST | `/api/guilds/{id}/channels` | List / create `{name, kind?}` (owner only for now) |
| GET | `/api/guilds/{id}/members` | Member list |
| POST | `/api/guilds/{id}/invites` | Create an invite code |
| POST | `/api/invites/{code}` | Join with a code |
| GET/POST | `/api/channels/{id}/messages` | History (`?before=&around=&thread=&limit=`) / send `{content, reply_to_id?, thread_id?}` |
| GET/POST | `/api/dms` | Your DMs / start one `{user_ids, name?}` (one person = one-to-one, more = group) |
| PATCH | `/api/dms/{id}` | Rename a group DM `{name}`; `POST /api/dms/{id}/recipients {user_id}` adds someone, `DELETE /api/dms/{id}/recipients/@me` leaves |
| GET/POST | `/api/channels/{id}/threads` | Threads in a channel / start one `{name, message_id?}`; `GET/PATCH/DELETE /api/threads/{id}` |
| POST | `/api/channels/{id}/polls` | `{question, options, multi?, duration_minutes?}`; vote with `PUT …/polls/{message}/votes {options}`, end with `POST …/polls/{message}/close` |
| GET/POST | `/api/guilds/{id}/emojis` | Custom emoji (multipart `name` + `file`); `PATCH/DELETE /api/emojis/{id}`; pictures at `/emojis/{id}` |
| PATCH/DELETE | `/api/guilds/{id}/voice/{user}` | Voice moderation: `{mute?, deafen?, channel_id?}` server-mutes/deafens or moves someone; DELETE disconnects them |
| GET/POST/DELETE | `/api/users/@me/entrance-sound` | Your entrance sound (multipart `file`) |
| GET | `/api/guilds/{id}/audit-log` | Audit log (`?action=&user=&before=&limit=`); needs View audit log or Manage Outhouse |
| GET | `/api/guilds/{id}/server-status` | Version, uptime, DB size, last backup (Manage Outhouse) |
| GET/POST | `/api/guilds/{id}/badges` | Badges and who has them / make one `{name, emoji, color?, description?}`; `PATCH/DELETE /api/badges/{id}`, `PUT/DELETE /api/badges/{id}/members/{user}` |
| GET | `/api/events/{id}/composition` | Roles, classes, armour, buffs and what's missing (`?extra=1` counts tentative and bench) |
| PUT/DELETE | `/api/events/{id}/reserves` | Soft-reserve `{item_id}` or `{query}` (link/ID/name); `DELETE …/reserves/{item}` (`?user_id=` for raid leaders); `PUT …/reserves/lock {locked}`; `GET …/reserves/export` |
| GET/POST | `/api/events/{id}/loot` | Loot won in a raid / log `{user_id, item_id or query, note?}`; `DELETE /api/loot/{id}`; `GET /api/guilds/{id}/loot?user=&q=&instance=&before=` |
| GET | `/api/events/{id}/loot-table` | The raid's loot list; `POST …/loot-items {query}` and `DELETE …/loot-items/{item}` (raid leaders); `GET /api/guilds/{id}/loot-instances`; `GET /api/wow/items?q=` and `/api/wow/items/{id}` |
| GET | `/api/users/@me/notifications` | VAPID key, levels, muted channels, push devices; `PUT …/notifications/guilds/{id} {level}` (0 = DMs), `PUT …/notifications/channels/{id} {muted}` |
| POST/DELETE | `/api/push/subscriptions` | Save / remove a browser's push subscription; `POST /api/push/test` sends a test |
| GET | `/api/health` | `{status, db, version, uptime_seconds}` (503 if the database is down) |
| PUT/DELETE | `/api/sounds/{id}/favourite` | Add/remove a soundboard favourite |
| … | | Roles, members, bans, channel permissions, uploads, reactions: see `crates/server/src/api/mod.rs` |
| GET | `/api/gateway` | WebSocket: send `{"op":"IDENTIFY","d":{"token":"…"}}`, then `{"op":"HEARTBEAT"}` every 30 s |

IDs are 64-bit snowflakes, sent as strings in JSON.

## Desktop app (Windows)
See [desktop/README.md](desktop/README.md): push to talk that works in game, notifications and a tray icon.

## Hosting it for real
See [deploy/README.md](deploy/README.md): one command sends Outhouse to an Ubuntu server and runs it at your domain with HTTPS, voice and everything.

## Testing on other devices at home (HTTPS)
`start.ps1` also runs Caddy, an HTTPS front door, because browsers only allow the microphone on HTTPS (or localhost).
1. Open these ports once (PowerShell **as Administrator**). `-Profile Any` matters: Windows often marks home Wi-Fi as "Public", and Private-only rules then do nothing.
   ```powershell
   New-NetFirewallRule -DisplayName "Outhouse voice UDP" -Direction Inbound -Protocol UDP -LocalPort 7882 -Action Allow -Profile Any
   New-NetFirewallRule -DisplayName "Outhouse voice TCP" -Direction Inbound -Protocol TCP -LocalPort 7880,7881,8443,8080 -Action Allow -Profile Any
   ```
2. On the other device open `https://<your-PC-IP>:8443` (start.ps1 prints it) and click past the certificate warning ("Advanced" → "Proceed"). Voice and the microphone then work.
3. To skip the warning for good on a device, install Caddy's certificate on it: `docker compose cp caddy:/data/caddy/pki/authorities/local/root.crt caddy-root.crt` and open that file on the device to trust it.

This is for home testing only; real hosting gets a proper certificate for outhouse.chat.

## Voice notes
- Voice uses LiveKit in Docker by default. The `--dev` keys in `.env.example` are fine on your own PC; set real keys before hosting publicly.
- If voice is choppy, click **Voice connected** in the voice panel to see the route (UDP or TCP), ping and packet loss. TCP means UDP 7882 is blocked. Loss over UDP usually comes from Docker Desktop's network forwarding on Windows: `.\start.ps1 -NativeVoice` downloads LiveKit for Windows once (into `tools\`) and runs it directly instead.
- Browsers only allow the microphone on `https://` sites or `localhost`, so other devices at home use `https://<your-PC-IP>:8443`.
- Push to talk works while the Outhouse window is focused; the desktop app will make it work in game.

## Webhooks
Create one in Outhouse settings → Webhooks & news feeds, then POST Discord-style JSON to it:
```powershell
$body = @{
  username = "Raid Bot"
  content  = "Raid in 30!"
  embeds   = @(@{ title = "Tonight"; color = 16737077; fields = @(@{ name = "Time"; value = "8pm"; inline = $true }) })
} | ConvertTo-Json -Depth 5
Invoke-RestMethod -Method Post -ContentType "application/json" -Body $body -Uri "http://localhost:8080/api/webhooks/ID/TOKEN"
```
Add `?wait=true` to get the message back. Note: services on the internet can only reach your webhook once Outhouse is hosted somewhere public (see the infrastructure doc).

## Checks
Every push to GitHub runs `.github/workflows/ci.yml`: builds and tests the
server, runs `ci/smoke.sh` (the whole API against a real Postgres, including a
live bot and raid reminders) and `ci/ui_test.py` (clicks through the site in
headless Chrome), and builds the Windows installer and Android APK. Security
jobs run `cargo audit`, check the vendored LiveKit client against npm, and
scan a test server with OWASP ZAP (`ci/zap.sh`, false positives explained in
`ci/zap-rules.tsv`). To check the live sites, see "Security checks" in
`deploy/README.md` (`deploy/security-scan.sh`).

## Next up (see the project docs for the full plan)
1. Battle.net character linking (pick your character when signing up for raids)
2. Loot notes
3. Mobile push notifications
