#!/usr/bin/env bash
# Turns a fresh Ubuntu 24.04 VM into a GitHub Actions runner for Outhouse CI
# (the Linux jobs: What changed, Server tests, Android APK, DAST, Security,
# Publish). Safe to re-run. See README.md in this folder.
#
#   sudo bash setup-linux-runner.sh
#
# Use a dedicated VM (4 vCPU, 8-12 GB RAM, 80-100 GB disk) or WSL2 Ubuntu 24.04
# on a Windows machine (run setup-wsl-host.ps1 on Windows first), NOT the
# Outhouse production VM: CI runs LiveKit on host ports 7880-7882, Postgres on
# 5432 and test servers on 8080/8097-8099, and kills leftovers on those ports.
#
# Inside WSL it needs systemd (it turns it on and asks you to restart WSL) and
# its own Docker Engine in the distro: Docker Desktop's WSL integration must
# be OFF for this distro, because CI uses `docker run --network host` and
# Docker Desktop's "host" is its own VM, not this distro.
#
# Options (environment variables):
#   RUNNER_USER=runner      the account the runner service runs as
#   RUNNER_SUDO=apt         apt  = passwordless sudo for apt-get only (default)
#                           none = no sudo at all
#                           full = passwordless sudo for everything
#   ANDROID_SDK=/opt/android-sdk
set -euo pipefail

RUNNER_USER=${RUNNER_USER:-runner}
RUNNER_SUDO=${RUNNER_SUDO:-apt}
ANDROID_SDK=${ANDROID_SDK:-/opt/android-sdk}
# Keep in step with the android job in .github/workflows/ci.yml.
ANDROID_NDK_VERSION=28.2.13676358
ANDROID_BUILD_TOOLS_VERSION=35.0.0
ANDROID_PLATFORMS="platforms;android-35 platforms;android-36"
CMDLINE_TOOLS_FALLBACK=13114758
REPO_URL=https://github.com/danite1252-ctrl/outhouse
HERE=$(cd "$(dirname "$0")" && pwd)

say() { printf '\n\033[1;33m== %s\033[0m\n' "$*"; }
[ "$(id -u)" = 0 ] || { echo "Run me with sudo: sudo bash $0"; exit 1; }
# shellcheck disable=SC1091
. /etc/os-release
[ "${ID:-}" = ubuntu ] || echo "Warning: made for Ubuntu 24.04, this is ${PRETTY_NAME:-unknown}."
[ "$(uname -m)" = x86_64 ] || { echo "Needs an x86_64 machine (the workflow downloads x64 tools)."; exit 1; }

WSL=no
if grep -qi microsoft /proc/version 2>/dev/null; then
  WSL=yes
  say "WSL2 detected"
  if ! grep -qE '^[[:space:]]*systemd[[:space:]]*=[[:space:]]*true' /etc/wsl.conf 2>/dev/null; then
    if grep -q '^\[boot\]' /etc/wsl.conf 2>/dev/null; then
      sed -i '/^\[boot\]/a systemd=true' /etc/wsl.conf
    else
      printf '\n[boot]\nsystemd=true\n' >> /etc/wsl.conf
    fi
  fi
  if [ "$(ps -p 1 -o comm= 2>/dev/null)" != systemd ]; then
    echo "systemd is now switched on in /etc/wsl.conf, but WSL needs a restart to use it:"
    echo "  in PowerShell on Windows:  wsl --shutdown"
    echo "  then open Ubuntu again and re-run:  sudo bash $0"
    exit 1
  fi
  # Docker Desktop's WSL integration puts its own `docker` here; CI needs a native engine.
  DOCKER_BIN=$(command -v docker 2>/dev/null || true)
  if [ -n "$DOCKER_BIN" ] && { readlink -f "$DOCKER_BIN" | grep -qi docker-desktop \
       || docker info --format '{{.OperatingSystem}}' 2>/dev/null | grep -qi 'docker desktop'; }; then
    echo "This distro is using Docker Desktop's docker. Turn it off for this distro first:"
    echo "  Docker Desktop -> Settings -> Resources -> WSL integration -> switch off this distro -> Apply & restart"
    echo "  then in PowerShell:  wsl --shutdown   and re-run this script."
    exit 1
  fi
fi
if ss -ltn 2>/dev/null | grep -qE ':(5432|8080|7880) '; then
  echo "Something already listens on 5432, 8080 or 7880 here. CI needs those ports (and kills"
  echo "leftovers on them), so use a dedicated VM or WSL distro, not the Outhouse server."
  exit 1
fi
[ "$WSL" = yes ] && echo "(WSL: also check on Windows that nothing listens on them: netstat -ano | findstr \":5432 :8080 :7880\")" 
export DEBIAN_FRONTEND=noninteractive

