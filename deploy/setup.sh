#!/usr/bin/env bash
# TraderTony V4 — one-shot bootstrap for a fresh Ubuntu/Debian box
# (Oracle Cloud Always Free ARM, GCP e2-micro, an old laptop, a phone in Termux…)
#
#   curl -fsSL https://raw.githubusercontent.com/<you>/meme/arena/01a0d052-meme/deploy/setup.sh | bash
#   # or, from a checkout:
#   sudo bash deploy/setup.sh
#
# What it does
#   1. installs Docker + compose plugin (and builds from this checkout)
#   2. creates ./data (persistent bot state) and a .env from .env.example
#   3. generates a strong API_KEY and enables the safe defaults
#   4. adds swap on small machines (the Rust build needs it on a 1 GB box)
#   5. installs an Oracle idle-reclamation keepalive (harmless elsewhere)
#   6. starts the bot with restart-on-boot and log rotation
#
# It never writes your exchange/API keys — you do that in .env afterwards.

set -euo pipefail

# Works both as `sudo bash deploy/setup.sh` and as `curl … | sudo bash`
# (in the piped case there is no script path, so fall back to the cwd).
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
  REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
else
  REPO_DIR="$(pwd)"
fi

if [ ! -f "${REPO_DIR}/Cargo.toml" ] || [ ! -f "${REPO_DIR}/deploy/docker-compose.yml" ]; then
  die "Run this from the repository root (or from deploy/): no Cargo.toml / deploy/docker-compose.yml under ${REPO_DIR}. Clone the repo first:
    git clone -b arena/01a0d052-meme https://github.com/farmanshahid471-code/meme.git && cd meme"
fi
DATA_DIR="${REPO_DIR}/deploy/data"
ENV_FILE="${REPO_DIR}/.env"
KEEPALIVE_MIN_CPU="${KEEPALIVE_MIN_CPU:-6}"   # % CPU to keep Oracle from reclaiming

log()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[!]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[x]\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "Run with sudo (needs to install packages)."

log "Detected: $(uname -s) $(uname -m) — $(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME" || echo unknown)"

# --------------------------------------------------------------------------
# 1. Docker
# --------------------------------------------------------------------------
if ! command -v docker >/dev/null 2>&1; then
  log "Installing Docker..."
  apt-get update -qq
  apt-get install -y -qq ca-certificates curl gnupg git
  install -m 0755 -d /etc/apt/keyrings
  curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc 2>/dev/null \
    || curl -fsSL https://download.docker.com/linux/debian/gpg -o /etc/apt/keyrings/docker.asc
  chmod a+r /etc/apt/keyrings/docker.asc
  CODENAME="$(. /etc/os-release && echo "${VERSION_CODENAME:-bookworm}")"
  DISTRO="$(. /etc/os-release && echo "${ID:-ubuntu}")"
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/${DISTRO} ${CODENAME} stable" \
    > /etc/apt/sources.list.d/docker.list
  apt-get update -qq
  apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
  systemctl enable --now docker
else
  log "Docker already installed: $(docker --version)"
fi

# --------------------------------------------------------------------------
# 2. Swap (a 1 GB free-tier box cannot link the Rust release build without it)
# --------------------------------------------------------------------------
if [ "$(free -m | awk '/^Swap:/ {print $2}')" -lt 1024 ]; then
  if [ ! -f /swapfile ]; then
    log "Creating a 2 GB swap file (needed to build/link the bot on small boxes)..."
    fallocate -l 2G /swapfile || dd if=/dev/zero of=/swapfile bs=1M count=2048
    chmod 600 /swapfile
    mkswap /swapfile >/dev/null
    swapon /swapfile
    grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
    sysctl -w vm.swappiness=10 >/dev/null
    grep -q 'vm.swappiness' /etc/sysctl.conf || echo 'vm.swappiness=10' >> /etc/sysctl.conf
  fi
