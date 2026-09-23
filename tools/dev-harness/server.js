#!/usr/bin/env node
/**
 * TraderTony V4 — local dev harness
 * =================================
 *
 * Runs the real `webapp/` dashboard against a *mock* bot backend, so UI and API
 * changes can be reviewed in a browser without building the Rust bot, without a
 * wallet, and without touching Solana.
 *
 *   node tools/dev-harness/server.js            # http://localhost:8080
 *   PORT=9000 node tools/dev-harness/server.js
 *
 * What it fakes
 * -------------
 * - every endpoint the dashboard calls, with the exact response shapes from
 *   `src/web/models.rs`
 * - a small market simulator: prices drift, stop-loss / take-profit fire, closed
 *   trades land in `/api/trades`
 * - the Track A risk guard (daily loss cap, drawdown breaker, losing streak,
 *   per-day entry cap, per-token cooldown) with the same semantics as
 *   `src/trading/risk_guard.rs`
 * - the kill switch, including `?flatten=true`
 *
 * No dependencies, no build step. Set HARNESS_API_KEY to simulate the backend's
 * API_KEY auth (401 on everything except /api/health).
 */

'use strict';

const http = require('http');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { URL } = require('url');

const PORT = Number(process.env.PORT || 8080);
const HOST = process.env.HOST || '0.0.0.0';
const API_KEY = process.env.HARNESS_API_KEY || '';
const WEBAPP_DIR = path.resolve(__dirname, '..', '..', 'webapp');
const TICK_MS = Number(process.env.HARNESS_TICK_MS || 2000);

// ---------------------------------------------------------------------------
// Configuration mirroring the bot's env vars
// ---------------------------------------------------------------------------
const config = {
  demo_mode: true,
  dry_run_mode: false,
  api_key: API_KEY,
  emergency_flatten_positions: process.env.EMERGENCY_FLATTEN_POSITIONS === 'true',
  total_budget_sol: Number(process.env.TOTAL_BUDGET_SOL || 1.0),
  rails: {
    daily_loss_limit_sol: num(process.env.DAILY_LOSS_LIMIT_SOL),
    max_drawdown_percent: num(process.env.MAX_DRAWDOWN_PERCENT),
    max_trades_per_day: int(process.env.MAX_TRADES_PER_DAY),
    max_consecutive_losses: int(process.env.MAX_CONSECUTIVE_LOSSES),
    token_cooldown_minutes: int(process.env.TOKEN_COOLDOWN_MINUTES) || 0,
  },
};

