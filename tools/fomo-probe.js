#!/usr/bin/env node
/**
 * fomo.family data-source probe
 * =============================
 *
 * fomo.family has no public API and Cloudflare blocks most non-browser clients,
 * so *which* data source works depends on your machine/network. Run this from
 * the box that will host the bot, before configuring it:
 *
 *   node tools/fomo-probe.js                          # probe everything configured
 *   node tools/fomo-probe.js --key fomo_live_xxx      # try the fomoapi.io mirror
 *   node tools/fomo-probe.js --token <jwt> --cookie '__cf_bm=…'   # official session
 *   node tools/fomo-probe.js --base https://my-proxy.example.com --key k
 *   node tools/fomo-probe.js --window 24h --dump      # print raw payloads
 *
 * It reports, per provider: HTTP status, whether the shape parsed, and the top
 * clan / top member it would pick. Nothing is written except stdout, and the
 * token is never logged (only its length).
 *
 * Env vars are also honoured: FOMO_PROVIDER, FOMO_API_BASE, FOMO_API_KEY,
 * FOMO_AUTH_TOKEN, FOMO_CF_COOKIE, FOMO_WINDOW.
 */

'use strict';

const args = process.argv.slice(2);

function flag(name, fallback) {
  const i = args.indexOf('--' + name);
  if (i === -1) return fallback;
  const next = args[i + 1];
  return next && !next.startsWith('--') ? next : true;
}

const WINDOW = String(flag('window', process.env.FOMO_WINDOW || '24h'));
const DUMP = Boolean(flag('dump', false));
const TIMEOUT_MS = Number(flag('timeout', 20000));

const targets = [];

if (process.env.FOMO_AUTH_TOKEN || flag('token')) {
  targets.push({
    name: 'official (fomo.family, JWT)',
    base: String(flag('base', process.env.FOMO_API_BASE || 'https://prod-api.fomo.family')),
    token: String(flag('token', process.env.FOMO_AUTH_TOKEN || '')),
    cookie: flag('cookie', process.env.FOMO_CF_COOKIE || '') || undefined,
  });
}
if (process.env.FOMO_API_KEY || flag('key')) {
  const key = String(flag('key', process.env.FOMO_API_KEY || ''));
  targets.push({
    name: 'fomoapi.io mirror',
    base: String(flag('base', 'https://api.fomoapi.io')),
    token: key,
  });
}
if (flag('base') && !targets.some((t) => t.base === String(flag('base')))) {
  targets.push({
    name: 'custom base URL',
    base: String(flag('base')),
    token: String(flag('key', process.env.FOMO_API_KEY || '')),
    cookie: flag('cookie', undefined) || undefined,
  });
}
if (!targets.length) {
  targets.push({ name: 'official (no credentials — expect 401/430)', base: 'https://prod-api.fomo.family', token: '' });
  targets.push({ name: 'fomoapi.io mirror (no key — expect 401)', base: 'https://api.fomoapi.io', token: '' });
}

