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
Backups: `docker compose exec db pg_dump -U outhouse outhouse > backup.sql`.
Uploaded files live in the `outhouse_uploads` Docker volume.

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