else
  log "Swap already configured ($(free -m | awk '/^Swap:/ {print $2}') MB)"
fi

# --------------------------------------------------------------------------
# 3. Data directory + .env
# --------------------------------------------------------------------------
log "Preparing ${DATA_DIR} (persistent bot state)..."
mkdir -p "${DATA_DIR}"
# The container runs as uid 1000 ("trader").
chown -R 1000:1000 "${DATA_DIR}"

if [ -f "${ENV_FILE}" ]; then
  log ".env already exists — leaving it untouched"
else
  log "Creating .env from .env.example with a generated API_KEY"
  cp "${REPO_DIR}/.env.example" "${ENV_FILE}"
  API_KEY_VALUE="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  sed -i "s|^API_KEY=.*|API_KEY=${API_KEY_VALUE}|" "${ENV_FILE}"
  # Safe defaults for a first boot.
  sed -i 's|^DEMO_MODE=.*|DEMO_MODE=true|' "${ENV_FILE}"
  sed -i 's|^DRY_RUN_MODE=.*|DRY_RUN_MODE=true|' "${ENV_FILE}"
  sed -i 's|^AUTO_START_TRADING=.*|AUTO_START_TRADING=true|' "${ENV_FILE}"
  sed -i 's|^CORS_ORIGINS=.*|CORS_ORIGINS=*|' "${ENV_FILE}"
  chmod 600 "${ENV_FILE}"
  warn "Fill in SOLANA_RPC_URL / WALLET_PRIVATE_KEY / HELIUS_API_KEY in ${ENV_FILE}, then:"
  warn "  cd ${REPO_DIR}/deploy && docker compose up -d --build"
  warn "Your dashboard API key is: ${API_KEY_VALUE}"
fi

# --------------------------------------------------------------------------
# 4. Oracle idle-reclamation keepalive
# --------------------------------------------------------------------------
if [ "${KEEPALIVE_MIN_CPU}" -gt 0 ]; then
  log "Installing keepalive (targets ~${KEEPALIVE_MIN_CPU}% CPU, defeats Oracle's idle reclamation)"
  install -m 0755 "${REPO_DIR}/deploy/keepalive.sh" /usr/local/bin/trader-keepalive.sh
  cat > /etc/systemd/system/trader-keepalive.service <<EOF
[Unit]
Description=TraderTony keepalive (keeps the free-tier instance out of Oracle's idle-reclamation rule)
After=network.target

[Service]
Type=simple
Environment=KEEPALIVE_MIN_CPU=${KEEPALIVE_MIN_CPU}
ExecStart=/usr/local/bin/trader-keepalive.sh
Restart=always
RestartSec=30
Nice=19

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable --now trader-keepalive.service
fi

# --------------------------------------------------------------------------
# 5. Start the bot
# --------------------------------------------------------------------------
cd "${REPO_DIR}/deploy"
if grep -q 'YOUR_' ../.env 2>/dev/null; then
  warn "Skipping 'docker compose up': .env still contains placeholder values."
  warn "Edit ${ENV_FILE}, then run: cd ${REPO_DIR}/deploy && docker compose up -d --build"
else
  log "Building and starting the bot (first build takes a while on 1–2 cores)..."
  docker compose up -d --build
  log "Waiting for the container to report healthy..."
  sleep 20
  docker compose ps
  curl -fsS http://127.0.0.1:3030/api/health && echo
fi

log "Done. Useful commands:"
cat <<EOF
  docker compose -f ${REPO_DIR}/deploy/docker-compose.yml logs -f      # follow logs
  docker compose -f ${REPO_DIR}/deploy/docker-compose.yml restart      # restart
  docker compose -f ${REPO_DIR}/deploy/docker-compose.yml pull         # update image
  curl -s localhost:3030/api/risk/status -H "X-API-Key: \$(grep ^API_KEY= ${ENV_FILE} | cut -d= -f2)"
EOF
