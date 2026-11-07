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

A backup on the same VM won't survive the VM's disk dying, so copy the
folder somewhere else now and then, for example from Windows:
`scp -r danite@192.168.1.122:outhouse-backups C:\Backups\Outhouse`.

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
| `LINK_PREVIEWS_OFFLINE` | off | `1` = never fetch pages for link previews (WoW links still get a basic card) |

### Battle.net linking (optional)
With these set, Settings → Profile & picture shows **Link Battle.net**, which
imports the Classic characters Blizzard lists for that account (people can
still add and edit characters by hand; if Blizzard's API doesn't list WoW:
Forever characters, nothing is imported and nothing breaks). Without them the
button is hidden.

1. Sign in at https://develop.battle.net/access/clients and **Create Client**.
2. Name it "Outhouse", and add the **Redirect URL**
   `https://outhouse.chat/api/bnet/callback` (your domain). Tick nothing else.
3. Copy the client ID and secret into `~/outhouse/deploy/.env`:
   ```
   BNET_CLIENT_ID=...
   BNET_CLIENT_SECRET=...
   BNET_REGION=us        # or eu, kr, tw
   ```
4. `docker compose up -d` in `~/outhouse/deploy`.

The server sends people back to `https://<your DOMAIN>/api/bnet/callback`,
so that's the redirect URL to register. (Running it on your PC instead? Set
`PUBLIC_URL=http://localhost:8080` in `.env` and register
`http://localhost:8080/api/bnet/callback`.) Advanced: `BNET_PROFILE_NAMESPACES` (default
`profile-classic1x-{region},profile-classic-{region}`) picks which Blizzard
character lists to try. The Battle.net sign-in token is only used once and
isn't stored.

## If something's off
- **Certificate errors / site won't load from outside:** port 80 or 443 isn't
  reaching the server, or DNS isn't pointing at your IP yet. `docker compose logs caddy`.
- **Site works from outside but not at home:** your router doesn't support
  "NAT loopback" (hairpinning). Turn it on if there's a setting, or add
  `outhouse.chat` → the server's home IP in your router's local DNS.
- **Voice connects but no audio:** UDP 7882 isn't forwarded, or the server's
  firewall blocks it (`sudo ufw status`).
- **The old Windows setup:** your PC copy still works for development
  (`.\start.ps1`); it's a separate database from the server's.