say "Packages (build tools, Python, Java 17, shellcheck, Postgres client...)"
apt-get update -qq
apt-get install -y -qq ca-certificates curl gnupg git jq unzip zip xz-utils build-essential pkg-config \
  libssl-dev python3 python3-pip python3-venv postgresql-client shellcheck psmisc \
  openjdk-17-jdk-headless gh >/dev/null

say "Docker"
if ! command -v docker >/dev/null 2>&1; then
  install -m 0755 -d /etc/apt/keyrings
  curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
  chmod a+r /etc/apt/keyrings/docker.asc
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu ${VERSION_CODENAME} stable" \
    > /etc/apt/sources.list.d/docker.list
  apt-get update -qq
  apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin >/dev/null
fi
systemctl enable --now docker >/dev/null

say "Node.js 22"
if ! node --version 2>/dev/null | grep -q '^v22\.'; then
  curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null
  apt-get install -y -qq nodejs >/dev/null
fi
node --version

say "User '$RUNNER_USER' (in the docker group; sudo: $RUNNER_SUDO)"
id "$RUNNER_USER" >/dev/null 2>&1 || useradd -m -s /bin/bash "$RUNNER_USER"
usermod -aG docker "$RUNNER_USER"
HOME_DIR=$(getent passwd "$RUNNER_USER" | cut -d: -f6)
case "$RUNNER_SUDO" in
  apt)  echo "$RUNNER_USER ALL=(root) NOPASSWD: /usr/bin/apt-get" > /etc/sudoers.d/outhouse-runner ;;
  full) echo "$RUNNER_USER ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/outhouse-runner ;;
  none) rm -f /etc/sudoers.d/outhouse-runner ;;
  *) echo "RUNNER_SUDO must be apt, full or none"; exit 1 ;;
esac
[ -f /etc/sudoers.d/outhouse-runner ] && { chmod 0440 /etc/sudoers.d/outhouse-runner; visudo -cf /etc/sudoers.d/outhouse-runner >/dev/null; }

say "Rust (rustup, for $RUNNER_USER)"
if [ ! -x "$HOME_DIR/.cargo/bin/rustup" ]; then
  sudo -iu "$RUNNER_USER" bash -c 'curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null'
fi
sudo -iu "$RUNNER_USER" bash -c '. ~/.cargo/env && rustup target add aarch64-linux-android armv7-linux-androideabi >/dev/null && rustc --version'

say "Android SDK in $ANDROID_SDK (NDK $ANDROID_NDK_VERSION, build-tools $ANDROID_BUILD_TOOLS_VERSION)"
mkdir -p "$ANDROID_SDK"
if ! ls "$ANDROID_SDK"/cmdline-tools/latest/bin/sdkmanager >/dev/null 2>&1; then
  REV=$(curl -fsSL https://dl.google.com/android/repository/repository2-3.xml 2>/dev/null \
        | grep -oE 'commandlinetools-linux-[0-9]+_latest\.zip' | grep -oE '[0-9]+' | sort -n | tail -1 || true)
  REV=${REV:-$CMDLINE_TOOLS_FALLBACK}
  TMP=$(mktemp -d)
  curl -fsSL -o "$TMP/tools.zip" "https://dl.google.com/android/repository/commandlinetools-linux-${REV}_latest.zip"
  unzip -q "$TMP/tools.zip" -d "$TMP"
  mkdir -p "$ANDROID_SDK/cmdline-tools"
  rm -rf "$ANDROID_SDK/cmdline-tools/latest"
  mv "$TMP/cmdline-tools" "$ANDROID_SDK/cmdline-tools/latest"
  rm -rf "$TMP"
fi
chown -R "$RUNNER_USER:$RUNNER_USER" "$ANDROID_SDK"
SM="$ANDROID_SDK/cmdline-tools/latest/bin/sdkmanager"
asrunner() { sudo -u "$RUNNER_USER" env JAVA_HOME="$(dirname "$(dirname "$(readlink -f "$(command -v javac)")")")" "$@"; }
yes | asrunner "$SM" --sdk_root="$ANDROID_SDK" --licenses >/dev/null 2>&1 || true
NDK_PKG="ndk;$ANDROID_NDK_VERSION"
if ! asrunner "$SM" --sdk_root="$ANDROID_SDK" --list 2>/dev/null | grep -q "^ *$NDK_PKG "; then
  NDK_PKG=$(asrunner "$SM" --sdk_root="$ANDROID_SDK" --list 2>/dev/null | grep -oE '^ *ndk;[0-9]+\.[0-9]+\.[0-9]+ ' | tr -d ' ' | sort -V | tail -1)
  echo "NDK $ANDROID_NDK_VERSION isn't offered any more; using $NDK_PKG (update ANDROID_NDK_VERSION in ci.yml to match)."
