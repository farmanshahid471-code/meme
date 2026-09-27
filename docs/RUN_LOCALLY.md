# Testing TraderTony V4 on your own machine (no VPS, no Oracle yet)

Four levels, from "zero install" to "close to production". Do them in order —
each one catches a different class of problem, and level 3 exercises everything
that is risky about going live.

| Level | What you need | How long | What it proves |
|---|---|---|---|
| **0. UI only** (`tools/dev-harness`) | Node 18+ | 30 s | The dashboard, FOMO panel and risk card render and behave against a mock backend |
| **1. Unit tests** | Rust toolchain *or* CI | 5–10 min | The trading logic itself (85 tests: sizing, parsing, risk rails, exits) |
| **2. Full bot, demo mode** | Rust toolchain *or* the prebuilt binary | 10–40 min first build | Real Solana data, real strategies, simulated money, real API + WebSocket |
| **3. Dry run + drills** | as level 2, plus your own keys | 1–2 h | Auth, kill switch, daily-loss rail, FOMO copy target — everything except signing |

**Get the code**

```bash
git clone -b arena/01a0d052-meme https://github.com/farmanshahid471-code/meme.git
cd meme
```

<https://github.com/farmanshahid471-code/meme/archive/refs/heads/arena/01a0d052-meme.zip>

**Or skip the Rust build entirely** — every tagged release has prebuilt bundles
(binary + dashboard + tools + `.env.example`), including **linux-aarch64** for
the Oracle ARM box:

<https://github.com/farmanshahid471-code/meme/releases>

```bash
tar -xzf trader-tony-v4-linux-x86_64.tar.gz && cd trader-tony-v4-linux-x86_64
./trader-tony-v4
```

---

## Level 0 — UI preview, no build, no keys (do this first)

The dev harness serves the *real* `webapp/` dashboard against a mock backend
that speaks the same API as the bot. It never touches Solana and cannot sign
anything.

```bash
node tools/dev-harness/server.js        # Node 18+ only — no npm install
# → http://localhost:8080
```

What to check:

1. **Risk Guard card** (bottom right) → rails render; the API-key button is there.
2. Press **Start** → positions open, prices walk, SL/TP/trailing close them.
3. Press **KILL SWITCH** → entries blocked, the WebSocket feed shows the halt.
4. Press **Reset halt** → trading resumes.
5. **FOMO COPY** card → clan/trader/swap data, provider + mode.

Drill the rails with a faster, meaner market:

```bash
HARNESS_TICK_MS=500 DAILY_LOSS_LIMIT_SOL=0.05 MAX_TRADES_PER_DAY=3 \
  node tools/dev-harness/server.js
```

```bash
# then watch the daily-loss rail trip, in another terminal:
curl -s localhost:8080/api/risk/status | head -40
```

Exercise authentication too:

```bash
HARNESS_API_KEY=dev-key node tools/dev-harness/server.js
curl -s -o /dev/null -w '%{http_code}\n' localhost:8080/api/risk/status   # 401
```

**Windows:** install Node from <https://nodejs.org> and run the same command in
PowerShell (`node tools\dev-harness\server.js`) — no WSL needed.

---

## Level 1 — the unit tests

