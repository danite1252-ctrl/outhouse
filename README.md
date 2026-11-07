# Outhouse

A small, self-hosted Discord-style chat app for our WoW group and family, written in Rust.

> Chat out the back.

## What works right now (v0.5)
- **Accounts:** sign up, sign in, sign out (Argon2 password hashing, 30-day login tokens)
- **Outhouses (servers):** create, invite with a 7-day code, join, leave, delete (owner)
- **Channels:** text channels with unread markers; owner/managers can create and delete them
- **Live chat:** messages, edits, deletes, replies, reactions, typing indicator, @mentions highlighted
- **Uploads:** images, video, audio and files (up to 10 × 25 MB) via the + button, drag and drop, or paste
- **Link previews:** title, description and image for links (fetched safely by the server)
- **GIF picker** (needs a free Giphy API key in `.env`; results are cached for 10 minutes so the free key goes a long way)
- **Pinned messages:** pin up to 50 messages per channel (needs Manage Messages) from a message's hover buttons or right-click menu; **📌 Pins** in the channel header lists them
- **Search:** the search box in the channel header searches every channel you can read in the outhouse, with `from:@name`, `in:#channel` and `has:image|video|file|link` filters; click a result to jump to it
- **Unread and mentions:** unread channels light up, and red counts show mentions of you, your roles, @everyone and @here (and replies to you) on channels, outhouses and the browser tab
- **Roles and permissions:** Discord-style roles with colours, 27 permissions, "show separately" in the member list, and per-channel allow/deny overrides for roles or members (e.g. private or read-only channels)
- **Moderation:** kick, ban (with reasons, unban list), time out (5 min to 1 week)
- **Voice channels** (via LiveKit): join/leave, who's-in-voice list with speaking rings, mute, deafen, push-to-talk or voice activity, mic/speaker picker, noise suppression, per-person volume (right-click someone in voice)
- **Soundboard:** each outhouse has its own clips (MP3/OGG/WAV up to 1 MB); everyone in the voice channel hears them, with their own soundboard volume and mute
- **News feeds:** follow any RSS/Atom feed in a channel (presets for Wowhead news), with keyword filters; checked every 10 minutes
- **Webhooks:** Discord-compatible, so any app with a "Discord webhook" option can post into Outhouse (rich cards with colours, fields, thumbnails)
- **Presence:** online / gone home in the member list, live
- **Right-click menus** on messages, channels, outhouses and members, plus hover buttons on messages
- Shortcuts: Enter sends, Shift+Enter new line, ↑ edits your last message, Esc cancels, Shift-click 🗑 deletes without asking
- **Raid planner:** the 📅 button plans a raid (time, length, tank/healer/DPS slots); people sign up from the card with class and spec, as late, tentative or "can't make it"; organisers can bench or remove people and copy the roster; everyone signed up gets an @mention 15 minutes before
- **Profiles:** profile picture, banner, "about me", accent colour and ten built-in backgrounds; click anyone's name or picture for their card
- **Bots and slash commands:** Server settings → Bots makes a bot with a token; bots can use the API and offer commands like `/roll` (example in `web/bots/`)
- **Desktop app (Windows):** push to talk that works in game, notifications, tray (`desktop/`)
- **Phones:** the site works on phones (slide-out channel and member lists, hold-to-talk button) and installs as an app from Chrome; an experimental Android app builds in CI
- **Backups:** nightly database + uploads backup on the server, with a one-command restore (`deploy/backup.sh`, `deploy/restore.sh`)

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
3. In the first window press the invite icon next to the server name, copy the code.
4. In the second window press the join icon (under **+**) and paste the code.
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
| GET/POST | `/api/channels/{id}/messages` | History (`?before=&limit=`) / send `{content}` |
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
headless Chrome), and builds the Windows installer and Android APK.

## Next up (see the project docs for the full plan)
1. Battle.net character linking (pick your character when signing up for raids)
2. Raid attendance history and loot notes
3. Mobile push notifications