fi
# shellcheck disable=SC2086  # ANDROID_PLATFORMS is a list
asrunner "$SM" --sdk_root="$ANDROID_SDK" "platform-tools" "build-tools;$ANDROID_BUILD_TOOLS_VERSION" "$NDK_PKG" $ANDROID_PLATFORMS >/dev/null
ls "$ANDROID_SDK/ndk" "$ANDROID_SDK/build-tools" "$ANDROID_SDK/platforms"

say "Chrome's system libraries for the Playwright UI tests"
TMPV=$(mktemp -d)
python3 -m venv "$TMPV"
"$TMPV/bin/pip" install --quiet playwright
"$TMPV/bin/python" -m playwright install-deps chromium >/dev/null
rm -rf "$TMPV"
# The workflow skips `playwright install --with-deps` (which needs full sudo) when this exists.
printf 'outhouse CI runner, set up %s\n' "$(date -Is)" > /etc/outhouse-ci-runner

say "Clean-up hook and weekly disk clean-up"
mkdir -p /opt/outhouse-ci
install -m 0755 "$HERE/job-started.sh" /opt/outhouse-ci/job-started.sh
install -m 0755 "$HERE/cleanup-runner.sh" /opt/outhouse-ci/cleanup-runner.sh
cat > /etc/systemd/system/outhouse-ci-cleanup.service <<EOF
[Unit]
Description=Outhouse CI runner disk clean-up
[Service]
Type=oneshot
Environment=RUNNER_USER=$RUNNER_USER
ExecStart=/opt/outhouse-ci/cleanup-runner.sh
EOF
cat > /etc/systemd/system/outhouse-ci-cleanup.timer <<'EOF'
[Unit]
Description=Weekly Outhouse CI runner disk clean-up
[Timer]
OnCalendar=Sun 04:00
Persistent=true
[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now outhouse-ci-cleanup.timer >/dev/null

say "GitHub Actions runner in $HOME_DIR/actions-runner"
RDIR="$HOME_DIR/actions-runner"
if [ ! -x "$RDIR/config.sh" ]; then
  VER=$(curl -fsSL https://api.github.com/repos/actions/runner/releases/latest | jq -r .tag_name | sed 's/^v//')
  [ -n "$VER" ] && [ "$VER" != null ] || { echo "Couldn't find the latest runner version"; exit 1; }
  sudo -u "$RUNNER_USER" mkdir -p "$RDIR"
  curl -fsSL "https://github.com/actions/runner/releases/download/v$VER/actions-runner-linux-x64-$VER.tar.gz" \
    | sudo -u "$RUNNER_USER" tar xz -C "$RDIR"
  "$RDIR/bin/installdependencies.sh" >/dev/null
fi
# Settings the runner service reads at start (config.sh keeps existing lines in .env).
ENVF="$RDIR/.env"
touch "$ENVF"
setenv() { grep -q "^$1=" "$ENVF" && sed -i "s#^$1=.*#$1=$2#" "$ENVF" || echo "$1=$2" >> "$ENVF"; }
setenv ANDROID_HOME "$ANDROID_SDK"
setenv ANDROID_SDK_ROOT "$ANDROID_SDK"
setenv ACTIONS_RUNNER_HOOK_JOB_STARTED /opt/outhouse-ci/job-started.sh
chown "$RUNNER_USER:$RUNNER_USER" "$ENVF"

if [ "$WSL" = yes ]; then
  say "WSL notes"
  echo "Keep the distro running after reboots with setup-wsl-host.ps1 (Windows side)."
  echo "Make sure Docker Desktop's WSL integration stays OFF for this distro."
fi

say "Done. Now register the runner (one time):"
cat <<EOF

 1. On GitHub: $REPO_URL/settings/actions/runners/new  (Linux, x64)
    copy the token from the "Configure" box (it's valid for an hour).

 2. Here:
      sudo -iu $RUNNER_USER
      cd ~/actions-runner
      ./config.sh --url $REPO_URL --token <TOKEN> --labels outhouse --name outhouse-linux --unattended
      exit
      cd $RDIR && sudo ./svc.sh install $RUNNER_USER && sudo ./svc.sh start

 3. Check it shows "Idle" on $REPO_URL/settings/actions/runners
    then set the repository variables (see README.md):
      RUNNER_LINUX = ["self-hosted","linux","outhouse"]

 Disk clean-up runs every Sunday 04:00 (systemctl list-timers outhouse-ci-cleanup.timer).
EOF
