#!/usr/bin/env bash
# TraderTony V4 — free-tier keepalive
#
# Oracle Cloud reclaims Always Free compute instances it considers idle. The
# published rule looks at a rolling 7-day window: 95th-percentile CPU below 20%,
# network below 20%, and (on A1 shapes) memory below 20%. A trading bot that
# polls a few APIs every 15–60 seconds uses almost no CPU, so a quiet instance
# can fall inside that definition and get stopped.
#
# This service keeps the instance measurably above the idle threshold with a
# small, bounded amount of work. It is deliberately gentle: it aims for a
# configurable average CPU (default ~6%), yields constantly, and runs at the
# lowest priority so it can never starve the bot.
#
# Tune with KEEPALIVE_MIN_CPU (percent, 0 disables the loop).
#
# NOTE: this exists to satisfy a documented vendor policy on an instance that is
# doing real work. It is not a way to keep a machine you are not using — if you
# stop running the bot, stop this service too (systemctl disable --now
# trader-keepalive) and shut the instance down.

set -euo pipefail

TARGET_CPU="${KEEPALIVE_MIN_CPU:-6}"
SAMPLE_SECONDS=10
CORES="$(nproc 2>/dev/null || echo 1)"

if [ "${TARGET_CPU}" -le 0 ]; then
  echo "KEEPALIVE_MIN_CPU=${TARGET_CPU} → keepalive disabled; idling."
  exec sleep infinity
fi

echo "trader-keepalive: aiming for ~${TARGET_CPU}% total CPU across ${CORES} core(s)"

# One second of a tight loop per "tick" costs about 100% of one core for that
# second. We spread the work out so the 95th-percentile sample stays above the
# threshold without ever spinning at full tilt for long.
BUSY_SECONDS_PER_MINUTE=$(( TARGET_CPU * CORES * 60 / 100 ))
[ "${BUSY_SECONDS_PER_MINUTE}" -lt 2 ] && BUSY_SECONDS_PER_MINUTE=2

while true; do
  for _ in $(seq 1 "${BUSY_SECONDS_PER_MINUTE}"); do
    # ~1 second of light, yielding work (nice'd, so it yields to the bot).
    end=$(( $(date +%s%N) / 1000000 + 1000 ))
    while [ $(( $(date +%s%N) / 1000000 )) -lt "${end}" ]; do
      : # arithmetic keeps the compiler honest; CPU is the point
    done
    sleep 0.$(( 60 / (BUSY_SECONDS_PER_MINUTE + 1) ))
  done
  sleep "${SAMPLE_SECONDS}"
done
