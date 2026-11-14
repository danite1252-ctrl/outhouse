# Moving voice (LiveKit) to its own server

Outhouse's voice chat and screen sharing run on **LiveKit**. At the start, it runs next to everything else in `deploy/docker-compose.yml`. Once Outhouse gets busy, voice is the first thing that should get its own machine. It uses far more CPU and bandwidth than chat, the database or the image detector, and a busy raid night shouldn't slow down everything else.

This guide moves LiveKit to a second server. **Chat, accounts, files and the database stay where they are.** Only voice moves, so people just reconnect to voice once.

---

## When to do it

Move voice when any of these happen:

- **CPU:** the main server sits above about 60% while voice is busy. Watch `docker stats`: it's the `livekit` container.
- **Sound and video:** people report choppy voice or blocky screen shares, even though chat is fine.
- **Traffic:** you regularly have several busy voice channels at once, or big events (40-person raids plus screen shares).
- **The cloud move:** you're moving to the cloud anyway. Planning it from day one costs little: a VPS-1 for voice is about A$6/month.

Rough capacity on a single 4-core server, forwarding only (LiveKit doesn't re-encode):
- **Voice:** a few hundred people across many channels.
- **Screen shares:** each viewer of a 1080p share costs roughly 2–4 Mbps of upload from the voice server, so bandwidth usually runs out before CPU. Use a host with **unmetered traffic and at least 500 Mbps–1 Gbps**.

---

## What you need

1. **A second server:** Ubuntu 24.04 in the same city as the main one, e.g. Sydney. A VPS-1 (2 vCPU / 4 GB) is plenty to start; use VPS-2 (4 vCPU / 8 GB) for big events.
2. **A name for it:** e.g. `voice.yourdomain`, with a **DNS A record pointing at the voice server's IP**. If you use Cloudflare, set it to **grey cloud (DNS only)**. Cloudflare's proxy doesn't carry voice traffic.
3. **These ports open on the voice server,** in the provider's firewall *and* in `ufw`:

   | Port | Protocol | What it's for |
   |---|---|---|
   | 22 | TCP | SSH, for you |
   | 80, 443 | TCP | HTTPS certificate and voice signalling (`wss://`) |
   | 7881 | TCP | Voice media fallback, for networks that block UDP |
   | 7882 | UDP | Voice media, the normal path |
   | 3478 | UDP | TURN, for very restrictive networks (optional) |
   | 5349 | TCP | TURN over TLS (optional) |

4. **The same LiveKit API key and secret** the main server uses. They're in `deploy/.env` on the main server (`LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`). You'll copy them across rather than make new ones.

---

## Step 1: set up the voice server

SSH in as your user, then:

```bash
# Docker
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker $USER && newgrp docker

# Firewall
sudo ufw allow OpenSSH
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw allow 7881/tcp
sudo ufw allow 7882/udp
sudo ufw allow 3478/udp
sudo ufw allow 5349/tcp
sudo ufw --force enable

mkdir -p ~/voice && cd ~/voice
```

Create `~/voice/.env`, using the values from the main server's `deploy/.env`:

```bash
VOICE_DOMAIN=voice.yourdomain
LIVEKIT_API_KEY=<same as the main server>
LIVEKIT_API_SECRET=<same as the main server>
```

Then `chmod 600 .env`.

Create `~/voice/livekit.yaml` and swap in your key and secret:

```yaml
port: 7880
bind_addresses: ["127.0.0.1"]
rtc:
  tcp_port: 7881
  udp_port: 7882
  use_external_ip: true
turn:
  enabled: true
  domain: voice.yourdomain
  udp_port: 3478
  tls_port: 5349
  # TURN over TLS reuses the certificate Caddy (below) gets from Let's Encrypt.
  cert_file: /caddy/caddy/certificates/acme-v02.api.letsencrypt.org-directory/voice.yourdomain/voice.yourdomain.crt
  key_file: /caddy/caddy/certificates/acme-v02.api.letsencrypt.org-directory/voice.yourdomain/voice.yourdomain.key
keys:
  <LIVEKIT_API_KEY>: <LIVEKIT_API_SECRET>
logging:
  level: info
```

Then `chmod 600 livekit.yaml`.

> **TURN is optional.** If you'd rather skip it at first, delete the whole `turn:` block and don't open ports 3478 and 5349. Almost everyone connects fine without it. It only helps people on very locked-down networks (some workplaces, schools and hotel Wi-Fi).

Create `~/voice/Caddyfile`:

```
{$VOICE_DOMAIN} {
	reverse_proxy 127.0.0.1:7880
}
```

Create `~/voice/docker-compose.yml`:

```yaml
services:
  livekit:
    image: livekit/livekit-server:latest   # pin a version once it works, e.g. v1.x.y
    restart: unless-stopped
    network_mode: host
    command: --config /etc/livekit.yaml
    volumes:
      - ./livekit.yaml:/etc/livekit.yaml:ro
      # Caddy's certificates, read-only, for TURN over TLS
      - caddy_data:/caddy:ro

  caddy:
    image: caddy:2
    restart: unless-stopped
    network_mode: host
    env_file: .env
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config

volumes:
  caddy_data:
  caddy_config:
```

Start it:

```bash
docker compose up -d
docker compose logs -f   # wait for Caddy to get the certificate, then Ctrl+C
```

**If you enabled TURN:** LiveKit starts before Caddy has its certificate the very first time, so restart LiveKit once the certificate exists:

```bash
docker compose restart livekit
```

Caddy renews the certificate about every 60 days. LiveKit only reads it at start-up, so add a monthly restart with `crontab -e`:

```
0 4 1 * * cd ~/voice && docker compose restart livekit
```

Or leave TURN off.

**Check it:**
- Open `https://voice.yourdomain` in a browser. LiveKit answers with **OK**.
- For a full test, use LiveKit's connection tester at https://livekit.io/connection-test. It needs a test token; ask Claude to make one with your key.

---

## Step 2: point Outhouse at it

On the **main** server, in `deploy/.env`, add or change these:

```bash
LIVEKIT_URL=wss://voice.yourdomain
LIVEKIT_API_URL=https://voice.yourdomain
```

`LIVEKIT_API_KEY` and `LIVEKIT_API_SECRET` stay exactly as they are.

Then, in the outhouse folder on the main server:

```bash
docker compose up -d server
docker compose stop livekit
```

That's all the main server needs:
- **The browser's security rules** (CSP) are built from `LIVEKIT_URL`, so they allow the new voice address automatically.
- **Server mute, disconnect and the voice lists** use `LIVEKIT_API_URL` to talk to the voice server.
- **The desktop and Android apps** get the voice address from the server, so they don't need updating.

Anyone in voice at the moment you switch drops out and simply rejoins.

**Afterwards, on the main server:**
- Close ports 7881/tcp and 7882/udp there (`sudo ufw delete allow 7881/tcp` and `sudo ufw delete allow 7882/udp`), and remove them from any router port-forwards.
- The `/rtc` lines in the main server's Caddy and Apache configs become unused. They're harmless, and you can tidy them later.
- To make sure LiveKit doesn't come back on the main server after a `push.ps1`, ask Claude to add a `VOICE_EXTERNAL=true` switch to `setup.sh` that skips the local LiveKit.

---

## Step 3: test

1. **Two people in a voice channel:** both hear each other, mute and deafen work, and the speaking rings light up.
2. **A screen share:** another person can watch it.
3. **As a moderator:** server-mute someone, then disconnect them. Both should work, which proves `LIVEKIT_API_URL` and the keys are right.
4. **From a phone on mobile data:** this tests the internet path rather than your home network.
5. **On the voice server:** `docker stats` shows LiveKit's CPU while people talk.

**If voice won't connect:**
- **Check the ports:** 7881/tcp and 7882/udp must be open in **both** the provider's firewall and `ufw`.
- **Check DNS:** `voice.yourdomain` must be **grey cloud** if you use Cloudflare.
- **Check the keys:** the key and secret must be **identical** on both servers.
- **Read the logs:** `docker compose logs livekit` on the voice server. A "could not find external IP" error means outbound internet is blocked. "invalid token" means the keys don't match.

**To undo it:** remove the two lines from the main server's `.env`, run `docker compose up -d server livekit`, and re-open 7881/tcp and 7882/udp there.

---

## Later: more than one voice server

Past roughly a thousand people in voice at once, or for voice servers in other countries, LiveKit can run several servers as one, sharing a small **Redis** database. Outhouse doesn't need to change for this, because it still talks to one address. That's a bigger job, so plan it with Claude when you get there.

## Costs (rough, Sydney VPS)

| Setup | Monthly |
|---|---|
| Everything on one VPS-2 | about A$12 |
| VPS-2 main + VPS-1 voice | about A$18 |
| VPS-2 main + VPS-2 voice (busy) | about A$24 |
