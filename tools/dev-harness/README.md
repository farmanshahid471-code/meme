# Dev harness — dashboard preview without the Rust bot

Runs the real `webapp/` dashboard against a mock backend that speaks the same
API as the bot, so UI/API changes can be reviewed in a browser before deploying.
No dependencies, no build step: plain Node (18+).

```bash
node tools/dev-harness/server.js          # http://localhost:8080
PORT=9000 node tools/dev-harness/server.js
```

## What it fakes

| Area | Notes |
|------|-------|
| REST API | `/api/health`, `/api/wallet`, `/api/stats`, `/api/positions`, `/api/trades`, `/api/autotrader/*`, `/api/strategy/active`, `/api/watchlist*`, `/api/simulation/*`, `/api/analyze`, `/api/status` — response shapes copied from `src/web/models.rs` |
| Risk guard | `/api/risk/status`, `/api/risk/reset`, `/api/emergency/stop?flatten=` with the same rails and semantics as `src/trading/risk_guard.rs` |
| Market | Position prices do a random walk every tick; stop-loss, take-profit and trailing stops close positions and append to `/api/trades` |
| WebSocket | `/ws` (hand-rolled RFC 6455, no `ws` dependency) pushes `status_update`, `price_update`, `position_opened`, `position_closed` |
| Auth | Set `HARNESS_API_KEY` to make the harness behave like a bot started with `API_KEY`: everything except `/api/health` returns 401 without credentials |
| Dashboard config | `js/config.js` is patched on the fly so `API_BASE_URL` points at the harness (same origin) instead of the author's Fly.io backend |

## Useful env vars

```bash
HARNESS_API_KEY=dev-key node tools/dev-harness/server.js    # exercise the 401 path
DAILY_LOSS_LIMIT_SOL=0.05 node tools/dev-harness/server.js  # watch the daily-loss rail trip
MAX_DRAWDOWN_PERCENT=10 node tools/dev-harness/server.js
MAX_TRADES_PER_DAY=3 node tools/dev-harness/server.js
MAX_CONSECUTIVE_LOSSES=3 node tools/dev-harness/server.js
TOKEN_COOLDOWN_MINUTES=30 node tools/dev-harness/server.js
EMERGENCY_FLATTEN_POSITIONS=true node tools/dev-harness/server.js
HARNESS_TICK_MS=500 node tools/dev-harness/server.js        # faster market
```

## Walkthrough

1. `node tools/dev-harness/server.js`, open http://localhost:8080
2. The **RISK GUARD** card bottom-right (from `webapp/js/risk.js`) shows the rails.
3. Press the dashboard's **Start** button → positions open, prices move, exits fire.
4. Press **KILL SWITCH** in the risk card → entries blocked, WS shows the halt.
5. Press **Reset halt** → trading can resume.

The harness is a development tool: it never talks to Solana and never signs
anything.
