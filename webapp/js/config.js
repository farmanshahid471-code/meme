/**
 * TraderTony V4 - Configuration
 * Environment-specific settings
 * VERSION: 2026-06-10-v4 (Fly.io backend)
 */

// Hosted backend URLs (Fly.io, moved off Railway June 2026).
// DIFFERENT BACKEND? These two lines are the only place the hosted API and
// WebSocket URLs live — edit them, or re-point just API_BASE_URL before this
// file loads. (The dev harness rewrites both, anchored to the line start, so
// keep them unindented like this.)
window.API_BASE_URL = 'https://trader-tony-v4.fly.dev';
window.WS_URL = 'wss://trader-tony-v4.fly.dev/ws';

// ---------------------------------------------------------------------------
// Local development auto-detect
// If this page is opened from your own machine (localhost, 127.0.0.1, *.local,
// or straight off disk with file://), talk to a bot running locally on port
// 3030 instead of the author's hosted backend. The dev harness
// (tools/dev-harness) sets window.API_BASE_URL itself and is left alone.
// ---------------------------------------------------------------------------
(function () {
    var HOSTED_DEFAULT = 'https://trader-tony-v4.fly.dev';
    var host = window.location.hostname;
    var isLocalPage =
        window.location.protocol === 'file:' ||
        host === 'localhost' || host === '127.0.0.1' || host === '0.0.0.0' ||
        host === '::1' || host === '[::1]' || /\.local$/.test(host);

    if (isLocalPage && window.API_BASE_URL === HOSTED_DEFAULT) {
        window.API_BASE_URL = 'http://localhost:3030';
        window.WS_URL = 'ws://localhost:3030/ws';
        console.log('[Config] Local page detected — using http://localhost:3030 (bot on this machine)');
    }
})();

console.log('[Config] VERSION: 2026-06-10-v4');
console.log('[Config] API_BASE_URL:', window.API_BASE_URL);

// WebSocket URL is set above - Railway backend only

// Feature flags
window.FEATURES = {
    // Enable copy trading feature
    copyTrading: false,

    // Enable manual trading controls
    manualTrading: true,

    // Enable demo mode by default if backend is unavailable
    autoEnableDemoMode: true,

    // Show detailed debug logs in console
    debugMode: true,
};

// Network configuration
window.NETWORK_CONFIG = {
    // 'mainnet-beta' or 'devnet'
    network: 'mainnet-beta',

    // RPC endpoint (for wallet interactions)
    rpcEndpoint: 'https://api.mainnet-beta.solana.com',

    // Solscan base URL
    solscanBaseUrl: 'https://solscan.io',
};

// Copy trade settings
window.COPY_TRADE_CONFIG = {
    // Fee percentage on profits
    feePercent: 10,

    // Minimum SOL balance required
    minBalance: 0.1,

    // Maximum position size in SOL
    maxPositionSize: 5.0,
};

console.log('[Config] TraderTony V4 Dashboard Configuration loaded');
console.log('[Config] Network:', window.NETWORK_CONFIG.network);
console.log('[Config] Features:', window.FEATURES);
