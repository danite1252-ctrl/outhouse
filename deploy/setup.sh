#!/usr/bin/env bash
# One-time setup (safe to re-run) for Outhouse on Ubuntu.
#   cd ~/outhouse/deploy && ./setup.sh
# Installs Docker if needed, writes secrets, opens the firewall and starts
# everything. Re-running keeps your existing passwords and keys.
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1;33m%s\033[0m\n' "$*"; }

if ! command -v docker >/dev/null 2>&1; then
  say "Installing Docker..."
  curl -fsSL https://get.docker.com | sudo sh
  sudo usermod -aG docker "$USER" || true
fi
DOCKER="docker"
if ! docker info >/dev/null 2>&1; then DOCKER="sudo docker"; fi

if [ ! -f .env ]; then
  say "First-time settings"
  read -rp "Domain for Outhouse (e.g. outhouse.chat): " DOMAIN
  read -rp "Email for the HTTPS certificate (Let's Encrypt expiry notices): " ACME_EMAIL
  read -rp "Giphy API key (optional, Enter to skip): " GIPHY_API_KEY
  read -rp "Does another web server (like Apache) already handle HTTPS for this domain? [y/N]: " BEHIND
  if [[ "${BEHIND,,}" == y* ]]; then PROFILE=behind-proxy; else PROFILE=https; fi
  cat > .env <<ENV
COMPOSE_PROFILES=${PROFILE}
DOMAIN=${DOMAIN}
ACME_EMAIL=${ACME_EMAIL}
GIPHY_API_KEY=${GIPHY_API_KEY}
POSTGRES_PASSWORD=$(openssl rand -hex 24)
LIVEKIT_API_KEY=outhouse$(openssl rand -hex 4)
LIVEKIT_API_SECRET=$(openssl rand -hex 32)
ENV
  chmod 600 .env
  echo "Saved deploy/.env (keep it private; it holds your passwords)."
fi
set -a; . ./.env; set +a

LAN_IP=$(hostname -I | awk '{print $1}')

# LiveKit's settings (written from .env so the keys match the server's).
cat > livekit.yaml <<YAML
port: 7880
bind_addresses: ["0.0.0.0"]
rtc:
  tcp_port: 7881
  udp_port: 7882
  # Finds your public IP so people outside your home can connect...
  use_external_ip: true
  # ...and also offers the home-network address, so people at home connect directly.
  advertise_internal_ip: true
  # Home routers often can't "ping themselves" through the public IP; don't let that block startup.
  skip_external_ip_validation: true
  # Only use this server's real network address, not Docker's internal ones,
  # so the public IP gets paired with the right address.
  ips:
    includes:
      - ${LAN_IP}/32
keys:
  ${LIVEKIT_API_KEY}: ${LIVEKIT_API_SECRET}
logging:
  level: info
YAML
chmod 600 livekit.yaml

if command -v ufw >/dev/null 2>&1; then
  say "Opening firewall ports (voice 7881 TCP + 7882 UDP, plus web 80/443 unless behind Apache)..."
  sudo ufw allow OpenSSH >/dev/null
  if [ "${COMPOSE_PROFILES}" = "https" ]; then
    sudo ufw allow 80/tcp >/dev/null
    sudo ufw allow 443/tcp >/dev/null
    sudo ufw allow 443/udp >/dev/null
  fi
  sudo ufw allow 7881/tcp >/dev/null
  sudo ufw allow 7882/udp >/dev/null
  # Caddy (inside Docker) talks to LiveKit's signalling port on this machine.
  sudo ufw allow from 172.16.0.0/12 to any port 7880 proto tcp >/dev/null
  sudo ufw --force enable >/dev/null
fi

say "Building and starting Outhouse (the first build takes a few minutes)..."
$DOCKER compose up -d --build
# LiveKit only reads its settings at start-up.
$DOCKER compose restart livekit >/dev/null

say "Scheduling a nightly backup (3:30 am, keeps 14)..."
mkdir -p "$HOME/outhouse-backups"
chmod +x backup.sh restore.sh 2>/dev/null || true
sed -i 's/\r$//' backup.sh restore.sh 2>/dev/null || true
( crontab -l 2>/dev/null | grep -v 'deploy/backup.sh' || true
  echo "30 3 * * * cd $(pwd) && ./backup.sh >> $HOME/outhouse-backups/backup.log 2>&1" ) | crontab -

PUBLIC_IP=$(curl -fsS https://api.ipify.org || echo "unknown")
say "Done!"
cat <<MSG
Outhouse:        https://${DOMAIN}
This server:     ${LAN_IP} (home network)   ${PUBLIC_IP} (internet)

Still to do, if you haven't already:
MSG
if [ "${COMPOSE_PROFILES}" = "https" ]; then cat <<MSG
  1. DNS: point ${DOMAIN} (an A record) at ${PUBLIC_IP}
  2. Router: forward these ports to ${LAN_IP}
       80 TCP, 443 TCP+UDP   (website + certificate)
       7881 TCP, 7882 UDP    (voice)
  3. Give it a minute, then open https://${DOMAIN}

MSG
else cat <<MSG
  1. DNS: point ${DOMAIN} (an A record) at ${PUBLIC_IP}
  2. Router: forward 7881 TCP and 7882 UDP (voice) to ${LAN_IP}
     (80/443 stay pointed at your Apache server)
  3. Apache: add the site from deploy/README.md, proxying to http://${LAN_IP}:8080,
     and get its certificate with: sudo certbot --apache -d ${DOMAIN}
  4. Open https://${DOMAIN}

MSG
fi
cat <<MSG
Logs:     cd $(pwd) && $DOCKER compose logs -f
Backups:  nightly to ~/outhouse-backups (run ./backup.sh any time; ./restore.sh to restore)
MSG
