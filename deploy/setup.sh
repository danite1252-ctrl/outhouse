#!/usr/bin/env bash
# One-time setup (safe to re-run) for Outhouse on Ubuntu.
#   cd ~/outhouse/deploy && ./setup.sh
# Installs Docker if needed, writes secrets, opens the firewall and starts
# everything. Re-running keeps your existing passwords and keys.
#   ./setup.sh --alerts    ask the uptime alert questions again
#   ./setup.sh --offsite   ask the off-site backup questions again
set -euo pipefail
cd "$(dirname "$0")"
ASK_ALERTS=0; ASK_OFFSITE=0
for arg in "$@"; do
  case $arg in
    --alerts) ASK_ALERTS=1 ;;
    --offsite) ASK_OFFSITE=1 ;;
  esac
done

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
if ! grep -q '^GITHUB_TOKEN=' .env; then
  say "App auto-updates (optional)"
  echo "The Windows and Android apps update themselves from this server, which"
  echo "fetches new builds from your private GitHub repo. That needs a GitHub"
  echo "fine-grained token with read-only access to the repo's Contents"
  echo "(github.com/settings/personal-access-tokens). Press Enter to skip."
  read -rp "GitHub token (paste once): " GITHUB_TOKEN
  read -rp "GitHub repo [danite1252-ctrl/outhouse]: " GITHUB_REPO
  echo "GITHUB_TOKEN=${GITHUB_TOKEN}" >> .env
  echo "GITHUB_REPO=${GITHUB_REPO:-danite1252-ctrl/outhouse}" >> .env
fi

# ---- Uptime alerts (optional) ----
if [ "$ASK_ALERTS" = 1 ] || ! grep -q '^ALERT_NTFY_URL=' .env; then
  sed -i '/^ALERT_NTFY_URL=/d; /^ALERT_WEBHOOK_URL=/d' .env
  say "Uptime alerts (optional)"
  echo "Every 5 minutes this server can check that Outhouse, voice and the nightly"
  echo "backup are OK, and tell you on your phone when something goes down or"
  echo "comes back. Easiest: install the free ntfy app, subscribe to a topic with a"
  echo "hard-to-guess name, and paste its address here, e.g."
  echo "  https://ntfy.sh/outhouse-$(openssl rand -hex 4)"
  echo "You can also (or instead) give a Discord-style webhook URL (a Discord or"
  echo "Outhouse channel webhook). Press Enter to skip either."
  read -rp "ntfy topic URL: " ALERT_NTFY_URL
  read -rp "Webhook URL: " ALERT_WEBHOOK_URL
  echo "ALERT_NTFY_URL=${ALERT_NTFY_URL}" >> .env
  echo "ALERT_WEBHOOK_URL=${ALERT_WEBHOOK_URL}" >> .env
fi

