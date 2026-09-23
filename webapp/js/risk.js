/**
 * TraderTony V4 — Risk Guard panel
 * =================================
 *
 * Self-contained UI for the Track A safety rails. Injected as a floating card so
 * it needs no changes to the existing dashboard markup:
 *
 *   - live view of GET /api/risk/status (daily PnL, trades today, losing streak,
 *     equity vs peak, refused entries, mints in cooldown)
 *   - the kill switch (POST /api/emergency/stop), with an optional full flatten
 *   - "Reset halt" (POST /api/risk/reset)
 *   - API-key entry, stored in localStorage as `tony_api_key` and sent on every
 *     REST call + WebSocket handshake
 *
 * Loaded after js/api.js. Talks straight to the backend with fetch() so it keeps
 * working even when the dashboard's demo-mode mock is enabled.
 */

(function () {
    'use strict';

    var BASE_URL = (window.API_BASE_URL || '').replace(/\/$/, '');
    var POLL_MS = 5000;
    var state = { open: true, data: null, busy: false, error: null };

    function apiKey() {
        try {
            return localStorage.getItem('tony_api_key') || '';
        } catch (e) {
            return '';
        }
    }

    function saveApiKey(key) {
        try {
            if (key) {
                localStorage.setItem('tony_api_key', key);
            } else {
                localStorage.removeItem('tony_api_key');
            }
        } catch (e) {
            console.warn('[RiskPanel] Could not store API key', e);
        }
    }

    async function call(path, options) {
        var opts = options || {};
        var headers = { 'Content-Type': 'application/json' };
        if (apiKey()) {
            headers['X-API-Key'] = apiKey();
        }
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
            var message = (payload && (payload.error || payload.details)) || ('HTTP ' + response.status);
            throw new Error(message);
        }
        return payload;
    }

    function fmtSol(value, digits) {
        var n = Number(value || 0);
        return (n >= 0 ? '+' : '') + n.toFixed(digits === undefined ? 4 : digits) + ' SOL';
    }

    function el(tag, style, text) {
        var node = document.createElement(tag);
        if (style) Object.assign(node.style, style);
        if (text !== undefined) node.textContent = text;
        return node;
    }

    function button(label, background, onClick) {
        var b = el('button', {
            background: background,
            color: '#fff',
            border: 'none',
            borderRadius: '6px',
            padding: '6px 10px',
            fontSize: '12px',
            fontWeight: '600',
            cursor: 'pointer',
            marginRight: '6px',
        }, label);
        b.addEventListener('click', onClick);
        return b;
    }

    var root = el('div', {
        position: 'fixed',
        right: '16px',
        bottom: '16px',
        width: '320px',
        zIndex: '9999',
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
        fontSize: '12px',
        background: 'rgba(12, 14, 22, 0.96)',
        color: '#e6e8ef',
        border: '1px solid #2a2f45',
        borderRadius: '10px',
        boxShadow: '0 10px 30px rgba(0,0,0,0.45)',
        overflow: 'hidden',
    });

    var header = el('div', {
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: '8px 10px',
        background: '#161a2b',
        cursor: 'pointer',
        userSelect: 'none',
    });
    var title = el('span', { fontWeight: '700', letterSpacing: '0.04em' }, 'RISK GUARD');
    var badge = el('span', {
        padding: '2px 8px',
        borderRadius: '999px',
        background: '#2a2f45',
        color: '#9aa3bd',
        fontSize: '11px',
    }, '…');
    header.appendChild(title);
    header.appendChild(badge);

    var body = el('div', { padding: '10px' });
    var metrics = el('div', { lineHeight: '1.6' });
    var cooldowns = el('div', { marginTop: '8px', color: '#9aa3bd' });
    var actions = el('div', { marginTop: '10px', display: 'flex', flexWrap: 'wrap', gap: '6px' });
    var note = el('div', { marginTop: '8px', color: '#8d97b5', fontSize: '11px', whiteSpace: 'pre-wrap' });

    var flattenToggle = el('label', { display: 'flex', alignItems: 'center', gap: '6px', marginTop: '8px', color: '#9aa3bd' });
    var flattenBox = document.createElement('input');
    flattenBox.type = 'checkbox';
    flattenToggle.appendChild(flattenBox);
    flattenToggle.appendChild(document.createTextNode('flatten open positions on kill'));

    body.appendChild(metrics);
    body.appendChild(cooldowns);
    body.appendChild(flattenToggle);
    body.appendChild(actions);
    body.appendChild(note);
    root.appendChild(header);
    root.appendChild(body);

    header.addEventListener('click', function () {
        state.open = !state.open;
        body.style.display = state.open ? 'block' : 'none';
    });

    function render() {
        var d = state.data;

        actions.innerHTML = '';
        actions.appendChild(
            button('KILL SWITCH', '#c0392b', function () {
                var flatten = flattenBox.checked;
                if (!window.confirm('Stop the bot' + (flatten ? ' and market-sell every open position' : '') + '?')) return;
                state.busy = true;
                render();
                call('/api/emergency/stop?flatten=' + (flatten ? 'true' : 'false'), { method: 'POST' })
                    .then(function (result) {
                        state.error = null;
                        note.textContent = result && result.message ? result.message : 'Kill switch engaged.';
                        return refresh();
                    })
                    .catch(function (error) {
                        state.error = error.message;
                    })
                    .finally(function () {
                        state.busy = false;
                        render();
                    });
            })
        );
        actions.appendChild(
            button('Reset halt', '#2563eb', function () {
                state.busy = true;
                render();
                call('/api/risk/reset', { method: 'POST' })
                    .then(function (result) {
                        state.error = null;
                        note.textContent = result && result.message ? result.message : 'Risk guard reset.';
                        return refresh();
                    })
                    .catch(function (error) {
                        state.error = error.message;
                    })
                    .finally(function () {
                        state.busy = false;
                        render();
                    });
            })
        );
        actions.appendChild(
            button(apiKey() ? 'API key ✓' : 'API key', '#374151', function () {
                var current = apiKey();
                var next = window.prompt('API key for this bot (sent as X-API-Key). Leave empty to clear.', current);
                if (next === null) return;
                saveApiKey(next.trim());
                note.textContent = next.trim() ? 'API key saved — reconnecting with credentials.' : 'API key cleared.';
                refresh();
            })
        );

        if (state.error) {
            badge.textContent = 'ERROR';
            badge.style.background = '#4b1d1d';
            badge.style.color = '#ffb4b4';
            metrics.innerHTML = '';
            note.textContent = state.error;
            return;
        }

        if (!d) {
            badge.textContent = 'LOADING';
            return;
        }

        var halted = Boolean(d.halted);
        badge.textContent = halted ? 'HALTED' : 'ACTIVE';
        badge.style.background = halted ? '#4b1d1d' : '#12351f';
        badge.style.color = halted ? '#ffb4b4' : '#8ff0b0';

        var limits = d.limits || {};
        metrics.innerHTML = '';
        var rows = [
            ['entry gate', halted ? 'BLOCKED — ' + (d.halt_kind || 'halt') : 'open'],
            ['halt reason', halted ? d.halt_reason || '' : '—'],
            ['PnL today', fmtSol(d.realized_pnl_today_sol) + ' / limit ' + (limits.daily_loss_limit_sol ? '-' + limits.daily_loss_limit_sol : 'off')],
            ['PnL total', fmtSol(d.realized_pnl_total_sol)],
            ['entries today', d.trades_today + (limits.max_trades_per_day ? ' / ' + limits.max_trades_per_day : ' / ∞')],
            ['losing streak', d.consecutive_losses + (limits.max_consecutive_losses ? ' / ' + limits.max_consecutive_losses : '')],
            ['equity / peak', Number(d.equity_sol || 0).toFixed(4) + ' / ' + Number(d.peak_equity_sol || 0).toFixed(4)],
            ['win / loss', d.wins + ' / ' + d.losses],
            ['entries refused', String(d.entry_refusals)],
        ];
        rows.forEach(function (row) {
            var line = el('div');
            line.appendChild(el('span', { color: '#8d97b5' }, row[0] + ': '));
            line.appendChild(el('span', {}, String(row[1])));
            metrics.appendChild(line);
        });

        var cool = (d.tokens_in_cooldown || []).slice(0, 3);
        cooldowns.textContent = cool.length
            ? 'cooldown: ' + cool.map(function (c) { return c.token_address.slice(0, 6) + '… ' + Math.round(c.remaining_seconds) + 's'; }).join(', ')
            : '';
    }

    async function refresh() {
        try {
            state.data = await call('/api/risk/status');
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
        // Re-render when the bot state changes over the WebSocket.
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
