# FOMO leaderboard copy-trading

Copies **the top member of the top clan** on fomo.family's leaderboard: once a
day the bot finds the best clan, picks that clan's best trader, then mirrors
their Solana swaps (buy → enter, sell → exit). It runs in **live**, **dry-run**
and **demo** mode.

```
                daily (FOMO_REFRESH_SECS, default 24h)
   ┌──────────────────────────────────────────────────────────────┐
   │ 1. clan leaderboard  →  rank 1 clan       (e.g. Conviction)  │
   │ 2. clan members      →  rank 1 by PnL     (e.g. @MrMetavers3)│
   └──────────────────────────────────────────────────────────────┘
                                │
                                ▼  every FOMO_POLL_SECS (default 60s)
              poll the copied trader's swaps
                                │
              ┌─────────────────┴──────────────────┐
              ▼                                    ▼
     buy  → open a mirrored position       sell → close our position
     (fixed size or a % of their notional)       in that mint
```

Every entry still passes the [risk guard](ARENA_ROADMAP.md#6-track-a--how-to-use-it),
so the daily loss cap, drawdown breaker, kill switch and per-token cooldown apply
to copied trades exactly as they do to scanned ones.

---

## 1. Pick a data source first

fomo.family has **no public API**. The app talks to `https://prod-api.fomo.family`
with a Privy JWT (~1 h TTL), and Cloudflare rejects most non-browser clients at
the edge (`430 {"error":"unauthorized"}`). Independent projects hit the same wall
even when replaying a real browser's headers.

So the bot supports three providers and you should probe yours before configuring:

```bash
node tools/fomo-probe.js --key <fomoapi-key>          # third-party mirror
node tools/fomo-probe.js --token <jwt> --cookie '__cf_bm=…'   # official session
node tools/fomo-probe.js --base https://my-proxy.example.com --key k
```

The probe prints, per provider: HTTP status, row counts, the top clan and top
member it would pick, and the fields of a sample swap. Use `--dump` to print raw
payloads.

| `FOMO_PROVIDER` | Base URL | Auth | Clan board | Notes |
|---|---|---|---|---|
| `official` | `https://prod-api.fomo.family` | `FOMO_AUTH_TOKEN` (JWT) + optional `FOMO_CF_COOKIE` | ✅ native | Highest fidelity. Often 430 from datacentres; works from some residential/VPS IPs. **The JWT lives ~1 h** — use `FOMO_AUTH_TOKEN_FILE` so a helper script can rewrite it without restarting the bot. |
| `fomoapi` | `https://api.fomoapi.io` | `FOMO_API_KEY` (free key) | ⚠️ derived | Documented third-party mirror; server-friendly. No clan board, so the bot **derives** clans by summing the traders' `clan` field. |
| `custom` | `FOMO_API_BASE` | `FOMO_API_KEY` | as served | Anything that speaks the same JSON: your own proxy, an extension bridge, a cache you refresh from the browser console. |

`FOMO_CLAN_MEMBERS_PATH` defaults to `/v2/clans/{clan_id}/members`; `{clan_id}`
and `{window}` are substituted, so a different provider path only needs a config
change.

**Keeping the official session alive.** The JWT expires after about an hour, so a
24/7 bot should not read it once at boot. Point `FOMO_AUTH_TOKEN_FILE` at a file
and refresh it from wherever you can get a fresh token (a browser helper, a
cron job, a proxy). The bot re-reads the file on **every request** and accepts
either a bare token or a `Bearer <token>` line:

```bash
FOMO_AUTH_TOKEN_FILE=/app/data/fomo_token.txt
# refresh helper (runs hourly):
#   echo "$NEW_JWT" > /app/data/fomo_token.txt
```

**If nothing works from your host**, pin the target instead — the bot then needs
only the trader's swaps:

```bash
FOMO_TRADER_HANDLE=MrMetavers3    # or FOMO_TRADER_ID=<uuid>, or FOMO_CLAN_ID=<uuid>
```

## 2. Configure

```bash
FOMO_PROVIDER=fomoapi
FOMO_API_KEY=fomo_live_xxx

FOMO_WINDOW=24h              # 24h | 7d | 30d | all
FOMO_REFRESH_SECS=86400      # re-run clan/member discovery daily
FOMO_POLL_SECS=60            # how often to poll the copied trader's swaps

FOMO_COPY_SIZE_SOL=0.05      # fixed size per copied buy
FOMO_SIZE_MODE=fixed         # fixed | proportional
FOMO_COPY_RATIO=0.02         # proportional: fraction of their notional (2%)
FOMO_MIN_SWAP_USD=25         # ignore dust trades
FOMO_MAX_POSITIONS=5         # concurrent copied positions
FOMO_MIRROR_SELLS=true       # close our position when they sell
FOMO_SOLANA_NETWORK_IDS=1399811149,101,solana
```

With `FOMO_SIZE_MODE=proportional` the bot converts the copied trade's USD
notional to SOL (live price) and spends `FOMO_COPY_RATIO` of it, capped by the
strategy's `max_position_size_sol`.

## 3. Switch it on

Set the active strategy to **FOMO Leaderboard Copy** — from the dashboard
`Strategy` selector, or:

```bash
ACTIVE_STRATEGY=fomocopy          # aliases: fomo, fomo_copy, copytrade
```

Then start the bot. It resolves the top clan + top member immediately, from the
next poll onward it mirrors new swaps. Exits, stop-loss/take-profit and the
trailing stop all still apply to copied positions, so a copied trade can also be
closed by the normal rules if the trader never sells.

```
GET  /api/fomo/status     who we copy, counters, recent decisions, last error
POST /api/fomo/refresh    re-run discovery now (instead of waiting for the day)
POST /api/fomo/target     {"clan_id": "…"} or {"trader": "handle"} to pin a target
```

The dashboard shows a **FOMO COPY** card (bottom-left) with the target, the
mirror counters and the decision feed.

## 4. Modes

| Mode | What a mirrored buy does |
|---|---|
| `DEMO_MODE=true` | Creates a simulated demo position (`PositionManager::create_demo_position`). Nothing is signed. |
| `DRY_RUN_MODE=true` | Records a simulated position through `SimulationManager`, like the token scanners do. |
| neither | Real Jupiter swap through the shared `execute_buy_task` path, with the strategy's slippage/priority fee. |

Copied sells always call `PositionManager::close_positions_by_token`, which
market-sells the position in every mode (demo closes are simulated).

## 5. What it deliberately does not do

- **No EVM copying.** Only Solana legs produce a signal — the bot trades Solana.
  A trader who moves to Base/Robinhood is not followed. (The parser knows the
  fomo network ids `1399811149`/`101` = Solana; extend
  `FOMO_SOLANA_NETWORK_IDS` if that changes.)
- **No pyramiding.** One copied position per mint; repeated buys are skipped.
- **No wallet mirroring.** fomo reports a *profile* address that holds nothing,
  and trades through a per-user execution wallet. Copying the wallet on-chain is
  a possible future track, but it needs its own discovery path.
- **No margin for stale data.** Every swap is processed once (persisted in
  `data/fomo_state.json`), so a restart never re-fires an old trade even if the
  upstream returns it again.
- **Not financial advice.** A 24h leaderboard leader is often a trader who just
  got lucky on one token. `FOMO_WINDOW=7d` or `30d` is the more conservative
  choice.