function num(v) {
  const n = Number(v);
  return Number.isFinite(n) && n > 0 ? n : null;
}
function int(v) {
  const n = parseInt(v, 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

// ---------------------------------------------------------------------------
// Mock market + risk guard state
// ---------------------------------------------------------------------------
const state = {
  running: false,
  wallet: {
    address: 'DevHarnessWa11et111111111111111111111111111111',
    balance_sol: 4.8123,
  },
  positions: [],
  trades: [],
  active_strategy: 'migrated',
  risk: {
    day: new Date().toISOString().slice(0, 10),
    realized_pnl_today_sol: 0,
    realized_pnl_total_sol: 0,
    trades_today: 0,
    trades_total: 0,
    wins: 0,
    losses: 0,
    consecutive_losses: 0,
    peak_equity_sol: config.total_budget_sol,
    equity_sol: config.total_budget_sol,
    halt: null,
    last_exit_by_token: {},
    entry_refusals: 0,
  },
  log: [],
};

const TOKENS = [
  { symbol: 'RETARD', name: 'Retarded Cat', price: 0.00000412 },
  { symbol: 'BIBI', name: 'Bibi', price: 0.0000912 },
  { symbol: 'GAMBO', name: 'Gamboled', price: 0.00000187 },
];

function mintFor(symbol) {
  const base = crypto.createHash('sha256').update(symbol).digest('base64url');
  return base.slice(0, 39) + 'pump';
}

function openPosition(spec, sizeSol) {
  const now = new Date().toISOString();
  const tokenAmount = sizeSol / spec.price;
  const position = {
    id: crypto.randomUUID(),
    token_address: mintFor(spec.symbol),
    token_name: spec.name,
    token_symbol: spec.symbol,
    strategy_id: 'strategy-migrated-default',
    entry_value_sol: sizeSol,
    token_amount: tokenAmount,
    entry_price: spec.price,
    current_price: spec.price,
    peak_price: spec.price,
    stop_loss_price: spec.price * 0.85,
    take_profit_price: spec.price * 1.5,
    trailing_stop_percent: 15,
    status: 'active',
    opened_at: now,
    closed_at: null,
    exit_reason: null,
  };
  state.positions.push(position);
  recordEntry();
  log(`BUY ${spec.symbol} for ${sizeSol.toFixed(3)} SOL @ ${spec.price}`);
  broadcast({
    type: 'position_opened',
    data: { id: position.id, token_symbol: spec.symbol, entry_value_sol: sizeSol },
  });
  return position;
}

function closePosition(position, reason) {
  position.status = 'closed';
  position.closed_at = new Date().toISOString();
  position.exit_reason = reason;
  const pnlSol = position.token_amount * position.current_price - position.entry_value_sol;
  const pnlPct = position.entry_value_sol > 0 ? (pnlSol / position.entry_value_sol) * 100 : 0;

  state.wallet.balance_sol += position.entry_value_sol + pnlSol;
  state.trades.unshift({
    id: crypto.randomUUID(),
    token_address: position.token_address,
    token_symbol: position.token_symbol,
    action: 'sell',
    amount_sol: position.token_amount * position.current_price,
    token_amount: position.token_amount,
    price: position.current_price,
    pnl_sol: pnlSol,
    pnl_percent: pnlPct,
    transaction_signature: crypto.randomBytes(32).toString('base64url').slice(0, 64),
    timestamp: new Date().toISOString(),
  });
  recordExit(position.token_address, pnlSol);
  log(`SELL ${position.token_symbol} (${reason}) PnL ${pnlSol >= 0 ? '+' : ''}${pnlSol.toFixed(4)} SOL`);
  broadcast({
    type: 'position_closed',
    data: { id: position.id, token_symbol: position.token_symbol, pnl_sol: pnlSol, pnl_percent: pnlPct, exit_reason: reason },
  });
}

// --- risk guard (mirror of src/trading/risk_guard.rs) ----------------------
function rollDay() {
  const today = new Date().toISOString().slice(0, 10);
  if (state.risk.day === today) return;
  state.risk.day = today;
  state.risk.realized_pnl_today_sol = 0;
  state.risk.trades_today = 0;
  if (state.risk.halt && ['daily_loss', 'consecutive_losses', 'max_trades_per_day'].includes(state.risk.halt.kind)) {
    state.risk.halt = null;
  }
}

function halt(kind, reason) {
  if (state.risk.halt) return state.risk.halt;
  state.risk.halt = { kind, reason, since: new Date().toISOString() };
  log(`🛑 HALT (${kind}): ${reason}`);
  broadcast({ type: 'alert', data: { severity: 'critical', message: `Trading halted: ${reason}` } });
  return state.risk.halt;
}

function checkEntry(tokenAddress) {
  rollDay();
  if (state.risk.halt) return `trading halted by ${state.risk.halt.kind} rail: ${state.risk.halt.reason}`;

  if (config.rails.daily_loss_limit_sol && state.risk.realized_pnl_today_sol <= -config.rails.daily_loss_limit_sol) {
    return `daily loss limit reached (${state.risk.realized_pnl_today_sol.toFixed(4)} SOL)`;
  }
  if (config.rails.max_trades_per_day && state.risk.trades_today >= config.rails.max_trades_per_day) {
    return `daily trade cap reached (${state.risk.trades_today}/${config.rails.max_trades_per_day})`;
  }
  if (config.rails.max_consecutive_losses && state.risk.consecutive_losses >= config.rails.max_consecutive_losses) {
    return `${state.risk.consecutive_losses} consecutive losing trades`;
  }
  if (config.rails.token_cooldown_minutes > 0) {
    const last = state.risk.last_exit_by_token[tokenAddress];
    if (last) {
      const elapsedMs = Date.now() - new Date(last).getTime();
      const cooldownMs = config.rails.token_cooldown_minutes * 60_000;
      if (elapsedMs < cooldownMs) {
        return `token ${tokenAddress} is in post-exit cooldown for another ${Math.round((cooldownMs - elapsedMs) / 1000)}s`;
      }
    }
  }
  return null;
}

function recordEntry() {
  rollDay();
  state.risk.trades_today += 1;
  state.risk.trades_total += 1;
}

function recordExit(tokenAddress, pnlSol) {
  rollDay();
  state.risk.realized_pnl_today_sol += pnlSol;
  state.risk.realized_pnl_total_sol += pnlSol;
  if (pnlSol >= 0) {
    state.risk.wins += 1;
    state.risk.consecutive_losses = 0;
  } else {
    state.risk.losses += 1;
    state.risk.consecutive_losses += 1;
  }
  state.risk.last_exit_by_token[tokenAddress] = new Date().toISOString();
  state.risk.equity_sol = config.total_budget_sol + state.risk.realized_pnl_total_sol;
  state.risk.peak_equity_sol = Math.max(state.risk.peak_equity_sol, state.risk.equity_sol);

  if (state.risk.halt && state.risk.halt.kind === 'drawdown' && config.rails.max_drawdown_percent) {
    const floor = state.risk.peak_equity_sol * (1 - config.rails.max_drawdown_percent / 100);
    if (state.risk.equity_sol > floor) state.risk.halt = null;
  }
  if (state.risk.halt) return;

  if (config.rails.daily_loss_limit_sol && state.risk.realized_pnl_today_sol <= -config.rails.daily_loss_limit_sol) {
    return halt('daily_loss', `realised PnL today ${state.risk.realized_pnl_today_sol.toFixed(4)} SOL ≤ -${config.rails.daily_loss_limit_sol}`);
  }
  if (config.rails.max_drawdown_percent) {
    const floor = state.risk.peak_equity_sol * (1 - config.rails.max_drawdown_percent / 100);
    if (state.risk.equity_sol <= floor) {
      return halt('drawdown', `equity ${state.risk.equity_sol.toFixed(4)} SOL is below the ${config.rails.max_drawdown_percent}% drawdown floor`);
    }
  }
  if (config.rails.max_consecutive_losses && state.risk.consecutive_losses >= config.rails.max_consecutive_losses) {
    return halt('consecutive_losses', `${state.risk.consecutive_losses} losing trades in a row`);
  }
}

function log(message) {
  const line = `${new Date().toISOString()}  ${message}`;
  state.log.unshift(line);
  state.log = state.log.slice(0, 200);
  console.log(`[harness] ${message}`);
}

// ---------------------------------------------------------------------------
// Market simulation tick
// ---------------------------------------------------------------------------
function tick() {
  if (!state.running) return;

  for (const position of state.positions.filter((p) => p.status === 'active')) {
    // Random walk with a fat right tail, like a memecoin.
    const drift = (Math.random() - 0.48) * 0.09;
    position.current_price = Math.max(position.current_price * (1 + drift), 1e-12);
    position.peak_price = Math.max(position.peak_price, position.current_price);

    const trailingStop = position.peak_price * (1 - position.trailing_stop_percent / 100);
    if (position.current_price <= position.stop_loss_price) {
      closePosition(position, 'stop_loss');
    } else if (position.current_price >= position.take_profit_price) {
      closePosition(position, 'take_profit');
    } else if (position.current_price <= trailingStop) {
      closePosition(position, 'trailing_stop');
    }
  }

  broadcast({ type: 'price_update', data: { positions: activePositions().length } });

  // Occasionally open a new position when the rails allow it.
  if (state.positions.filter((p) => p.status === 'active').length < 3 && Math.random() < 0.25) {
    const spec = TOKENS[Math.floor(Math.random() * TOKENS.length)];
    const refusal = checkEntry(mintFor(spec.symbol));
    if (refusal) {
      state.risk.entry_refusals += 1;
      log(`entry refused for ${spec.symbol}: ${refusal}`);
    } else {
      openPosition(spec, 0.05 + Math.random() * 0.1);
    }
  }

  pushSnapshot();
}

// ---------------------------------------------------------------------------
// Response builders (shapes match src/web/models.rs)
// ---------------------------------------------------------------------------
function activePositions() {
  return state.positions.filter((p) => p.status === 'active');
}

function positionResponse(p) {
  const currentValue = p.token_amount * p.current_price;
  const pnlSol = currentValue - p.entry_value_sol;
  return {
    id: p.id,
    token_address: p.token_address,
    token_name: p.token_name,
    token_symbol: p.token_symbol,
    strategy_id: p.strategy_id,
    entry_value_sol: p.entry_value_sol,
    current_value_sol: p.status === 'active' ? currentValue : p.token_amount * p.current_price,
    token_amount: p.token_amount,
    entry_price: p.entry_price,
    current_price: p.current_price,
    pnl_percent: p.entry_value_sol > 0 ? (pnlSol / p.entry_value_sol) * 100 : 0,
    pnl_sol: pnlSol,
    status: p.status,
    opened_at: p.opened_at,
    closed_at: p.closed_at,
    exit_reason: p.exit_reason,
  };
}

function statsResponse() {
  const sells = state.trades.filter((t) => t.action === 'sell');
  const wins = sells.filter((t) => (t.pnl_sol || 0) > 0);
  const losses = sells.filter((t) => (t.pnl_sol || 0) <= 0);
  const totalPnl = sells.reduce((acc, t) => acc + (t.pnl_sol || 0), 0);
  const roi = sells.length ? sells.reduce((acc, t) => acc + (t.pnl_percent || 0), 0) / sells.length : 0;
  return {
    total_trades: sells.length,
    winning_trades: wins.length,
    losing_trades: losses.length,
    win_rate: sells.length ? (wins.length / sells.length) * 100 : 0,
    total_pnl_sol: totalPnl,
    avg_roi_percent: roi,
    total_volume_sol: state.trades.reduce((acc, t) => acc + t.amount_sol, 0),
    best_trade_pnl: sells.length ? Math.max(...sells.map((t) => t.pnl_sol || 0)) : 0,
    worst_trade_pnl: sells.length ? Math.min(...sells.map((t) => t.pnl_sol || 0)) : 0,
  };
}

function riskStatusResponse() {
  rollDay();
  const cooldownMinutes = config.rails.token_cooldown_minutes;
  const tokensInCooldown = Object.entries(state.risk.last_exit_by_token)
    .map(([token, last]) => ({
      token_address: token,
      remaining_seconds: Math.round(cooldownMinutes * 60 - (Date.now() - new Date(last).getTime()) / 1000),
    }))
    .filter((entry) => entry.remaining_seconds > 0)
    .sort((a, b) => b.remaining_seconds - a.remaining_seconds);

  return {
    halted: Boolean(state.risk.halt),
    halt_kind: state.risk.halt ? state.risk.halt.kind : null,
    halt_reason: state.risk.halt ? state.risk.halt.reason : null,
    halted_since: state.risk.halt ? state.risk.halt.since : null,
    day: state.risk.day,
    realized_pnl_today_sol: state.risk.realized_pnl_today_sol,
    realized_pnl_total_sol: state.risk.realized_pnl_total_sol,
    equity_sol: state.risk.equity_sol,
    peak_equity_sol: state.risk.peak_equity_sol,
    trades_today: state.risk.trades_today,
    trades_total: state.risk.trades_total,
    wins: state.risk.wins,
    losses: state.risk.losses,
    consecutive_losses: state.risk.consecutive_losses,
    entry_refusals: state.risk.entry_refusals,
    limits: {
      daily_loss_limit_sol: config.rails.daily_loss_limit_sol,
      max_drawdown_percent: config.rails.max_drawdown_percent,
      max_trades_per_day: config.rails.max_trades_per_day,
      max_consecutive_losses: config.rails.max_consecutive_losses,
      token_cooldown_minutes: config.rails.token_cooldown_minutes,
      starting_equity_sol: config.total_budget_sol,
    },
    tokens_in_cooldown: tokensInCooldown,
  };
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------
function route(req, res, url) {
  const send = (status, body) => {
    const payload = JSON.stringify(body, null, 2);
    res.writeHead(status, {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(payload),
    });
    res.end(payload);
  };

  const endpoint = url.pathname;

  // --- auth, exactly like src/web/auth.rs ---------------------------------
  if (API_KEY && endpoint !== '/api/health' && req.method !== 'OPTIONS') {
    const headerKey = req.headers['x-api-key'];
    const bearer = (req.headers['authorization'] || '').replace(/^Bearer\s+/i, '');
    const queryKey = url.searchParams.get('api_key');
    const presented = headerKey || bearer || queryKey || '';
    if (presented !== API_KEY) {
      return send(401, {
        error:
          presented
            ? 'Unauthorized: invalid API key'
            : 'Unauthorized: supply your API key via the X-API-Key header, an Authorization: Bearer token, or ?api_key= for the WebSocket',
      });
    }
  }

  switch (endpoint) {
    case '/api/health':
      return send(200, { status: 'ok', version: '0.1.0-harness', timestamp: new Date().toISOString() });

    case '/api/wallet':
      return send(200, { address: state.wallet.address, balance_sol: state.wallet.balance_sol });

    case '/api/stats':
      return send(200, statsResponse());

    case '/api/positions':
      return send(200, { positions: state.positions.map(positionResponse), total: state.positions.length });

    case '/api/positions/active':
      return send(200, { positions: activePositions().map(positionResponse), total: activePositions().length });

    case '/api/trades': {
      const limit = Number(url.searchParams.get('limit') || 50);
      return send(200, { trades: state.trades.slice(0, limit), total: state.trades.length, page: 1, limit });
    }

    case '/api/autotrader/status':
      return send(200, {
        running: state.running,
        demo_mode: config.demo_mode,
        dry_run_mode: config.dry_run_mode,
        active_strategies: 1,
        active_positions: activePositions().length,
      });

    case '/api/autotrader/start': {
      if (state.risk.halt) {
        return send(400, { error: 'Failed to start AutoTrader', details: `risk guard halt active: ${state.risk.halt.reason}` });
      }
      state.running = true;
      log('AutoTrader started');
      broadcast({ type: 'status_change', data: { running: true } });
      if (!state.positions.length) openPosition(TOKENS[0], 0.08);
      return send(200, { success: true, message: 'AutoTrader started' });
    }

    case '/api/autotrader/stop':
      state.running = false;
      log('AutoTrader stopped');
      broadcast({ type: 'status_change', data: { running: false } });
      return send(200, { success: true, message: 'AutoTrader stopped' });

    case '/api/risk/status':
      return send(200, riskStatusResponse());

    case '/api/risk/reset': {
      const clearCounters = url.searchParams.get('clear_counters') === 'true';
      state.risk.halt = null;
      if (clearCounters) {
        state.risk.realized_pnl_today_sol = 0;
        state.risk.trades_today = 0;
        state.risk.consecutive_losses = 0;
        state.risk.entry_refusals = 0;
      }
      log(`Risk guard reset (clear_counters=${clearCounters})`);
      return send(200, {
        success: true,
        message: clearCounters
          ? "Risk guard reset and today's counters cleared"
          : 'Risk guard reset — new entries allowed again',
        halted: false,
      });
    }

    case '/api/emergency/stop': {
      const flatten = url.searchParams.has('flatten')
        ? url.searchParams.get('flatten') === 'true'
        : config.emergency_flatten_positions;
      const reason = url.searchParams.get('reason') || 'operator pressed the kill switch';
      halt('manual', reason);
      state.running = false;
      let closed = 0;
      if (flatten) {
        for (const position of activePositions()) {
          closePosition(position, 'emergency_close');
          closed += 1;
        }
      }
      broadcast({ type: 'status_change', data: { running: false } });
      return send(200, {
        success: true,
        message: `Kill switch engaged. ${closed} position(s) closed. New entries stay blocked until POST /api/risk/reset.`,
        halted: true,
        flatten_requested: flatten,
        positions_closed: closed,
      });
    }

    case '/api/strategy/active':
      if (req.method === 'GET') return send(200, { strategy_type: state.active_strategy, name: labelFor(state.active_strategy) });
      return send(200, { success: true, strategy_type: state.active_strategy });

    case '/api/watchlist':
      return send(200, { tokens: [], count: 0 });

    case '/api/watchlist/stats':
      return send(200, { total_tokens: 12, active_tokens: 9, traded_tokens: 3, migrated_tokens: 4, max_capacity: 100 });

    case '/api/signals':
      return send(200, { signals: [], total: 0 });

    case '/api/signals/active':
      return send(200, { signals: activePositions().map((p) => ({ token_address: p.token_address, token_symbol: p.token_symbol })), total: activePositions().length });

    case '/api/simulation/stats':
      return send(200, {
        dry_run_mode: config.dry_run_mode,
        stats: { total_positions: 0, open_positions: 0, closed_positions: 0, total_pnl_sol: 0, win_rate: 0 },
      });

    case '/api/simulation/positions':
    case '/api/simulation/positions/open':
      return send(200, { positions: [], total: 0 });

    case '/api/simulation/clear':
      return send(200, { success: true, message: 'All simulated positions cleared' });

    case '/api/status':
      return send(200, {
        is_running: state.running,
        wallet_connected: true,
        network: 'mainnet-beta',
        active_positions: activePositions().length,
        autotrader_running: state.running,
      });

    case '/api/analyze': {
      const mint = url.searchParams.get('address') || 'unknown';
      const risk = Math.floor(Math.random() * 80) + 10;
      return send(200, {
        token_address: mint,
        risk_level: risk,
        risk_rating: risk < 30 ? 'Low' : risk < 55 ? 'Medium' : risk < 75 ? 'High' : 'Very High',
        liquidity_sol: 12 + Math.random() * 40,
        holder_count: 40 + Math.floor(Math.random() * 400),
        has_mint_authority: Math.random() < 0.3,
        has_freeze_authority: Math.random() < 0.2,
        lp_burned: Math.random() < 0.6,
        can_sell: Math.random() < 0.9,
        transfer_tax_percent: Math.random() < 0.5 ? 0 : Number((Math.random() * 8).toFixed(1)),
        top_holder_percent: Number((Math.random() * 40).toFixed(1)),
        analysis_details: ['harness: synthetic analysis'],
      });
    }

    default:
      if (endpoint.startsWith('/api/strategies')) {
        return send(200, {
          strategies: [
            {
              id: 'strategy-migrated-default',
              name: 'Migrated (harness)',
              enabled: true,
              max_concurrent_positions: 3,
              max_position_size_sol: 0.1,
              total_budget_sol: 1,
              stop_loss_percent: 15,
              take_profit_percent: 50,
              trailing_stop_percent: 15,
              max_hold_time_minutes: 240,
              min_liquidity_sol: 10,
              max_risk_level: 60,
              min_holders: 50,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            },
          ],
          total: 1,
        });
      }
      if (endpoint.startsWith('/api/copy/')) {
        return send(200, { success: true, registered: false, message: 'copy trading disabled in harness' });
      }
      if (endpoint.startsWith('/api/trade/')) {
        return send(200, { success: true, message: 'manual trade accepted (harness)' });
      }
      return send(404, { error: `harness: no mock for ${req.method} ${endpoint}` });
  }
}

function labelFor(strategy) {
  return { new_pairs: 'New Pairs', final_stretch: 'Final Stretch', migrated: 'Migrated', telegram_call: 'Telegram Call' }[strategy] || strategy;
}

// ---------------------------------------------------------------------------
// Static files (webapp/ with a patched config.js so it talks to this harness)
// ---------------------------------------------------------------------------
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
};

function serveStatic(res, pathname) {
  const relative = pathname === '/' ? 'index.html' : pathname.replace(/^\/+/, '');
  const filePath = path.join(WEBAPP_DIR, relative);

  if (!filePath.startsWith(WEBAPP_DIR)) {
    res.writeHead(403).end('Forbidden');
    return;
  }

  fs.readFile(filePath, (err, data) => {
    if (err) {
      res.writeHead(404, { 'Content-Type': 'text/plain' }).end(`harness: ${relative} not found`);
      return;
    }
    // Point the dashboard at this harness instead of the author's Fly.io backend.
    if (relative === 'js/config.js') {
      let patched = data
        .toString('utf8')
        .replace(/window\.API_BASE_URL\s*=\s*'[^']*';/, "window.API_BASE_URL = '';")
        .replace(/window\.WS_URL\s*=\s*'[^']*';/, "window.WS_URL = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws';");
      patched += `\n// [dev-harness] talking to the mock backend on this origin (API key ${API_KEY ? 'required' : 'not required'})\n`;
      res.writeHead(200, { 'Content-Type': MIME['.js'] });
      res.end(patched);
      return;
    }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(filePath)] || 'application/octet-stream' });
    res.end(data);
  });
}

