# Running TraderTony V4 24/7 for free (2026 edition)

You do not need a paid VPS. Two providers still give a genuinely free, always-on
machine with no expiry, and there are two more options if you have spare
hardware. This guide covers what actually works for **this** bot, including the
free-tier rules that will bite you if you ignore them.

---

## TL;DR

| Option | Cost | Always on? | Persistent disk | Good for this bot? |
|---|---|---|---|---|
| **Oracle Cloud Always Free** (ARM Ampere A1) | $0 forever | ✅ | ✅ 200 GB | ⭐ **Best choice.** 2 OCPU / 12 GB, 10 TB egress. Needs a real bank card at signup; ARM capacity is often "out of host capacity". |
| **Spare Android phone / old laptop / Raspberry Pi at home** | $0 (hardware you own) | ✅ | ✅ | ⭐ **Also excellent** — and the only free option on a *residential* IP, which is what the fomo.family data source needs. |
| **Northflank free ("Sandbox")** | $0 | ✅ no sleep | ⚠️ 0.5 GB volume | Works: 2 services, 1 vCPU / 1 GB, always-on compute. Card required. |
| **Koyeb free (Hobby)** | $0 | ✅ no sleep | ⚠️ 2 GB | Works: 1 service, 512 MB, no idle sleep. Tight RAM, and you must use a Docker image. |
| **Google Cloud e2-micro free tier** | $0 | ✅ | ✅ | ⚠️ **Risky here:** 1 GB RAM *and only 1 GB/month egress* (North America). This bot polls prices every 15 s per position plus scanners — see §5. |
| Render free | $0 | ❌ sleeps after 15 min | — | ❌ Dead in the water: a sleeping bot trades nothing. |
| Railway | no free tier (one-time $5 trial) | — | — | ❌ Not free anymore. |
| Fly.io | no free tier for new accounts (legacy orgs only) | ✅ | ✅ | 💰 Cheapest paid option at ~$2–3/month if you ever want "just works" (the repo already ships `fly.toml`). |
| GitHub Actions cron | $0 | ❌ 6 h job cap, delayed schedules | ❌ | ❌ Wrong tool — see §7. |

**Recommended path:** Oracle Cloud Always Free (install with `deploy/setup.sh`),
or an old phone/laptop at home if you can't get an Oracle account or a card.

---

## 1. Why the free tier changed (read this before following old guides)