# ---- Off-site backup copies (optional) ----
if [ "$ASK_OFFSITE" = 1 ] || ! grep -q '^OFFSITE_DIR=' .env; then
  sed -i '/^OFFSITE_DIR=/d; /^OFFSITE_KEEP=/d; /^RCLONE_REMOTE=/d; /^RCLONE_KEEP=/d' .env
  say "Off-site backup copies (optional)"
  echo "Nightly backups live on this VM. A copy somewhere else survives the VM's"
  echo "disk dying. Two options (either, both or neither):"
  echo "  1. A shared folder on your Windows PC, e.g. //192.168.1.10/backups"
  echo "     (in Windows: right-click a folder > Properties > Sharing)."
  echo "  2. An rclone remote (Google Drive, OneDrive, Backblaze...) if you've set one"
  echo "     up with 'rclone config', e.g. gdrive:outhouse-backups"
  read -rp "Windows share (//PC/share, Enter to skip): " SMB_SHARE
  OFFSITE_DIR=""
  if [ -n "$SMB_SHARE" ]; then
    SMB_SHARE=${SMB_SHARE//\\//}
    read -rp "Windows user name for the share: " SMB_USER
    read -rsp "Password for $SMB_USER (not shown, kept in /etc/outhouse-backups.cred): " SMB_PASS; echo
    MOUNT=/mnt/outhouse-backups
    command -v mount.cifs >/dev/null 2>&1 || sudo apt-get install -y cifs-utils >/dev/null
    sudo mkdir -p "$MOUNT"
    printf 'username=%s\npassword=%s\n' "$SMB_USER" "$SMB_PASS" | sudo tee /etc/outhouse-backups.cred >/dev/null
    sudo chmod 600 /etc/outhouse-backups.cred
    unset SMB_PASS
    sudo sed -i "\| $MOUNT cifs |d" /etc/fstab
    echo "$SMB_SHARE $MOUNT cifs credentials=/etc/outhouse-backups.cred,uid=$(id -u),gid=$(id -g),file_mode=0600,dir_mode=0700,iocharset=utf8,nofail,_netdev,x-systemd.automount,x-systemd.mount-timeout=30 0 0" \
      | sudo tee -a /etc/fstab >/dev/null
    sudo systemctl daemon-reload || true
    if sudo mount "$MOUNT" 2>/dev/null || mountpoint -q "$MOUNT"; then
      echo "Mounted $SMB_SHARE at $MOUNT."
    else
      echo "Couldn't mount $SMB_SHARE yet (check the share name, user and password, and"
      echo "that the PC is on). Backups will report the failure until it works;"
      echo "re-run ./setup.sh --offsite to change it."
    fi
    OFFSITE_DIR=$MOUNT
  fi
  read -rp "rclone remote (e.g. gdrive:outhouse-backups, Enter to skip): " RCLONE_REMOTE
  if [ -n "$RCLONE_REMOTE" ] && ! command -v rclone >/dev/null 2>&1; then
    echo "rclone isn't installed: sudo apt install rclone, then 'rclone config' to add the remote."
  fi
  read -rp "How many copies to keep off-site [14]: " OFFSITE_KEEP
  {
    echo "OFFSITE_DIR=${OFFSITE_DIR}"
    echo "OFFSITE_KEEP=${OFFSITE_KEEP:-14}"
    echo "RCLONE_REMOTE=${RCLONE_REMOTE}"
    echo "RCLONE_KEEP=${OFFSITE_KEEP:-14}"
  } >> .env
fi

set -a; . ./.env; set +a
mkdir -p updates status
# The server reads backup status from here; make sure backup.sh can write it.
[ -w status ] || sudo chown "$USER" status

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

if [ -n "${GITHUB_TOKEN:-}" ]; then
  say "Fetching the latest app builds (then every 10 minutes)..."
  sed -i 's/\r$//' fetch-updates.py 2>/dev/null || true
  python3 fetch-updates.py || echo "Couldn't fetch app builds yet (is the token right?)."
fi

say "Scheduling a nightly backup (3:30 am, keeps 14)..."
mkdir -p "$HOME/outhouse-backups"
chmod +x backup.sh restore.sh healthcheck.sh 2>/dev/null || true
sed -i 's/\r$//' backup.sh restore.sh healthcheck.sh 2>/dev/null || true
( crontab -l 2>/dev/null | grep -v 'backup.sh\|fetch-updates.py\|healthcheck.sh' || true
  echo "30 3 * * * cd $(pwd) && ./backup.sh >> $HOME/outhouse-backups/backup.log 2>&1"
  echo "*/10 * * * * cd $(pwd) && python3 fetch-updates.py >> $HOME/outhouse-backups/updates.log 2>&1"
  if [ -n "${ALERT_NTFY_URL:-}${ALERT_WEBHOOK_URL:-}" ]; then
    echo "*/5 * * * * cd $(pwd) && ./healthcheck.sh >> $HOME/outhouse-backups/health.log 2>&1"
  fi ) | crontab -
if [ -n "${ALERT_NTFY_URL:-}${ALERT_WEBHOOK_URL:-}" ]; then
  echo "Uptime checks every 5 minutes; alerts go to ${ALERT_NTFY_URL:-}${ALERT_NTFY_URL:+ }${ALERT_WEBHOOK_URL:+(webhook)}."
fi

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
Off-site: ${OFFSITE_DIR:-no share}${RCLONE_REMOTE:+ + rclone $RCLONE_REMOTE}  (change: ./setup.sh --offsite)
Alerts:   ${ALERT_NTFY_URL:-${ALERT_WEBHOOK_URL:-off}}  (change: ./setup.sh --alerts; test: ./healthcheck.sh)
MSG
