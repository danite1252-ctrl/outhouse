# Hosting Outhouse on your own Ubuntu server

Runs the whole thing on a Linux box at home, reachable as `https://outhouse.chat`
(or your domain) from anywhere, with a real certificate and no warnings.

## Before you start
- **Ubuntu 22.04 or newer** on the server, with SSH access from your PC.
- **A fixed home-network IP for the server.** In your router, add a DHCP
  reservation for it (e.g. `192.168.1.122`), so port forwarding keeps working.
- **Check you're not behind CGNAT.** Compare the "WAN IP" on your router's
  status page with https://api.ipify.org. If they differ (or the router shows
  `100.64.x.x`–`100.127.x.x`), your ISP shares one IP between customers and
  outside connections can't reach you. Most Australian NBN providers will
  switch you off CGNAT or sell a static IP if you ask.

## 1. Point your domain at home
At your domain registrar, add an **A record**: `outhouse.chat` → your public IP
(https://api.ipify.org shows it).

Home IPs can change. If your plan doesn't include a static IP, use your
registrar's dynamic DNS (or Cloudflare + a DDNS updater on the server) so the
record follows your IP.

## 2. Forward ports on your router to the server
| Port | Protocol | For |
|---|---|---|
| 80 | TCP | Certificate setup (Let's Encrypt) |
| 443 | TCP + UDP | The website |
| 7881 | TCP | Voice (fallback) |
| 7882 | UDP | Voice |

## Already have Apache (or another web server) on ports 80/443?
Your router can only send 80/443 to one machine, so let Apache handle HTTPS
and pass the Outhouse subdomain through to this server:
1. Answer **y** to "Does another web server already handle HTTPS" during setup.
   Outhouse then listens on plain HTTP port **8080** instead of 80/443.
2. Use `deploy/oh.hartspace.cc.conf` as the Apache site (set up for Apache
   on 192.168.1.32 and Outhouse on 192.168.1.122 at oh.hartspace.cc). The
   commands to enable it and get the certificate are at the top of the file.
   (`apache-outhouse.conf` is an older template; use the oh.hartspace.cc one.)
3. On the router, only forward **7881 TCP** and **7882 UDP** to the Outhouse
   server. Voice can't go through Apache. 80/443 stay with Apache.

## 3. Send it over and start it
From `C:\dev\Outhouse Discord` on your PC:
```powershell
.\deploy\push.ps1 -Server yourname@192.168.1.122
```
It packs the code, uploads it, and runs `deploy/setup.sh` on the server. The
first time, it asks for your domain and an email address, installs Docker if
needed, makes strong passwords and voice keys (kept in `~/outhouse/deploy/.env`
on the server), opens the firewall and builds everything. The first build takes
a few minutes. Then open `https://outhouse.chat`.

**To update later:** run the same command. Your data, passwords and keys stay.

## Handy commands (on the server, in `~/outhouse/deploy`)
```bash
docker compose ps                 # what's running
docker compose logs -f server     # Outhouse's log
docker compose logs -f caddy      # certificate / HTTPS problems
docker compose restart            # restart everything
```
## Backups
Setup schedules a backup every night at 3:30 am into `~/outhouse-backups`
(the newest 14 are kept). Each one holds the database, every uploaded file
and the `.env` settings.

```bash
cd ~/outhouse/deploy
./backup.sh                                              # back up right now
./restore.sh ~/outhouse-backups/outhouse-2026-11-08_0330.tgz   # put one back (replaces everything)
```

A backup on the same VM won't survive the VM's disk dying, so let
`backup.sh` copy each one somewhere else (below), or copy the folder by hand
now and then, for example from Windows:
`scp -r danite@192.168.1.122:outhouse-backups C:\Backups\Outhouse`.

### Off-site copies (a Windows share and/or rclone)
`backup.sh` can copy each night's backup somewhere else and keep the newest
14 there too. `setup.sh` asks about it once (run `./setup.sh --offsite` to
change your answers later):

- **A shared folder on your Windows PC.** In Windows, make a folder (e.g.
  `C:\Backups\Outhouse`), right-click it → Properties → Sharing → Share…,
  and give your Windows user access. Then answer the question with
  `//<PC's IP>/<share name>` (e.g. `//192.168.1.10/Outhouse`), the Windows
  user name and password. Setup installs `cifs-utils`, keeps the password in
  `/etc/outhouse-backups.cred` (only root can read it), and mounts the share
  at `/mnt/outhouse-backups` whenever it's used (an `/etc/fstab` line with
  `nofail` and automount, so a sleeping PC never stops the VM booting).
  If the PC is off at 3:30 am, that night's copy fails and you get an alert
  (see below); the backup on the VM is still made.
- **An rclone remote** (Google Drive, OneDrive, Backblaze B2, another SSH
  box…): `sudo apt install rclone`, `rclone config` to add the remote, then
  give its name and folder, e.g. `gdrive:outhouse-backups`.

The settings live in `~/outhouse/deploy/.env`:

| Setting | What it does |
|---|---|
| `OFFSITE_DIR` | A mounted folder to copy backups to (`/mnt/outhouse-backups`). It must really be mounted, so a missing share never fills the VM's own disk (`OFFSITE_REQUIRE_MOUNT=no` turns that check off for a plain folder). |
| `OFFSITE_KEEP` | How many backups to keep there (14) |
| `RCLONE_REMOTE` | An rclone remote and folder (`gdrive:outhouse-backups`) |
| `RCLONE_KEEP` | How many backups to keep there (14) |

Each run writes `deploy/status/backup.json`; Outhouse settings → **Server
status** shows the last backup and whether the off-site copy worked, and the
uptime check below alerts you if a backup or its copy fails, or if no backup
has happened for 36 hours. Check by hand any time: `./backup.sh` (it says
"Copied to …" or why it couldn't) and `ls /mnt/outhouse-backups`.

## Uptime alerts
`deploy/healthcheck.sh` checks every 5 minutes that Outhouse answers (its
`/api/health` also checks the database), that voice (LiveKit) answers, and
that last night's backup worked. When something **changes** (goes down,
comes back, a backup fails) it sends one alert, so you're not pinged every
5 minutes. `setup.sh` asks for where to send them once (`./setup.sh --alerts`
to change):

- **ntfy** (easiest, free): install the ntfy app on your phone, subscribe to
  a topic with a long random name (e.g. `outhouse-3f9c1a2b`), and use
  `https://ntfy.sh/outhouse-3f9c1a2b` as `ALERT_NTFY_URL`. Anyone who knows
  the topic name can read it, so keep it random.
- **A webhook** (`ALERT_WEBHOOK_URL`): any Discord-compatible webhook, e.g. a
  Discord channel's webhook, or an Outhouse channel's webhook (Outhouse
  settings → Webhooks) if you'd like alerts in a channel (though if
  Outhouse itself is down, that one can't arrive: use ntfy as well).

The cron job is only installed when at least one of them is set. Other
settings (in `.env`): `HEALTH_URL` (default `http://127.0.0.1:8080/api/health`
behind Apache, else `https://<DOMAIN>/api/health`), `LIVEKIT_HEALTH_URL`
(default `http://127.0.0.1:7880`, `off` to skip) and `BACKUP_MAX_AGE_HOURS`
(36). Run `./healthcheck.sh` to check now; it logs changes to
`~/outhouse-backups/health.log` and remembers the last state in
`deploy/status/health.state` (delete that file to get a fresh "all good"
baseline).

Outhouse settings → **Server status** (people with Manage Outhouse) shows
the version, uptime, database size and the last backup.

## Push notifications (phones and desktops)
Nothing to set up: on first start the server makes its own Web Push key
(kept in the database, so it's in every backup) and Settings →
**Notifications** has a **Turn on push notifications** button. Pushes go to
a device when you're away (no Outhouse tab or app open, or it's been hidden
or idle for a while): DMs, @mentions and replies, raid reminders and bench
promotions, following each person's per-outhouse level (all / mentions /
nothing) and muted channels.