// ---------------------------------------------------------------------------
// Minimal WebSocket server (RFC 6455, text frames only) — no dependencies
// ---------------------------------------------------------------------------
const WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const sockets = new Set();

function encodeFrame(payload, opcode = 0x1) {
  const data = Buffer.from(payload);
  const len = data.length;
  let header;
  if (len < 126) {
    header = Buffer.from([0x80 | opcode, len]);
  } else if (len < 65536) {
    header = Buffer.alloc(4);
    header[0] = 0x80 | opcode;
    header[1] = 126;
    header.writeUInt16BE(len, 2);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x80 | opcode;
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(len), 2);
  }
  return Buffer.concat([header, data]);
}

function handleUpgrade(req, socket) {
  const key = req.headers['sec-websocket-key'];
  if (!key) return socket.destroy();
  const accept = crypto.createHash('sha1').update(key + WS_GUID).digest('base64');
  socket.write(
    'HTTP/1.1 101 Switching Protocols\r\n' +
      'Upgrade: websocket\r\n' +
      'Connection: Upgrade\r\n' +
      `Sec-WebSocket-Accept: ${accept}\r\n\r\n`
  );
  sockets.add(socket);
  log('dashboard connected over websocket');
  pushSnapshot(socket);

  let buffer = Buffer.alloc(0);
  socket.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 2) {
      const opcode = buffer[0] & 0x0f;
      const masked = (buffer[1] & 0x80) !== 0;
      let length = buffer[1] & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (buffer.length < 4) return;
        length = buffer.readUInt16BE(2);
        offset = 4;
      } else if (length === 127) {
        if (buffer.length < 10) return;
        length = Number(buffer.readBigUInt64BE(2));
        offset = 10;
      }
      const maskLen = masked ? 4 : 0;
      if (buffer.length < offset + maskLen + length) return;

      let payload = buffer.slice(offset + maskLen, offset + maskLen + length);
      if (masked) {
        const mask = buffer.slice(offset, offset + 4);
        payload = Buffer.from(payload.map((byte, i) => byte ^ mask[i % 4]));
      }
      buffer = buffer.slice(offset + maskLen + length);

      if (opcode === 0x8) {
        socket.end(encodeFrame('', 0x8));
        sockets.delete(socket);
        return;
      }
      if (opcode === 0x9) {
        socket.write(encodeFrame(payload, 0xA));
        continue;
      }
      if (opcode === 0x1) {
        try {
          const message = JSON.parse(payload.toString('utf8'));
          if (message.type === 'ping') socket.write(encodeFrame(JSON.stringify({ type: 'pong', timestamp: new Date().toISOString() })));
          if (message.type === 'subscribe') pushSnapshot(socket);
        } catch (e) {
          /* ignore malformed frames */
        }
      }
    }
  });

  socket.on('close', () => {
    sockets.delete(socket);
    log('dashboard disconnected');
  });
  socket.on('error', () => sockets.delete(socket));
}