If CI is green you can trust this level without installing anything: open
[Actions → Verify](https://github.com/farmanshahid471-code/meme/actions/workflows/arena-verify.yml).

Locally:

```bash
cargo test --locked          # ~85 tests, no network, no keys
cargo test --locked -- --nocapture fomo_copy    # just the copy-trading logic
```

<details>
<summary>Installing Rust (one-time, ~5 min, ~1.5 GB)</summary>

```bash
# Linux / macOS / WSL
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# then build deps:
#   Ubuntu/Debian: sudo apt install -y build-essential pkg-config libssl-dev
#   macOS:         xcode-select --install
```
On Windows, build inside **WSL2** (`wsl --install`) or just use Docker (level 2).
</details>

---

## Level 2 — the real bot in demo mode

Demo mode uses **real market data and real strategy logic** with simulated money:
no private key is ever used to sign, and no transaction leaves the machine.

### 2a. Configure

```bash
cp .env.example .env
```

The minimum for a safe local run:

```ini
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com   # or a free Helius URL (better)
HELIUS_API_KEY=demo                                   # non-empty; free key = real data
WALLET_PRIVATE_KEY=11111111111111111111111111111111   # dummy 32-byte key — demo mode never signs
DEMO_MODE=true
DRY_RUN_MODE=true
AUTO_START_TRADING=false
```

> The dummy key is a valid *shape* so the wallet loader is happy, but it is not a
> usable account. **Never** fund it, and never switch `DEMO_MODE` off while it is
> set. When you go live, generate a real burner (`solana-keygen new`) and move
> the key out of the repo directory.

### 2b. Run it

**Prebuilt binary** (fastest):

```bash
./trader-tony-v4
```

**Build from source:**

```bash
cargo run --release           # first build is 10–40 min, then instant
```

**Docker** (same thing you will run on Oracle):

```bash
cd deploy && docker compose up -d --build && docker compose logs -f
```

### 2c. Poke at it

```bash
curl -s localhost:3030/api/health | jq
curl -s localhost:3030/api/wallet | jq .demo_mode
curl -s localhost:3030/api/risk/status | jq '{halted, equity_sol, trades_today}'
curl -s localhost:3030/api/status | jq
```

Dashboard: `cd webapp && python3 -m http.server 5173` → <http://localhost:5173>
(no `jq`? drop the `| jq` parts.)

`webapp/js/config.js` auto-detects a local page and points at
`http://localhost:3030`, so nothing to edit. If you serve the dashboard from a
different machine, edit that file.

### 2d. Things to watch in the logs

| Line | Means |
|---|---|
| `Demo mode: true` / `Dry run mode: true` | the safety net is on |
| `Solana RPC connection verified` | RPC + key are good |
| `Configuration loaded successfully (v4.1.0 - multi-strategy)` | config parsed |
| `AutoTrader initialized` | strategies registered, state files loaded |

---

## Level 3 — dry run against live data, with the safety rails drilled

This is the level that decides whether you go near real money.

```ini
DEMO_MODE=false
DRY_RUN_MODE=true            # real scan + real risk analysis, simulated fills
AUTO_START_TRADING=true
DAILY_LOSS_LIMIT_SOL=0.05
MAX_DRAWDOWN_PERCENT=10
MAX_TRADES_PER_DAY=3
MAX_CONSECUTIVE_LOSSES=3
TOKEN_COOLDOWN_MINUTES=30
API_KEY=dev-key-please-change
```

### Drill list (tick them off)

| # | Drill | How | Expected |
|---|---|---|---|
| 1 | Health | `curl -s localhost:3030/api/health` | `status: ok` |
| 2 | Auth on | `curl -s -o /dev/null -w '%{http_code}' localhost:3030/api/risk/status` | `401` |
| 3 | Auth key | same with `-H 'X-API-Key: dev-key-please-change'` | `200` |
| 4 | Kill switch | `curl -X POST localhost:3030/api/emergency/stop -H 'X-API-Key: …'` | entries blocked; `halted:true` |
| 5 | Halt persists | restart the bot, check `/api/risk/status` | still `halted:true` (state file) |
| 6 | Resume | `curl -X POST localhost:3030/api/risk/reset …` | `halted:false` |
| 7 | Daily-loss rail | set `DAILY_LOSS_LIMIT_SOL` tiny, let a loser close | `halt_kind: daily_loss` |
| 8 | Max trades/day | `MAX_TRADES_PER_DAY=1`, wait for 2nd entry attempt | entry refused, reason logged |
| 9 | Cooldown | after an exit, same token again | refused for `TOKEN_COOLDOWN_MINUTES` |
| 10 | WebSocket | `websocat ws://localhost:3030/ws` (or the dashboard console) | `status_update` / `price_update` frames |
| 11 | Persistence | `ls data/`, restart, `curl /api/positions` | positions, strategies, risk state survive |
| 12 | FOMO discovery | `curl -s .../api/fomo/status -H 'X-API-Key: …'` | clan → top member resolved |
| 13 | FOMO refresh | `curl -X POST .../api/fomo/refresh …` | new swap list fetched, skips explained |
| 14 | FOMO pin | `curl -X POST .../api/fomo/target -d '{"trader":"PoorGoat_"}' …` | target pinned, overrides auto-discovery |
| 15 | Emergency flatten | `curl -X POST '.../api/emergency/stop?flatten=true'` | open positions closed (simulated) |

### Before you ask the bot to sign anything

1. Run `node tools/fomo-probe.js` **on the machine that will host the bot** — it
   tells you whether that network can reach fomo.family at all (datacentre IPs
   usually can't; see `docs/FOMO_COPY_TRADING.md` §1).
2. Confirm `ls data/` shows the state files you expect, and back that folder up.
3. Confirm `DEMO_MODE=false` + `DRY_RUN_MODE=true` still produces **zero**
   on-chain transactions (`solana address -k …` / your wallet's explorer page).
4. Only then: `DRY_RUN_MODE=false` with a burner wallet holding a small amount.

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| `SOLANA_RPC_URL not set` / `HELIUS_API_KEY not set` | `.env` missing or not in the working directory — the bot reads `./.env` |
| `Invalid private key format` | `WALLET_PRIVATE_KEY` must be base58 (see level 2a) |
| `Address already in use` | something else owns 3030: `API_PORT=3031` (and point `config.js` at it) |
| Dashboard shows dashes / "demo mode" | the bot isn't running, or the dashboard points at the hosted backend — check the browser console for `[Config] Local page detected` |
| FOMO panel empty | `/api/fomo/status` needs `FOMO_PROVIDER` + key; press refresh and read `last_error` |
| FOMO provider returns 430 | Cloudflare blocks that network — use `fomoapi` or `custom` |
| Rate limited by RPC | free public RPC is throttled: use a free Helius key |
| Rust build OOM on a small box | add swap (see `deploy/setup.sh`) or use the prebuilt binary |
| macOS: "cannot be opened because the developer cannot be verified" | `xattr -dr com.apple.quarantine trader-tony-v4` |

---

## What "done testing locally" looks like

- [ ] Level 0: dashboard, FOMO panel and risk card all behave
- [ ] Level 1: `cargo test` green (or CI green on the same commit)
- [ ] Level 2: bot runs in demo mode, dashboard live, state files created
- [ ] Level 3: drills 1–15 pass, including kill switch + daily-loss rail
- [ ] FOMO probe run on the machine that will host it
- [ ] `data/` backed up, burner wallet ready, `docs/DEPLOY_FREE.md` read

Then deployment is just the same compose file on a free Oracle instance
(`deploy/setup.sh`).