async function get(target, path) {
  const url = target.base.replace(/\/$/, '') + path;
  const headers = { accept: 'application/json' };
  if (target.token) headers.authorization = 'Bearer ' + target.token;
  if (target.cookie) headers.cookie = target.cookie;
  if (target.base.includes('fomo.family')) {
    headers['x-supported-chains'] = '1399811149,8453,1,56,4663';
    headers.origin = 'https://fomo.family';
    headers.referer = 'https://fomo.family/';
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
  try {
    const response = await fetch(url, { headers, signal: controller.signal });
    const text = await response.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch (e) {
      /* not JSON */
    }
    return { url, status: response.status, json, text };
  } catch (error) {
    return { url, status: 0, json: null, text: String(error.message || error) };
  } finally {
    clearTimeout(timer);
  }
}

/** Pull the first non-empty array out of the shapes both providers use. */
function rows(json, keys) {
  if (!json) return [];
  const roots = [json, json.responseObject, json.data].filter(Boolean);
  for (const root of roots) {
    for (const key of keys) {
      if (Array.isArray(root[key]) && root[key].length) return root[key];
    }
  }
  return [];
}

function firstNumber(row, keys) {
  for (const key of keys) {
    const value = row[key];
    if (typeof value === 'number') return value;
    if (typeof value === 'string' && value.trim() && !Number.isNaN(Number(value))) return Number(value);
  }
  return 0;
}

function describeClan(row) {
  const node = row.clan && typeof row.clan === 'object' ? row.clan : row;
  return {
    id: node.id || node.clanId || row.clanId || node.name || '?',
    name: node.name || node.displayName || row.name || '?',
    pnl: firstNumber(row, ['pnlUsd', 'pnl', `pnl${WINDOW}`, 'pnl24h', 'totalPnl']),
  };
}

function describeTrader(row) {
  return {
    id: row.id || row.userId || '?',
    handle: row.userHandle || row.handle || '?',
    pnl: firstNumber(row, ['pnlUsd', 'pnl', `pnl${WINDOW}`, 'pnl24h']),
    clan: row.clan && typeof row.clan === 'object' ? row.clan.name : row.clan || null,
  };
}

async function probe(target) {
  console.log('\n──────────────────────────────────────────────────────────────');
  console.log(`provider: ${target.name}`);
  console.log(`base:     ${target.base}`);
  console.log(`auth:     ${target.token ? 'Bearer (' + target.token.length + ' chars)' : 'none'}`);
  if (target.cookie) console.log(`cookie:   ${String(target.cookie).length} chars`);

  const clanBoard = await get(target, `/v2/clans/leaderboard?window=${WINDOW}&limit=50`);
  const traderBoard = await get(target, `/v2/leaderboard/${WINDOW}?limit=50`);
  const clans = rows(clanBoard.json, ['leaderboard', 'clans', 'items', 'rows']);
  const traders = rows(traderBoard.json, ['leaderboard', 'traders', 'items', 'rows']);

  console.log(`\nclan board    HTTP ${clanBoard.status}  ${clans.length} row(s)`);
  console.log(`trader board  HTTP ${traderBoard.status}  ${traders.length} row(s)`);

  if (clanBoard.status === 430 || String(clanBoard.text).includes('"error":"unauthorized"')) {
    console.log('  → Cloudflare rejected this client (HTTP 430). This host cannot read fomo.family directly.');
    console.log('    Use the fomoapi mirror (--key), a proxy/bridge (--base), or pin a target manually.');
  }
  if (clanBoard.status === 401 || traderBoard.status === 401) {
    console.log('  → 401: a credential is required for this endpoint.');
  }

  const topClan = clans.length ? describeClan(clans[0]) : null;
  if (topClan) {
    console.log(`\ntop clan:  ${topClan.name}  (${topClan.id})  pnl=${topClan.pnl}`);
    const members = await get(target, `/v2/clans/${encodeURIComponent(topClan.id)}/members?limit=100`);
    const memberRows = rows(members.json, ['members', 'leaderboard', 'traders', 'items', 'rows']);
    console.log(`members    HTTP ${members.status}  ${memberRows.length} row(s)`);
    if (memberRows.length) {
      const sorted = memberRows.map(describeTrader).sort((a, b) => b.pnl - a.pnl);
      console.log(`top member: @${sorted[0].handle}  (${sorted[0].id})  pnl=${sorted[0].pnl}`);
      const swaps = await get(target, `/v2/users/${encodeURIComponent(sorted[0].id)}/swaps?limit=5`);
      const swapRows = rows(swaps.json, ['swaps', 'positions', 'trades', 'items']);
      console.log(`swaps      HTTP ${swaps.status}  ${swapRows.length} row(s)`);
      if (swapRows.length) {
        const sample = swapRows[0];
        console.log('sample swap fields:', Object.keys(sample).slice(0, 14).join(', '));
      }
      if (DUMP && swapRows.length) console.log(JSON.stringify(swapRows.slice(0, 2), null, 2));
    } else if (DUMP) {
      console.log(JSON.stringify(members.json, null, 2).slice(0, 2000));
    }
  } else if (traders.length) {
    const top = traders.map(describeTrader).sort((a, b) => b.pnl - a.pnl)[0];
    console.log(`\n(no clan board) top trader: @${top.handle}  (${top.id})  pnl=${top.pnl}  clan=${top.clan || '—'}`);
    const swaps = await get(target, `/v2/users/${encodeURIComponent(top.id)}/swaps?limit=5`);
    const swapRows = rows(swaps.json, ['swaps', 'positions', 'trades', 'items']);
    console.log(`swaps      HTTP ${swaps.status}  ${swapRows.length} row(s)`);
  }

  if (DUMP && traders.length) console.log(JSON.stringify(traders.slice(0, 2), null, 2));
}

(async () => {
  console.log('fomo data-source probe — window=' + WINDOW);
  for (const target of targets) {
    try {
      await probe(target);
    } catch (error) {
      console.log(`probe failed: ${error.message}`);
    }
  }
  console.log('\nSummary: use whichever provider reported rows for the *clan board* (best fidelity for this strategy).');
  console.log('If only the trader board works, the bot still derives clans from the traders\' clan field.');
})();