- **Oracle halved its Always Free ARM allowance** on 15 June 2026 — 4 OCPU / 24 GB
  became **2 OCPU / 12 GB** (1,500 OCPU-hours + 9,000 GB-hours per month), and
  enforcement began **18 August 2026**: instances above the new limit are
  terminated. Storage (200 GB) and the two AMD micro instances are unchanged.
  Guides still quoting "4 cores / 24 GB" describe a machine you can no longer
  create. ([summary](https://terminalbytes.com/oracle-cloud-free-tier-changes-2026/), [HN thread](https://news.ycombinator.com/item?id=49183750))
- **Oracle reclaims idle instances.** Across a rolling 7-day window they look at
  95th-percentile CPU (< 20%), network (< 20%) and, on A1 shapes, memory (< 20%).
  A quiet trading bot can fall inside that definition — `deploy/setup.sh` installs
  a small keepalive service so that doesn't happen. ([free-VPS comparison](https://klymentiev.com/blog/free-vps))
- **Render's free services now sleep after 15 minutes** (was 30) with 30–50 s cold
  starts; **Railway's free tier is gone** (a one-time $5 trial credit only);
  **Koyeb** and **Northflank** are the two remaining always-on container free
  tiers. ([Render vs Railway 2026](https://agentdeals.dev/railway-vs-render), [free Docker hosts](https://flywp.com/blog/9769/best-free-docker-hosting-platforms/))

---

## 2. Recommended: Oracle Cloud Always Free

### 2.1 Create the instance

1. Sign up at <https://oracle.com/cloud/free>. **Use a real bank credit/debit
   card** — prepaid cards get rejected by their fraud checks. Nothing is charged
   while you stay inside the Always Free limits.
2. Create a compute instance:
   - **Image:** Ubuntu 22.04 or 24.04 (aarch64)
   - **Shape:** `VM.Standard.A1.Flex` — set **1 OCPU / 6 GB** (leave headroom; the
     free allowance is 2 OCPU / 12 GB total, and you may want a second instance)
   - **Boot volume:** 50–100 GB (up to 200 GB is free)
   - **Region:** pick one near you *and* one that has ARM capacity. Karachi →
     **Mumbai** or **Hyderabad** are closest; **Singapore** is a good fallback.
     Capacity in popular US regions is often exhausted.
   - Download the SSH key.
3. If you get **"Out of host capacity"**, that region has no free ARM left. Try
   another region/availability domain, retry later, or upgrade the account to
   **Pay As You Go** — PAYG keeps the larger allowance *and* only bills what you
   use, so an Always Free-sized instance still costs $0.
4. Note your instance's **public IP** from the console.

> ⚠️ **Set a budget alert** (Billing → Budgets → threshold $0.01). It catches an
> accidental paid resource before it costs you anything.

### 2.2 Open only what you need

Oracle's Ubuntu images come with iptables blocking everything except SSH. Leave
it that way for now — you reach the dashboard through an SSH tunnel (§6). If you
later want the API public, you must open **both** the VCN security list *and*
iptables, and set `API_KEY` first.

### 2.3 Install the bot

```bash
ssh -i your-key.pem ubuntu@<public-ip>

sudo apt-get update && sudo apt-get install -y git
git clone -b arena/01a0d052-meme https://github.com/<you>/meme.git
cd meme
sudo bash deploy/setup.sh
```

`setup.sh` installs Docker, creates swap, makes `deploy/data/` with the right
ownership, writes a `.env` with a **generated API key** and safe defaults
(`DEMO_MODE=true`, `DRY_RUN_MODE=true`), installs the keepalive service, then
builds and starts the container.

Then fill in your real credentials and restart:

```bash
nano .env      # SOLANA_RPC_URL, WALLET_PRIVATE_KEY, HELIUS_API_KEY, BIRDEYE_API_KEY, MORALIS_API_KEY
cd deploy && docker compose up -d --build
docker compose logs -f
```

**On ARM, the first build takes 20–40 minutes** on 1–2 cores (it compiles
Solana's SDK). If you would rather not wait, build the image once on a faster
machine — or use the `Publish Docker image` GitHub Actions workflow to push a
multi-arch image to GHCR and pull it:

```bash
docker compose pull && docker compose up -d
```

### 2.4 Make it survive reboots

`restart: unless-stopped` in `deploy/docker-compose.yml` plus Docker's own
service is enough — Docker starts on boot and restarts the container. Verify:

```bash
sudo reboot
# …wait, ssh back in:
docker compose -f ~/meme/deploy/docker-compose.yml ps     # healthy again
curl -s localhost:3030/api/health | head -c 200
```

### 2.5 Keep Oracle from reclaiming it

Already handled by `deploy/setup.sh`, which installs `trader-keepalive.service`
(≈6 % CPU, nice'd to 19). Check and tune it:

```bash
systemctl status trader-keepalive
sudo journalctl -u trader-keepalive -n 20
# different target, e.g. 10%:
sudo systemctl set-environment KEEPALIVE_MIN_CPU=10 && sudo systemctl restart trader-keepalive
```

Stop it if you stop trading — it exists to keep a *working* instance out of the
idle definition, not to park an unused VM.

---

## 3. Free and local: a spare phone, laptop or Pi

If you have a spare Android phone, an old laptop, or a Raspberry Pi, this is the
most forgiving option — and the only free one on a **residential IP**.

**Why the IP matters:** fomo.family's own API sits behind Cloudflare, which
rejects datacentre clients regardless of credentials. A home connection often
works where a cloud VM gets HTTP 430. If you want the `official` FOMO provider
(see `docs/FOMO_COPY_TRADING.md`), a home box is your best shot — otherwise use
the `fomoapi` mirror from anywhere.

### Android phone (Termux)

```bash
# In Termux (F-Droid build, NOT the Play Store one)
pkg update && pkg install -y proot-distro openssh
proot-distro install ubuntu
proot-distro login ubuntu
# inside Ubuntu:
apt update && apt install -y git docker.io || apt install -y git build-essential curl
```

Then either run Docker (needs a rooted device or `proot` Docker workarounds — not
reliable), or **build the binary directly**:

```bash
apt install -y pkg-config libssl-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone -b arena/01a0d052-meme https://github.com/<you>/meme.git && cd meme
cp .env.example .env && nano .env
cargo build --release
```

Keep it alive:

- Termux → **Acquire wakelock** (notification → "Acquire wakelock"), and disable
  battery optimisation for Termux in Android settings.
- Start it under `tmux` so an SSH drop doesn't kill it:
  `tmux new -s bot 'while true; do ./target/release/trader-tony-v4; sleep 10; done'`
- Use the systemd-free equivalent of boot persistence: Termux:Boot add-on, or a
  `~/.termux/boot/start-bot.sh` script.

### Old laptop / Raspberry Pi

Same as the Oracle path but simpler — no security lists, no capacity lottery:

```bash
git clone -b arena/01a0d052-meme https://github.com/<you>/meme.git && cd meme
sudo bash deploy/setup.sh
```

If you prefer no Docker, use the systemd unit instead:

```bash
sudo apt install -y build-essential pkg-config libssl-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo build --release
sudo useradd -m trader && sudo mkdir -p /opt/trader-tony
sudo cp -r target/release/trader-tony-v4 .env data /opt/trader-tony/
sudo chown -R trader:trader /opt/trader-tony && sudo chmod 600 /opt/trader-tony/.env
sudo cp deploy/systemd/trader-tony.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now trader-tony
journalctl -u trader-tony -f
```

> A home box has no UPS. A power cut mid-trade leaves a position open on-chain
> that the bot no longer tracks. Keep `data/` backed up (`rsync` it off the box
> nightly) and check `/api/positions` after any outage.

---

## 4. Container PaaS free tiers (no server to manage)

Both are always-on with **no sleep** on the free tier. Neither gives you a real
persistent disk worth relying on, so treat `data/` as ephemeral and export
positions if it matters.

**Northflank** (free "Sandbox": 2 services, 1 database, 2 cron jobs, 1 vCPU /
1 GB, 0.5 GB volume — card required)

1. New project → *Service* → *Build from repo* → point at your fork/branch.
2. It detects the `Dockerfile`. Set the container port to **3030**.
3. Add the env vars from `.env.example` under *Secrets*.
4. Add a persistent volume mounted at **`/app/data`**.
5. Health check path: `/api/health`.

**Koyeb** (free Hobby: 1 service, 512 MB, 0.1 vCPU, 2 GB volume, no card needed)

1. Create app → *Docker* → GitHub → repo + branch.
2. Port **3030**, health check path `/api/health`, instance type *Free*.
3. Env vars as above; add a volume at `/app/data`.

Both are tight on RAM. The bot idles around 150–300 MB, which fits 512 MB, but
keep `FOMO_MAX_POSITIONS` and `MAX_POSITION_SIZE_SOL` low, and set
`RUST_LOG=warn` if you see the container getting OOM-killed.

---

## 5. Why not Google Cloud's free e2-micro

GCP's always-free tier is real (1 e2-micro in `us-west1` / `us-central1` /
`us-east1`, 30 GB disk) — but it gives you **1 GB of North America egress per
month**, and this bot is chatty:

| Source | Frequency | Rough volume |
|---|---|---|
| Position price updates (`JupiterClient::get_price`) | every **15 s per open position** | ~1 KB × 4/min × positions |
| Scanner cycle (Moralis / Helius) | every 30–60 s | 5–200 KB per cycle |
| FOMO swap polling | every 60 s | 1–20 KB |
| dexscreener lookups (market caps, SOL price) | per token event | 10–50 KB each |

With three open positions you are looking at roughly **0.5–1.5 GB/month** of
egress before the scanners are counted — genuinely at or over the cap, and going
over is billed. Oracle gives 10 TB. If you do use GCP, disable the Moralis
scanner strategies (stick to `telegram_call` or `fomo_copy`), watch
`VPC → Network → Egress`, and set a budget alert.

---

## 6. Reaching the dashboard safely

The dashboard is a **separate static site** (`webapp/`) — the bot's Rust binary does
not serve it (that's why `.dockerignore` drops `webapp/`), and its API base URL is
baked into `webapp/js/config.js`. Host it free on Vercel / Netlify / Cloudflare
Pages / GitHub Pages:

```bash
# point the dashboard at your box before deploying
nano webapp/js/config.js      # window.API_BASE_URL = 'http://localhost:3030'
# then deploy the webapp/ folder, e.g. with Vercel (webapp/vercel.json already exists)
npx vercel deploy --prod webapp
```

Mixed content: Chrome and Firefox treat `http://localhost` as a *trustworthy*
origin, so an HTTPS-hosted dashboard can still call `http://localhost:3030`
through the tunnel below — as long as you are browsing from the machine that
holds the tunnel.

If you would rather have a real HTTPS API (so the dashboard works from any
device), three options, all $0:

| Option | URL | $ | Notes |
|---|---|---|---|
| `cloudflared tunnel --url http://localhost:3030` | random `*.trycloudflare.com` | 0 | No account needed. The URL **changes on every restart** and Cloudflare calls quick tunnels a debug aid — no uptime guarantee. |
| **Tailscale Funnel** | stable `https://<machine>.<tailnet>.ts.net` | 0 | `tailscale funnel 3030`; Personal plan is free (6 users, unlimited devices) and Serve/Funnel are not metered. No domain needed. |
| Named Cloudflare Tunnel | your own `api.example.com` | 0 for the tunnel | Needs a **domain** added to Cloudflare — the domain is the only cost here (~$8–12/year at Cloudflare Registrar's at-cost pricing). Best if you want Cloudflare Access in front of the API. |

Whichever you pick, set `CORS_ORIGINS` to your dashboard's exact origin instead
of `*`, and keep `API_KEY` set — a public HTTPS URL is still a public URL.

> If you edit `webapp/js/config.js` you are also free to just open the dashboard
> file locally (`python3 -m http.server` in `webapp/`), which keeps everything on
> your machine.

The API can start/stop trading and read your wallet, so **never leave port 3030
open without `API_KEY`**. Options, best first:

1. **SSH tunnel** (nothing exposed at all):
   ```bash
   ssh -i key.pem -L 3030:localhost:3030 ubuntu@<public-ip>
   # then point the dashboard at http://localhost:3030
   ```
2. **API key + a tunnel/proxy.** Set `API_KEY=<32-byte hex>` in `.env`, put the
   same value in the dashboard's *API key* field — the *Risk* card has an
   **API key** button, and it is stored in `localStorage` as `tony_api_key` — then
   expose the port via Cloudflare Tunnel (free) rather than opening the firewall. Then:
   ```yaml
   ports:
     - "3030:3030"        # only after API_KEY is set
   ```
3. Keep `CORS_ORIGINS` locked to your dashboard's origin rather than `*`.

The bot's own `webapp/js/risk.js` card has an **API key** button — paste it there
once and the REST + WebSocket clients attach it automatically.

### Free uptime monitoring

Use any free HTTP monitor (UptimeRobot, Better Stack, healthchecks.io) against
`https://<your-host>/api/health` — it is public by design, so it never needs the
key. An alert on that endpoint tells you the bot died before you notice missing
trades. If you have not exposed the port, use healthchecks.io instead: the bot
does not call it, so instead have a cron job `curl` the local health endpoint and
ping healthchecks.io (`*/5 * * * * curl -fsS localhost:3030/api/health && curl -fsS https://hc-ping.com/<uuid>`).

---

## 7. Why not GitHub Actions (or any cron-based "free" hosting)

It looks tempting — free minutes, a Dockerfile already in the repo — but it is
the wrong shape for a trading bot:

- **Jobs are capped at 6 hours**, so a continuous bot restarts constantly and
  loses in-memory state. The bot persists to `data/`, but on Actions that
  filesystem is destroyed with the runner.
- **Scheduled workflows are best-effort**: `cron` is not a real scheduler, and
  GitHub explicitly warns runs may be delayed by minutes or skipped during load.
  A sniper that reacts to Telegram calls cannot tolerate that.
- **Scheduled workflows are disabled after 60 days of repository inactivity.**
- **State does not survive**: every run starts from a fresh checkout unless you
  commit state back to the repo — which means putting your `positions.json` (and
  your open-trade bookkeeping) in git.
- **Acceptable-use rules:** Actions must be used for the project's own
  build/test/deploy work, not as a general-purpose compute service.
- **Datacentre IPs** are exactly what Cloudflare blocks for fomo.family.

Use Actions for what it is good at — the `Verify` and `Publish Docker image`
workflows in this repo — and rent/find a real box to run the bot.

---

## 8. Go-live checklist

```bash
# 1. Health and auth
curl -s localhost:3030/api/health                                    # {"status":"ok",...}
curl -s -o /dev/null -w '%{http_code}\n' localhost:3030/api/risk/status   # 401 → auth works

# 2. Risk rails configured (see docs/ARENA_ROADMAP.md §6)
curl -s localhost:3030/api/risk/status -H "X-API-Key: $API_KEY" | head -40

# 3. FOMO copier target resolved (if you use that strategy)
curl -s localhost:3030/api/fomo/status -H "X-API-Key: $API_KEY" | head -40

# 4. State persists
ls -la deploy/data/          # positions.json, strategies.json, fomo_state.json…
docker compose restart && sleep 15 && curl -s localhost:3030/api/positions -H "X-API-Key: $API_KEY"

# 5. Survives a reboot
sudo reboot   # then re-check 1–4
```

Before switching off `DEMO_MODE`:

- [ ] Ran at least a few days in `DEMO_MODE=true` and understood the behaviour
- [ ] `DRY_RUN_MODE=true` tested against real tokens
- [ ] `API_KEY` set, port not exposed publicly (or only via a tunnel)
- [ ] `DAILY_LOSS_LIMIT_SOL` / `MAX_DRAWDOWN_PERCENT` / `MAX_CONSECUTIVE_LOSSES` set
- [ ] Wallet is a **burner** funded with only what you can lose
- [ ] `data/` backed up somewhere off the box
- [ ] You know how to stop it: `POST /api/emergency/stop?flatten=true`
- [ ] Dashboard deployed with `webapp/js/config.js` pointing at your API, and the
      API key pasted once into its *Risk* card
- [ ] An uptime monitor on `/api/health` (see §6) so a dead bot texts you

---

## 9. What this actually costs

Everything in this guide is $0 **except** the last two rows — those are the price
of trading at all, not of hosting.

| Piece | Cost | Notes |
|---|---|---|
| Bot compute (Oracle Always Free ARM A1) | **$0** forever | 2 OCPU / 12 GB, 200 GB disk, 10 TB egress. Card needed at signup, nothing charged inside the limits. |
| Bot compute (spare phone / laptop / Pi) | **$0** | You already own it; residential IP is a bonus. Electricity is the only "cost". |
| Docker, Compose, systemd, this guide's scripts | **$0** | Open source. |
| Static dashboard hosting | **$0** | **Cloudflare Pages** is the best free tier: unlimited bandwidth *and* unlimited requests, 500 builds/month, free SSL, free custom domain, no credit card. Netlify and Vercel free tiers cap at 100 GB/month; Vercel's Hobby plan also **forbids commercial use**, so prefer Cloudflare Pages for a trading dashboard. |
| SSH tunnel (`ssh -L 3030:localhost:3030`) | **$0** | Just the SSH client you already have. Nothing is exposed to the internet. |
| Cloudflare quick tunnel | **$0** | Random `*.trycloudflare.com` URL, no account. URL changes on restart; explicitly a debug aid, not production. |
| Tailscale Funnel | **$0** | Stable `*.ts.net` HTTPS URL on the free Personal plan, no domain purchase. |
| Named Cloudflare Tunnel | **$0** for the software | Needs a domain you own — see the next row. |
| A domain name (optional) | ~**$8–12/year** | Only if you want `api.yourname.com` instead of a `*.ts.net` / `*.trycloudflare.com` URL. Cloudflare Registrar sells at cost; a `.xyz`/`.dev` is cheapest. **This is the only optional spend in the whole setup.** |
| Uptime monitoring | **$0** | UptimeRobot / healthchecks.io free plans are plenty for one bot. |
| RPC + data APIs (Helius, Birdeye, Moralis) | **$0** to start | All have free tiers (rate-limited). If you run many positions, a Helius paid plan (~$50/mo) removes the rate limits — optional. |
| FOMO mirror key (`FOMO_PROVIDER=fomoapi`) | depends on the provider | Check the mirror's pricing; the `custom` provider costs whatever your proxy costs. |
| **Solana network + priority fees, Jito tips** | **not free, not avoidable** | Every entry/exit costs ~0.000005 SOL base + priority fee (and a Jito tip if you enable it). Budget for it — this is why the risk guard's `DAILY_LOSS_LIMIT_SOL` matters. |
| **The SOL you trade with** | **not free** | Obviously. Use a burner wallet funded with money you can afford to lose. |

**Bottom line:** hosting a 24/7 bot with a live dashboard costs **$0/month**, and
you can stay at $0 forever by skipping the domain and using `*.trycloudflare.com`
or `*.ts.net`. The only money that leaves your wallet is what you choose to trade
(plus per-transaction Solana fees).
