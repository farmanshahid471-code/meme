# Arena Roadmap — TraderTony V4 fork (`farmanshahid471-code/meme`)

Working branch: **`arena/01a0d052-meme`**
Upstream imported: **`tony-42069/trader-tony-v4` @ `bd6ad77`** (Merge PR #33, Telegram sniper)
To pull future upstream changes: `git remote add upstream https://github.com/tony-42069/trader-tony-v4.git && git fetch upstream && git merge upstream/master`

---

## 1. What the bot does today

| Area | State |
|---|---|
| Runtime | Rust / Tokio, Axum REST API + WebSocket, ~14.2k LOC in `src/` |
| Discovery | 4 strategy types: `NewPairs` (pump.fun create events), `FinalStretch` (bonding curve + traction), `Migrated` (graduated to PumpSwap/Raydium), `TelegramCall` (channel call-out sniper) |
| Risk | `src/trading/risk.rs` — mint/freeze authority, LP burned, holder concentration, transfer tax, honeypot/sellability |
| Sizing | Per-strategy `max_position_size_sol`, `total_budget_sol`, `max_concurrent_positions` |
| Exits | SL %, TP %, trailing-stop %, max hold time (`src/trading/position.rs:522`) |
| Modes | `DEMO_MODE` (simulated), `DRY_RUN_MODE` (real scan, simulated fills), live |
| UI | `webapp/` static HTML/JS dashboard, pointed at a hard-coded backend in `webapp/js/config.js` |
| Extras | Copy-trade registry + 10% profit fee, `/api/simulation/*` paper positions, $TONY buyback config |

## 2. Gaps found in the code

1. **No authentication on the API and `CORS = *`** (`src/web/server.rs:19-22`). Anyone who finds the host can `POST /api/autotrader/start|stop`, read `/api/wallet`, or change strategy params. This is the single biggest risk for real money.
2. **All-or-nothing exits** (`src/trading/position.rs:522-557`). One TP price; no partial/laddered exits (sell 25% at 2x, 25% at 5x…), no breakeven move after a pump.
3. **Trailing stop armed at entry**, not after profit (`position.rs:495-505`). It reacts to any tick down from entry instead of only after +X%.
4. **No loss controls**: no daily loss limit, no max-drawdown kill switch, no "pause after N consecutive losses", no per-token cooldown after an exit (grep for `daily_loss|kill_switch|drawdown|circuit` → 0 hits).
5. **No outbound notifications**: draws nothing from Discord/Telegram-bot/webhooks on fills, exits or errors (`src/models/user.rs` has `notify_*` flags that are never used).
6. **No journal/export/analytics**: `/api/stats` only; no CSV/JSON export, no per-strategy PnL breakdown, win-rate/expectancy/hold-time metrics.
7. **No execution-quality features**: no Jito bundle / MEV protection, no dynamic priority-fee, no slippage retry ladder (grep `jito|mev` → 0 hits).
8. **No dev-wallet / bundle detection**: risk analysis doesn't check the creator wallet, insider holdings or bundled buys (grep `dev_wallet|insider|bundle` → 0 hits).
9. **Copy trading is signals-only** (`src/web/copy_trade.rs`): no multi-wallet fan-out, no proportional sizing, no per-follower risk caps.
10. `sled` is a declared dependency but unused — state is JSON files under `data/`.

## 3. Proposed feature tracks

### A. Safety & risk controls (highest value for live money)
- API key / bearer-token auth middleware + CORS allow-list (`API_KEY`, `CORS_ORIGINS` already exists as a var but is ignored — the layer hard-codes `Any`).
- Global kill switch (`POST /api/emergency/stop`) that halts scanning, cancels pending entries and optionally flattens open positions.
- Daily loss cap + max-drawdown breaker: stop trading for the rest of the day when realized PnL ≤ −X SOL / −Y %.
- Per-token cooldown and max trades/day so the bot can't re-buy the same rug repeatedly.

### B. Smarter exits
- Laddered take-profit: N tiers as `(price_multiple, sell_percent)` (e.g. 2x → 25%, 5x → 50%, remainder trails).
- Breakeven stop: after +T%, move SL to entry (or entry + fees).
- Trailing-stop activation threshold: only trail once profit ≥ X%.
- Momentum/time exits: dump if volume/price stalls, or if the token fails to move within N minutes.

### C. Alerts & analytics
- Outbound alerts to Discord webhook / Telegram Bot API / generic webhook on entry, exit, SL/TP, RPC errors, kill-switch trips.
- Trade journal: append-only JSONL of every fill + exit reason, `GET /api/journal/export?format=csv`.
- Richer stats: win rate, expectancy, avg/best/worst trade, per-strategy and per-day PnL.

### D. Entry & discovery upgrades
- Dev-wallet analysis: creator's share, who else funded them, whether the dev sold.
- Bundle/sniper-bot detection on the first slots.
- Dynamic priority fee based on recent network congestion; optional Jito tip for landing on snipes.
- Multi-wallet copy-trade fan-out with per-follower caps.

## 4. Environment limits (important)

- This sandbox has **no Rust toolchain** and its egress allow-list blocks `static.rust-lang.org`, `crates.io`, Solana/Jupiter/Birdeye endpoints. So Rust changes are **compiled and unit-tested in GitHub Actions** (`.github/workflows/arena-verify.yml`, runs `cargo check --all-targets` + `cargo test` on every push to this branch) — they are *not* run against live Solana from here.
- Live/paper validation (dry run, demo mode) has to happen on your machine or VPS:
  `cp .env.example .env && DEMO_MODE=true DRY_RUN_MODE=true cargo run --release`.
- Node.js and Python *are* available here, so a mock backend + live-previewable dashboard harness can be built if we want to iterate on the UI/analytics without running the Rust bot.

---

## 5. Status

| Track | State | Notes |
|-------|-------|-------|
| **A — Safety & risk controls** | ✅ implemented | see §6 for the API; verified by CI (`cargo check --all-targets` + `cargo test`, 84 tests) |
| **F — FOMO leaderboard copy** | ✅ implemented | new `FomoCopy` strategy: top clan → top member → mirror their swaps. See [FOMO_COPY_TRADING.md](FOMO_COPY_TRADING.md) |
| B — Smarter exits | ⬜ next | laddered TP, breakeven stop, trailing activation threshold, momentum/time exits |
| C — Alerts & analytics | ⬜ planned | Discord/Telegram/webhook alerts, trade journal + CSV export, richer stats |
| D — Entry & discovery | ⬜ planned | dev-wallet/bundle detection, dynamic priority fees, Jito tips, copy-trade fan-out |

## 6. Track A — how to use it

### Configure

```bash
API_KEY=$(openssl rand -hex 32)      # required on every call except /api/health
CORS_ORIGINS=https://your-dashboard.example.com
DAILY_LOSS_LIMIT_SOL=0.10            # stop for the day at -0.10 SOL realised
MAX_DRAWDOWN_PERCENT=25              # stop when equity is 25% below its peak
MAX_TRADES_PER_DAY=10                # entry cap per UTC day
MAX_CONSECUTIVE_LOSSES=3             # pause after 3 losers in a row
TOKEN_COOLDOWN_MINUTES=30            # no re-entry into a mint for 30 min after exit
EMERGENCY_FLATTEN_POSITIONS=false    # kill switch: sell everything, or just stop?
```

Leaving a rail empty disables it. With `API_KEY` unset the API stays open (as
before) but the bot logs a loud warning at boot.

### Endpoints

| Endpoint | Purpose |
|----------|---------|
| `GET /api/risk/status` | rails, today's PnL, entry counts, losing streak, equity vs peak, refused entries, mints in cooldown |
| `POST /api/risk/reset[?clear_counters=true]` | clear a halt (kill switch, daily-loss pause, drawdown breaker) |
| `POST /api/emergency/stop[?flatten=true&reason=...]` | kill switch: block entries immediately, optionally market-sell everything, stop the loops |

Authentication: `X-API-Key: <key>`, `Authorization: Bearer <key>`, or `?api_key=<key>`
for the WebSocket. `/api/health` stays public for uptime probes.

### Behaviour notes

- The rails gate **entries only** — stop loss, take profit, trailing stop and the
  kill switch keep working on open positions, and a halted bot still manages them.
- Every automated entry is checked immediately before the swap (scan cycle,
  Moralis/Final-Stretch scanner, manual dashboard buys, Telegram sniper), so a
  refusal costs nothing but a log line.
- Realised PnL from each closed position feeds the guard; the daily counters roll
  over at UTC midnight, while a manual halt persists until `/api/risk/reset`.
- The dashboard ships `webapp/js/risk.js`: a floating card with the live rail
  state, the kill switch and an API-key field.

### Previewing the UI without the bot

```bash
node tools/dev-harness/server.js     # http://localhost:8080 — mock backend + real dashboard
```

See `tools/dev-harness/README.md`.

## 7. Track F — FOMO leaderboard copy-trading

Full guide: **[FOMO_COPY_TRADING.md](FOMO_COPY_TRADING.md)**.

What it does, once a day (configurable): reads fomo.family's **clan leaderboard**,
takes rank 1, reads that clan's **members**, takes rank 1 by PnL, then polls that
trader's swaps and mirrors them (buy → entry, sell → exit). Runs in live,
dry-run **and demo** mode; every copied entry passes the Track A risk guard.

| Piece | Where |
|---|---|
| Data client (3 providers, tolerant parsing) | `src/api/fomo.rs` |
| Discovery + mirror engine, pure `decide()` | `src/trading/fomo_copy.rs` |
| Strategy type `FomoCopy` + factory defaults | `src/trading/strategy.rs` |
| Dashboard card | `webapp/js/fomo.js` |
| Host-probe tool (run this first) | `tools/fomo-probe.js` |
| API | `GET /api/fomo/status`, `POST /api/fomo/refresh`, `POST /api/fomo/target` |

**Important constraint:** fomo.family has no public API and Cloudflare blocks most
non-browser clients (HTTP 430) from datacentres, so the bot supports three data
sources (`official`, `fomoapi` mirror, `custom` proxy) and lets you pin a clan or
trader by hand. Run `node tools/fomo-probe.js` on the VPS to see what your host
can reach.