function broadcast(message) {
  const frame = encodeFrame(JSON.stringify(message));
  for (const socket of sockets) {
    try {
      socket.write(frame);
    } catch (e) {
      sockets.delete(socket);
    }
  }
}

function pushSnapshot(target) {
  const message = JSON.stringify({
    type: 'status_update',
    data: {
      running: state.running,
      halted: Boolean(state.risk.halt),
      active_positions: activePositions().length,
      total_pnl_sol: statsResponse().total_pnl_sol,
      wallet_balance_sol: state.wallet.balance_sol,
      timestamp: new Date().toISOString(),
    },
  });
  const frame = encodeFrame(message);
  if (target) {
    target.write(frame);
    return;
  }
  for (const socket of sockets) socket.write(frame);
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------
const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://${req.headers.host || 'localhost'}`);
  if (url.pathname.startsWith('/api/')) return route(req, res, url);
  return serveStatic(res, url.pathname);
});

server.on('upgrade', (req, socket) => {
  const url = new URL(req.url, `http://${req.headers.host || 'localhost'}`);
  if (url.pathname !== '/ws') return socket.destroy();
  if (API_KEY && url.searchParams.get('api_key') !== API_KEY) {
    socket.write('HTTP/1.1 401 Unauthorized\r\n\r\n');
    return socket.destroy();
  }
  handleUpgrade(req, socket);
});

setInterval(tick, TICK_MS);

server.listen(PORT, HOST, () => {
  console.log(`\n  TraderTony dev harness → http://localhost:${PORT}`);
  console.log(`  serving dashboard from ${WEBAPP_DIR}`);
  console.log(`  rails: ${JSON.stringify(config.rails)}`);
  console.log(`  api key: ${API_KEY ? 'REQUIRED (HARNESS_API_KEY)' : 'not required (open API)'}\n`);
});
