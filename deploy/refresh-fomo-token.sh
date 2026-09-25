#!/usr/bin/env bash
# TraderTony V4 — keep the FOMO (official) bearer token fresh
#
# The official provider (prod-api.fomo.family) authenticates with a Privy JWT
# that lives about an hour, and Cloudflare rejects non-browser clients from most
# datacentres. The bot re-reads FOMO_AUTH_TOKEN_FILE on every request, so all
# this script has to do is drop a fresh token into that file.
#
# There is no server-side login you can automate, so pick ONE of these:
#
#   A. Browser bookmarklet (simplest, manual — works from your phone)
#      Log in to fomo.family, open DevTools, and run:
#
#        copy(localStorage.getItem('privy:token'))     // or inspect a Network
#        // request to prod-api.fomo.family and copy its Authorization header
#
#      Then paste it into the file below (over SSH or the web console).
#
#   B. Token broker on a home machine with a residential IP (set up once)
#      A small always-on box at home (old laptop, Raspberry Pi, Android phone)
#      keeps a browser session and pushes the token to the bot:
#
#        ssh bot-host 'cat > /app/data/fomo_token.txt' <<< "$TOKEN"
#
#   C. Third-party mirror instead (no token juggling at all)
#      FOMO_PROVIDER=fomoapi + FOMO_API_KEY — server-friendly, key doesn't expire.
#      See docs/FOMO_COPY_TRADING.md §1.
#
# If you use A or B, install this as a cron job so an expired token is visible
# immediately instead of silently stopping the copier:
#
#   */5 * * * * /opt/trader-tony/deploy/refresh-fomo-token.sh --check >> /var/log/fomo-token.log 2>&1

set -euo pipefail

TOKEN_FILE="${FOMO_AUTH_TOKEN_FILE:-/app/data/fomo_token.txt}"
BASE_URL="${FOMO_API_BASE:-https://prod-api.fomo.family}"
MODE="${1:---check}"

log() { printf '%s %s\n' "$(date -Is)" "$*"; }

case "${MODE}" in
  --check)
    if [ ! -s "${TOKEN_FILE}" ]; then
      log "NO TOKEN at ${TOKEN_FILE} — the FOMO copier cannot authenticate."
      exit 1
    fi

    # Cheap authorised probe: a 429/200 means the token was accepted, 401/430 means it is dead.
    status="$(curl -s -o /dev/null -w '%{http_code}' \
      -H "authorization: Bearer $(tr -d '\r\n' < "${TOKEN_FILE}")" \
      -H 'origin: https://fomo.family' \
      -H 'referer: https://fomo.family/' \
      --max-time 15 \
      "${BASE_URL}/v2/leaderboard/24h?limit=1" || echo 000)"

    case "${status}" in
      200|429) log "token OK (HTTP ${status})" ;;
      430)     log "EDGE BLOCKED (HTTP 430) — this host cannot reach fomo.family; use FOMO_PROVIDER=fomoapi or a home broker" ; exit 2 ;;
      401|403) log "TOKEN EXPIRED OR INVALID (HTTP ${status}) — paste a fresh JWT into ${TOKEN_FILE}"; exit 1 ;;
      000)     log "NO RESPONSE (network/DNS) — check connectivity" ; exit 3 ;;
      *)       log "unexpected HTTP ${status}" ; exit 4 ;;
    esac
    ;;

  --set)
    # ./refresh-fomo-token.sh --set 'eyJhbGciOi…'
    [ -n "${2:-}" ] || { echo "usage: $0 --set <jwt>"; exit 1; }
    install -d -m 0755 "$(dirname "${TOKEN_FILE}")"
    printf '%s\n' "$2" > "${TOKEN_FILE}"
    chmod 600 "${TOKEN_FILE}"
    log "token written to ${TOKEN_FILE} ($(wc -c < "${TOKEN_FILE}") bytes)"
    ;;

  --status)
    log "file: ${TOKEN_FILE}"
    [ -s "${TOKEN_FILE}" ] && log "present, $(wc -c < "${TOKEN_FILE}") bytes, modified $(date -r "${TOKEN_FILE}" -Is)" \
      || log "missing or empty"
    ;;

  *)
    echo "usage: $0 [--check | --set <jwt> | --status]"
    exit 1
    ;;
esac
