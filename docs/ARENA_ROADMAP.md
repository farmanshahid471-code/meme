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