- **Android phones:** open the site in Chrome, ⋮ → **Install app**, open it,
  sign in, Settings → Notifications → Turn on. (The Android APK can't get
  push while it's closed: Android's web view has no Web Push. It still shows
  notifications while it's open.)
- **iPhone/iPad:** Share → **Add to Home Screen**, open it from there, then
  turn push on (iOS 16.4+).
- **Windows:** Chrome or Edge can do it too; the Windows app shows its own
  notifications while it's running.

Optional: `VAPID_SUBJECT=mailto:you@example.com` in `.env` (push services
use it to contact the server's owner; it defaults to the site's address).
The server only sends pushes to the big push services (Google, Mozilla,
Microsoft, Apple); `PUSH_ALLOWED_HOSTS` adds others.

## Invite links
**Invite people** (the person-plus icon next to the outhouse name, or the
outhouse menu) gives a link like `https://oh.hartspace.cc/invite/AbC12345`
with a **Copy link** button, a choice of how long it lasts (30 minutes to 7
days, or never) and how many people can use it. Outhouse settings → **Invites**
(Manage Outhouse) lists every working invite with who made it and how often it's
been used, and revokes them.

The link opens a page on your server (with a preview card when pasted into
Discord, Messenger or WhatsApp). From there people sign up or sign in and land
straight in the outhouse, or open it in the apps:
- **Android:** the app claims `https://<your domain>/invite/…` links (Android
  App Links), so tapping one opens the app if it's installed and the web page
  if not. Android checks `https://<your domain>/.well-known/assetlinks.json`,
  which Outhouse serves itself with the app's package name and the release
  signing certificate's SHA-256 (the public one CI prints as "Signing
  certificate SHA-256"; it's in `crates/server/src/state.rs`). If you ever sign
  the APK with a different key, set `ANDROID_CERT_SHA256=AB:CD:…` (comma-separate
  several) in `.env`; `ANDROID_CERT_SHA256=off` switches App Links off. The
  app's link host comes from `OUTHOUSE_SERVER_URL` in CI. Phones verify the
  link when the app is installed or updated, so after the first build with
  invite links, update the app once (or reinstall) for links to open it.
- **Windows:** the installer registers `outhouse://` links; the invite page's
  **Open in the Outhouse app** button uses `outhouse://invite/<code>`, and if
  nothing answers it suggests downloading the app.

Behind Apache, `assetlinks.json` is just proxied through like everything else
on 443 (only `/.well-known/acme-challenge/` on port 80 is answered by Apache
for certbot), so no Apache change is needed. To check from anywhere:
`curl -i https://oh.hartspace.cc/.well-known/assetlinks.json` should give
`200`, `application/json` and no redirect.

## Audit log
Outhouse settings → **Audit log** (people with View audit log or Manage
Outhouse) lists who changed what: channels and their permissions, roles,
kicks, bans, timeouts, deleted messages (by moderators), pins, emoji,
sounds, voice mutes/moves/disconnects, bots, webhooks, feeds, cancelled
raids and badges. Entries are kept for 90 days (`AUDIT_RETENTION_DAYS` in
`.env` changes that).

## WoW tools (optional settings)
The group finder (🔎 / `/lfg`), `/item`, `/char`, characters on profiles and
Wowhead / Warcraft Logs / Raider.IO link cards work out of the box. Item cards
come from Wowhead's Classic data; lookups are cached in the database for a
week, and if Wowhead can't be reached the card falls back to a Wowhead search
link. Optional settings, added to `~/outhouse/deploy/.env` (then
`docker compose up -d`):

| Setting | Default | What it does |
|---|---|---|
| `WOWHEAD_URL` | `https://www.wowhead.com` | Where item links and name searches go |
| `WOWHEAD_BRANCH` | `classic` | Wowhead's section (if Wowhead adds a WoW: Forever section, put its name here) |
| `WOWHEAD_TOOLTIP_URL` | `https://nether.wowhead.com` | Wowhead's item-details service |
| `WOWHEAD_DATA_ENV` | `4` | Wowhead's data set number (4 = Classic era) |
| `WOWHEAD_BRANCH_FOREVER` | `forever,classic` | Wowhead sections to read WoW: Forever raid loot lists from, tried in order (one Wowhead doesn't have, or refuses with 403 as it does `/forever` today, is skipped and re-checked daily). On `classic`, Season of Discovery items and bosses are left out |
| `WOWHEAD_BRANCH_TBC` | `tbc` | Wowhead's Burning Crusade section (TBC raids' loot lists and item links) |
| `WOWHEAD_DATA_ENV_TBC` | `5` | Wowhead's data set number for TBC item tooltips |
| `WOWHEAD_MIN_GAP_MS` | `1500` | Least time between two Wowhead page reads for loot lists (be polite; bosses' pages for drop chances are read ten times slower, in the background) |
| `WOWHEAD_ICON_URL` | `https://wow.zamimg.com/images/wow/icons/large` | Where item icons come from (the server keeps a copy and serves them itself) |
| `LINK_PREVIEWS_OFFLINE` | off | `1` = never fetch pages for link previews (WoW links still get a basic card) |

### Battle.net and the Armory (optional)
With Battle.net API keys set, Outhouse can:

- **Link Battle.net** (Settings → Profile & picture): imports your WoW: Forever
  characters (first and last name, class, race including Skyborne, faction,
  level, ruleset), plus any Classic Era / TBC ones. Linking again updates them
  instead of adding them twice. The sign-in token is used once and not kept.
- **Look up on the Armory** in the *Add a character* form: type the first and
  last name, pick the region, and it fills in class, race, level, ruleset,
  guild, item level and the portrait. Nobody has to link anything for this.
- **Refresh** each WoW: Forever character, and refresh characters signed up for
  raids in the next two weeks every 6 hours by itself (slowly, a few at a time).
- Show portraits, item level and guild on character cards, profile cards, raid
  rosters (hover a name) and `/char`, and use Blizzard's WoW: Forever item data
  for `/item` when Wowhead doesn't have an item.

Without the keys all of that is hidden, everything else works, and Settings →
Profile & picture says an admin can switch it on. "View on Armory" links work
either way.

**Switching it on (about 5 minutes):**

1. Go to https://develop.battle.net and sign in with your Battle.net account
   (any account works; you may need to accept the developer terms and verify
   your email).
2. Open **API Access** (https://develop.battle.net/access/clients) and click
   **Create Client**.
3. Fill in:
   - **Client name:** `Outhouse`
   - **Redirect URLs:** `https://oh.hartspace.cc/api/bnet/callback`
     (exactly that: your domain + `/api/bnet/callback`)
   - **Service URL:** `https://oh.hartspace.cc`
   - **Intended use:** something like "Guild chat site: imports our members'
     WoW characters and shows them from the Armory."
   - Leave everything else as it is, then **Save**.
4. On the client's page, copy the **Client ID**, and click to show and copy the
   **Client Secret**. (The secret is like a password: don't paste it in chat or
   anywhere public. If it ever leaks, use "Generate new secret" there.)
5. On the server, edit `~/outhouse/deploy/.env` and add:
   ```
   BNET_CLIENT_ID=paste-the-client-id
   BNET_CLIENT_SECRET=paste-the-client-secret
   BNET_REGION=us
   ```
   `BNET_REGION` is where your characters are: `us` (the Americas *and*
   Oceania, so Australian players on the Oceanic servers are `us`), `eu`, `kr`
   or `tw`. It's only the default: people can pick another region when they
   look a character up.
6. Redeploy: in `~/outhouse/deploy` run `docker compose up -d` (or use
   `push.ps1` from your PC as usual).
7. Check it: Settings → Profile & picture should show **Link Battle.net**, and
   *Add character* should show **Look up on the Armory**.
   `docker compose logs outhouse | grep -i battle.net` shows any problems
   (the secret and tokens are never written to the log).

Running it on your PC instead? Set `PUBLIC_URL=http://localhost:8080` in `.env`
and also add `http://localhost:8080/api/bnet/callback` as a redirect URL.

**Blizzard hasn't documented WoW: Forever's API yet**, so these can be changed
in `.env` if Blizzard moves things (defaults are what players found in November 2026):

| Setting | Default | What it does |
|---|---|---|
| `BNET_PROFILE_NAMESPACES` | `profile-forever-{region},profile-classic1x-{region},profile-classic-{region}` | Character lists tried by **Link Battle.net**, in order |
| `BNET_NAMESPACE_GAMES` | (worked out from the name: `forever` → WoW: Forever, `classic1x` → Classic Era, `classic` → TBC) | Which game each list's characters are from, e.g. `profile-classic-{region}=tbc` |
| `BNET_FOREVER_PROFILE_NAMESPACE` | `profile-forever-{region}` | Where Armory look-ups find characters |
| `BNET_FOREVER_STATIC_NAMESPACE` | `static-forever-{region}` | WoW: Forever item data for `/item` |
| `ARMORY_URL_TEMPLATE` | `https://worldofwarcraft.blizzard.com/en-us/forever/{region}/armory/character/{slug}` | "View on Armory" link (Blizzard's Forever armory address). `{region}`, `{slug}` (`name-surname`), `{name}`, `{surname}`, `{Name}`, `{Surname}` are filled in. `off` hides it |
| `ARMORY_URL_TEMPLATE_TBC` | `https://worldofwarcraft.blizzard.com/en-us/classicann/{region}/armory/character/{realm}/{name}` | "View on Armory" for TBC Anniversary characters. `{realm}` is the realm's slug (`Pyrewood Village` → `pyrewood-village`), `{name}` the name in lower case; the region is the character's (set in its Region box, default US). `off` hides it |
| `ARMORY_URL_TEMPLATE_ERA` | `https://worldofwarcraft.blizzard.com/en-us/classic1x/{region}/armory/character/{realm}/{name}` | The same for Classic Era characters. `off` hides it |
| `ARMORY_COMMUNITY_URL_TEMPLATE` | `https://www.aotc.gg/forever/character/{region}/{slug}` | Second link (a community armory). `off` hides it |
| `ARMORY_REFRESH_HOURS` | `6` | How often characters going to upcoming raids are refreshed (`0` = only when someone presses Refresh) |

If the Armory doesn't know a character (Blizzard's EU Forever data started
late, and brand-new characters can take a while), people see a friendly note
and can still save the character by hand.

## Security checks
Outhouse sends its own security headers (a strict Content-Security-Policy,
HSTS, no framing, no sniffing), limits sign-in attempts (10 wrong passwords
for one account or 20 from one address in 15 minutes pause sign-in for that
account/address), new accounts (20 an hour per address) and invite look-ups
(120 a minute per address), and closes other open sessions when you sign
out or change your password.

**Who can sign up.** Set `REGISTRATION` in `deploy/.env`, then
`docker compose up -d`:
- `open` (the default): anyone who can reach the site.
- `invite`: only people with a working invite link (the invite page passes
  the code along when they make their account).
- `closed`: nobody new (the very first account on a new server always works).

**Check your sites yourself** (oh.hartspace.cc and hartspace.cc by default).
On the Outhouse VM, which has Docker:

    cd ~/outhouse/deploy && ./security-scan.sh
    ./security-scan.sh --quick            # skip ZAP and the full port scan
    ./security-scan.sh oh.hartspace.cc    # one site

It checks TLS (testssl.sh), headers and http→https redirects, that `.env`,
`.git`, backups, `server-status`, folder listings and `/updates/` listings
aren't public, open ports on your public address (nmap: top 1000 TCP and the
UDP ports that matter; Postgres 5432, LiveKit's API 7880 and 8080 must be
closed) and runs an OWASP ZAP baseline (passive) scan. Nothing it does is an
attack: no password guessing, no floods. The report lands in
`~/outhouse-security/<date-time>/report.md`, with every tool's raw output
next to it, and a summary is printed at the end (it exits non-zero if
anything needs fixing).

From inside the house your router "hairpins" traffic to your own public
address, so the port scan may show the inside view. For the real outside
view, run the Windows version on a laptop connected to a **phone hotspot**:

    powershell -ExecutionPolicy Bypass -File .\deploy\security-scan.ps1

(or point an online port scanner at your public IP). Expected open: 80 and
443 (Apache), 7881 TCP and 7882 UDP (voice). Anything else, especially
5432, 7880, 8080 or 22 with passwords allowed, should be closed on the
router.

**Apache** (it also serves hartspace.cc): `deploy/apache-hardening.conf` is
a server-wide snippet (ServerTokens Prod, ServerSignature Off, TraceEnable
Off, TLS 1.2+ only with modern ciphers, no folder listings, dotfiles and
backup files refused, mod_status for localhost only, HSTS and nosniff for
every site). Install steps are at the top of the file. The Outhouse site
file (`oh.hartspace.cc.conf`) also drops any `X-Forwarded-For` a visitor
sends, so the sign-in limits see the real address.

**Other things worth knowing**
- Outhouse believes `X-Forwarded-For` only from `TRUSTED_PROXIES` (default:
  this machine, home and Docker networks). If Apache isn't on a private
  address, set `TRUSTED_PROXIES=127.0.0.1/32,172.16.0.0/12,<apache ip>/32`.
- Docker's published port 8080 (the behind-Apache set-up) skips ufw, so
  anyone on your home network can reach it directly. That's fine for a home
  LAN; to allow only Apache, add a `DOCKER-USER` rule:
  `sudo iptables -I DOCKER-USER -p tcp --dport 8080 ! -s 192.168.1.32 -j DROP`
  (and save it with `iptables-persistent`).
- Backups contain `.env` (every password). They're readable only by you;
  the off-site copies are as safe as the share they're on.
- `RATE_LIMITS=off` turns the limits off (for testing only).

## If something's off
- **Certificate errors / site won't load from outside:** port 80 or 443 isn't
  reaching the server, or DNS isn't pointing at your IP yet. `docker compose logs caddy`.
- **Site works from outside but not at home:** your router doesn't support
  "NAT loopback" (hairpinning). Turn it on if there's a setting, or add
  `outhouse.chat` → the server's home IP in your router's local DNS.
- **Voice connects but no audio:** UDP 7882 isn't forwarded, or the server's
  firewall blocks it (`sudo ufw status`).
- **Server mute / disconnect only work on the next rejoin:** Outhouse can't
  reach LiveKit's API. It uses `http://host.docker.internal:7880` (set as
  `LIVEKIT_API_URL` in `docker-compose.yml`); `docker compose logs server | grep LiveKit`
  shows why. The firewall rule setup.sh adds for Caddy (172.16.0.0/12 → 7880)
  covers it.
- **The old Windows setup:** your PC copy still works for development
  (`.\start.ps1`); it's a separate database from the server's.
