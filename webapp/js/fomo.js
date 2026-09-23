/**
 * TraderTony V4 — FOMO copy-trading panel
 * =======================================
 *
 * Floating card for the `FomoCopy` strategy. Shows who the bot is following
 * (top clan → top member), the mirror counters and the recent decisions, and
 * lets you re-run discovery or pin a clan/trader by hand.
 *
 * Talks straight to the backend with fetch() so it works with the real bot and
 * with the dev harness (tools/dev-harness/server.js).
 */

(function () {
    'use strict';

    var BASE_URL = (window.API_BASE_URL || '').replace(/\/$/, '');
    var POLL_MS = 8000;
    var state = { open: true, data: null, error: null, busy: false };

    function apiKey() {
        try {
            return localStorage.getItem('tony_api_key') || '';
        } catch (e) {
            return '';
        }
    }

    async function call(path, options) {
        var opts = options || {};
        var headers = { 'Content-Type': 'application/json' };
        if (apiKey()) headers['X-API-Key'] = apiKey();

        var response = await fetch(BASE_URL + path, {
            method: opts.method || 'GET',
            headers: headers,
            body: opts.body,
        });
        var payload = null;
        try {
            payload = await response.json();
        } catch (e) {
            payload = null;
        }
        if (!response.ok) {
            throw new Error((payload && (payload.error || payload.details)) || ('HTTP ' + response.status));
        }
        return payload;
    }

    function el(tag, style, text) {
        var node = document.createElement(tag);
        if (style) Object.assign(node.style, style);
        if (text !== undefined) node.textContent = text;
        return node;
    }

    function button(label, background, onClick) {
        var b = el('button', {
            background: background, color: '#fff', border: 'none', borderRadius: '6px',
            padding: '6px 10px', fontSize: '12px', fontWeight: '600', cursor: 'pointer',
        }, label);
        b.addEventListener('click', onClick);
        return b;
    }

    function money(value) {
        var n = Number(value || 0);
        var sign = n < 0 ? '-' : '';
        n = Math.abs(n);
        if (n >= 1e6) return sign + '$' + (n / 1e6).toFixed(2) + 'M';
        if (n >= 1e3) return sign + '$' + (n / 1e3).toFixed(1) + 'K';
        return sign + '$' + n.toFixed(2);
    }

    function ago(iso) {
        if (!iso) return 'never';
        var seconds = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
        if (seconds < 60) return seconds + 's ago';
        if (seconds < 3600) return Math.floor(seconds / 60) + 'm ago';
        if (seconds < 86400) return Math.floor(seconds / 3600) + 'h ago';
        return Math.floor(seconds / 86400) + 'd ago';
    }

    var root = el('div', {
        position: 'fixed', left: '16px', bottom: '16px', width: '340px', zIndex: '9999',
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: '12px',
        background: 'rgba(12, 14, 22, 0.96)', color: '#e6e8ef', border: '1px solid #2a2f45',
        borderRadius: '10px', boxShadow: '0 10px 30px rgba(0,0,0,0.45)', overflow: 'hidden',
    });

    var header = el('div', {
        display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        padding: '8px 10px', background: '#161a2b', cursor: 'pointer', userSelect: 'none',
    });
    var badge = el('span', {
        padding: '2px 8px', borderRadius: '999px', background: '#2a2f45', color: '#9aa3bd', fontSize: '11px',
    }, '…');
    header.appendChild(el('span', { fontWeight: '700', letterSpacing: '0.04em' }, 'FOMO COPY'));
    header.appendChild(badge);

    var body = el('div', { padding: '10px' });
    var target = el('div', { lineHeight: '1.6' });
    var counters = el('div', { marginTop: '6px', color: '#9aa3bd' });
    var feed = el('div', { marginTop: '8px', borderTop: '1px solid #232840', paddingTop: '8px', maxHeight: '150px', overflowY: 'auto' });
    var actions = el('div', { marginTop: '10px', display: 'flex', gap: '6px', flexWrap: 'wrap' });
    var note = el('div', { marginTop: '8px', color: '#8d97b5', fontSize: '11px', whiteSpace: 'pre-wrap' });

    body.appendChild(target);
    body.appendChild(counters);
    body.appendChild(feed);
    body.appendChild(actions);
    body.appendChild(note);
    root.appendChild(header);
    root.appendChild(body);

    header.addEventListener('click', function () {
        state.open = !state.open;
        body.style.display = state.open ? 'block' : 'none';
    });

    function render() {
        actions.innerHTML = '';

        actions.appendChild(button('Refresh target', '#2563eb', async function () {
            if (state.busy) return;
            state.busy = true;
            note.textContent = 'Running discovery (top clan → top member)…';
            try {
                var result = await call('/api/fomo/refresh', { method: 'POST' });
                note.textContent = result.message || 'Target refreshed';
                state.error = null;
            } catch (error) {
                state.error = error.message;
            } finally {
                state.busy = false;
                await refresh();
            }
        }));

        actions.appendChild(button('Pin target', '#374151', async function () {
            var clan = window.prompt('Clan id to copy (leave empty to use automatic top-clan discovery):', '');
            if (clan === null) return;
            var trader = window.prompt('…or a trader handle to copy directly (leave empty for none):', '');
            if (trader === null) return;
            try {
                var result = await call('/api/fomo/target', {
                    method: 'POST',
                    body: JSON.stringify({ clan_id: clan, trader: trader }),
                });
                note.textContent = result.message || 'Target pinned';
                state.error = null;
            } catch (error) {
                state.error = error.message;
            }
            await refresh();
        }));

        if (state.error) {
            badge.textContent = 'ERROR';
            badge.style.background = '#4b1d1d';
            badge.style.color = '#ffb4b4';
            note.textContent = state.error;
            return;
        }

        var d = state.data;
        if (!d) {
            badge.textContent = 'LOADING';
            return;
        }

        var running = Boolean(d.running);
        var target_ok = d.target && (d.target.trader_handle || d.target.clan_name);
        badge.textContent = !d.enabled ? 'UNCONFIGURED' : running && target_ok ? 'COPING' : running ? 'SEARCHING' : 'IDLE';
        badge.style.background = !d.enabled ? '#3a2f16' : running ? '#12351f' : '#2a2f45';
        badge.style.color = !d.enabled ? '#ffd88f' : running ? '#8ff0b0' : '#9aa3bd';

        target.innerHTML = '';
        var clanLine = el('div');
        clanLine.appendChild(el('span', { color: '#8d97b5' }, 'clan: '));
        clanLine.appendChild(el('span', {}, d.target && d.target.clan_name
            ? d.target.clan_name + ' (' + money(d.target.clan_pnl_usd) + ' ' + d.window + ')'
            : 'not resolved yet'));
        target.appendChild(clanLine);

        var traderLine = el('div');
        traderLine.appendChild(el('span', { color: '#8d97b5' }, 'copying: '));
        traderLine.appendChild(el('span', { color: '#8ff0b0', fontWeight: '600' }, d.target && d.target.trader_handle
            ? '@' + d.target.trader_handle + ' (' + money(d.target.trader_pnl_usd) + ')'
            : '—'));
        target.appendChild(traderLine);

        var meta = el('div', { color: '#8d97b5' });
        meta.textContent = d.provider + ' · ' + d.mode + ' · ' + d.window +
            ' · discovery ' + ago(d.last_discovery) + ' · poll ' + ago(d.last_poll);
        target.appendChild(meta);

        counters.textContent = 'mirrored ' + d.mirrored + ' · skipped ' + d.skipped + ' · failed ' + d.failed +
            ' · size ' + (d.limits.size_mode === 'proportional'
                ? (d.limits.copy_ratio * 100).toFixed(1) + '% of notional'
                : d.limits.copy_size_sol + ' SOL') +
            ' · max ' + d.limits.max_positions + ' open';

        if (d.last_error) {
            note.textContent = 'last error: ' + d.last_error;
        } else if (!d.enabled) {
            note.textContent = 'Set FOMO_PROVIDER + FOMO_API_KEY (or FOMO_API_BASE) and restart, then switch the active strategy to "FOMO Leaderboard Copy".';
        }

        feed.innerHTML = '';
        (d.recent || []).slice(0, 8).forEach(function (entry) {
            var color = entry.action === 'buy' ? '#8ff0b0'
                : entry.action === 'sell' ? '#ffb4b4'
                : entry.action === 'failed' ? '#ff9f43' : '#8d97b5';
            var line = el('div', { marginBottom: '4px' });
            line.appendChild(el('span', { color: color, fontWeight: '600' }, entry.action.toUpperCase() + ' '));
            line.appendChild(el('span', {}, (entry.token_symbol || (entry.token_address || '').slice(0, 6) + '…') + ' '));
            line.appendChild(el('span', { color: '#6f7891', fontSize: '11px' }, ago(entry.at)));
            var detail = el('div', { color: '#6f7891', fontSize: '11px', marginLeft: '8px' }, entry.detail);
            line.appendChild(detail);
            feed.appendChild(line);
        });
        if (!feed.childNodes.length) {
            feed.appendChild(el('div', { color: '#6f7891' }, 'no decisions yet'));
        }
    }

    async function refresh() {
        try {
            state.data = await call('/api/fomo/status');
            state.error = null;
        } catch (error) {
            state.error = error.message;
        }
        render();
    }

    function mount() {
        document.body.appendChild(root);
        render();
        refresh();
        setInterval(refresh, POLL_MS);
        if (window.wsClient && window.wsClient.on) {
            window.wsClient.on('onStatusChange', function () { refresh(); });
        }
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', mount);
    } else {
        mount();
    }
})();
