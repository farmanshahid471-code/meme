use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    // Solana Configuration
    pub solana_rpc_url: String,
    pub solana_ws_url: String,
    pub solana_private_key: String,
    pub network: String,

    // API Keys
    pub helius_api_key: String,
    pub jupiter_api_key: Option<String>,
    pub birdeye_api_key: Option<String>,
    pub moralis_api_key: Option<String>,

    // Telegram Sniper Configuration
    pub tg_api_id: Option<i32>,
    pub tg_api_hash: Option<String>,
    pub tg_phone: Option<String>,
    pub tg_channel: Option<String>,         // e.g. "cryptoyeezuscalls" or "@cryptoyeezuscalls"
    pub tg_session_path: String,            // default "data/tg_session.session"

    // Snipe Execution
    pub snipe_amount_sol: f64,              // default 0.25
    pub snipe_slippage_bps: u32,            // default 1500 (15%)
    pub snipe_priority_fee_micro_lamports: u64,  // default 1_000_000 (1M μlamports = high priority)
    pub snipe_exit_delay_ms: u64,           // default 3000 (3 seconds)
    pub snipe_exit_percent: u32,            // default 90

    // Web API Configuration
    pub api_host: Option<String>,
    pub api_port: Option<u16>,
    pub cors_origins: Vec<String>,
    pub auto_start_trading: bool,
    /// Shared secret required on every mutating API call. `None`/empty = auth disabled (warns at boot).
    pub api_key: Option<String>,

    // Risk Guard (portfolio-level safety rails)
    pub daily_loss_limit_sol: Option<f64>,
    pub max_drawdown_percent: Option<f64>,
    pub max_trades_per_day: Option<u32>,
    pub max_consecutive_losses: Option<u32>,
    pub token_cooldown_minutes: u32,
    /// When the kill switch fires, also market-sell every open position.
    pub emergency_flatten_positions: bool,

    // =========================================================================
    // FOMO leaderboard copy-trading
    // =========================================================================
    /// `official` | `fomoapi` | `custom`.
    pub fomo_provider: String,
    /// Base URL override (needed for `custom`, optional otherwise).
    pub fomo_api_base: Option<String>,
    /// Bearer token: a fomoapi.io key, an official JWT, or a custom-endpoint key.
    pub fomo_api_key: Option<String>,
    /// Explicit bearer token, wins over `fomo_api_key` (official JWT).
    pub fomo_auth_token: Option<String>,
    /// `Cookie` header, e.g. `__cf_bm=...` for the official endpoint.
    pub fomo_cf_cookie: Option<String>,
    /// Leaderboard window: `24h` | `7d` | `30d` | `all`.
    pub fomo_window: String,
    /// Pin the clan instead of discovering the top one.
    pub fomo_clan_id: Option<String>,
    /// Pin the trader (handle) instead of clan -> top member discovery.
    pub fomo_trader_handle: Option<String>,
    pub fomo_trader_id: Option<String>,
    /// Path template for clan members; `{clan_id}` and `{window}` are substituted.
    pub fomo_clan_members_path: String,
    /// How often to re-run clan/member discovery (default: daily).
    pub fomo_refresh_secs: u64,
    /// How often to poll the copied trader's swaps.
    pub fomo_poll_secs: u64,
    /// Fixed copy size in SOL (`FOMO_SIZE_MODE=fixed`).
    pub fomo_copy_size_sol: f64,
    /// `fixed` | `proportional`.
    pub fomo_size_mode: String,
    /// Fraction of the copied trade's notional used when `proportional`.
    pub fomo_copy_ratio: f64,
    /// Close our position when the copied trader sells the same mint.
    pub fomo_mirror_sells: bool,
    /// Ignore copied swaps smaller than this (USD).
    pub fomo_min_swap_usd: f64,
    /// Maximum concurrent copied positions.
    pub fomo_max_positions: u32,
    /// Network ids treated as Solana.
    pub fomo_solana_network_ids: Vec<String>,

    // Copy Trade Configuration
    pub treasury_wallet: Option<String>,
    pub copy_trade_fee_percent: f64,

    // Trading Configuration
    pub demo_mode: bool,
    pub dry_run_mode: bool,  // Scans real tokens, simulates trades without execution
    pub max_position_size_sol: f64,
    pub total_budget_sol: f64,
    pub default_stop_loss_percent: u32,
    pub default_take_profit_percent: u32,
    pub default_trailing_stop_percent: u32,
    pub max_hold_time_minutes: u32,

    // Risk Parameters
    pub min_liquidity_sol: u32,
    pub max_risk_level: u32,
    pub min_holders: u32,

    // Transaction Parameters
    pub default_slippage_bps: u32,
    pub default_priority_fee_micro_lamports: u64,
}

impl Config {
    pub fn load() -> Result<Self> {
        // Parse CORS origins from comma-separated string
        let cors_origins: Vec<String> = env::var("CORS_ORIGINS")
            .unwrap_or_else(|_| "*".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(Self {
            // Solana Configuration
            solana_rpc_url: env::var("SOLANA_RPC_URL")
                .context("SOLANA_RPC_URL not set in environment")?,
            solana_ws_url: env::var("SOLANA_WS_URL")
                .unwrap_or_else(|_| {
                    // Derive WebSocket URL from RPC URL if not provided
                    let rpc = env::var("SOLANA_RPC_URL").unwrap_or_default();
                    rpc.replace("https://", "wss://").replace("http://", "ws://")
                }),
            solana_private_key: env::var("WALLET_PRIVATE_KEY")
                .or_else(|_| env::var("SOLANA_PRIVATE_KEY"))
                .context("WALLET_PRIVATE_KEY or SOLANA_PRIVATE_KEY not set in environment")?,
            network: env::var("NETWORK").unwrap_or_else(|_| "mainnet".to_string()),

            // API Keys
            helius_api_key: env::var("HELIUS_API_KEY")
                .context("HELIUS_API_KEY not set in environment")?,
            jupiter_api_key: env::var("JUPITER_API_KEY").ok(),
            birdeye_api_key: env::var("BIRDEYE_API_KEY").ok(),
            moralis_api_key: env::var("MORALIS_API_KEY").ok(),

            // Telegram Sniper
            tg_api_id: env::var("TG_API_ID").ok().and_then(|v| v.parse().ok()),
            tg_api_hash: env::var("TG_API_HASH").ok(),
            tg_phone: env::var("TG_PHONE").ok(),
            tg_channel: env::var("TG_CHANNEL").ok(),
            tg_session_path: env::var("TG_SESSION_PATH")
                .unwrap_or_else(|_| "data/tg_session.session".to_string()),

            // Snipe Execution
            snipe_amount_sol: env::var("SNIPE_AMOUNT_SOL")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(0.25),
            snipe_slippage_bps: env::var("SNIPE_SLIPPAGE_BPS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(1500),
            snipe_priority_fee_micro_lamports: env::var("SNIPE_PRIORITY_FEE_MICRO_LAMPORTS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(1_000_000),
            snipe_exit_delay_ms: env::var("SNIPE_EXIT_DELAY_MS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(3000),
            snipe_exit_percent: env::var("SNIPE_EXIT_PERCENT")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(90),

            // Web API Configuration
            api_host: env::var("API_HOST").ok(),
            api_port: env::var("API_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .or_else(|| env::var("PORT").ok().and_then(|v| v.parse().ok())), // Railway uses PORT
            cors_origins,
            auto_start_trading: env::var("AUTO_START_TRADING")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),
            api_key: env::var("API_KEY")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),

            // Risk Guard
            daily_loss_limit_sol: parse_optional_f64("DAILY_LOSS_LIMIT_SOL"),
            max_drawdown_percent: parse_optional_f64("MAX_DRAWDOWN_PERCENT"),
            max_trades_per_day: env::var("MAX_TRADES_PER_DAY")
                .ok()
                .and_then(|v| v.trim().parse().ok()),
            max_consecutive_losses: env::var("MAX_CONSECUTIVE_LOSSES")
                .ok()
                .and_then(|v| v.trim().parse().ok()),
            token_cooldown_minutes: env::var("TOKEN_COOLDOWN_MINUTES")
                .unwrap_or_else(|_| "0".to_string())
                .parse()
                .unwrap_or(0),
            emergency_flatten_positions: env::var("EMERGENCY_FLATTEN_POSITIONS")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),

            // FOMO leaderboard copy-trading
            fomo_provider: env::var("FOMO_PROVIDER")
                .unwrap_or_else(|_| "official".to_string()),
            fomo_api_base: env::var("FOMO_API_BASE").ok(),
            fomo_api_key: env::var("FOMO_API_KEY").ok(),
            fomo_auth_token: env::var("FOMO_AUTH_TOKEN").ok(),
            fomo_cf_cookie: env::var("FOMO_CF_COOKIE").ok(),
            fomo_window: env::var("FOMO_WINDOW").unwrap_or_else(|_| "24h".to_string()),
            fomo_clan_id: env::var("FOMO_CLAN_ID").ok(),
            fomo_trader_handle: env::var("FOMO_TRADER_HANDLE").ok(),
            fomo_trader_id: env::var("FOMO_TRADER_ID").ok(),
            fomo_clan_members_path: env::var("FOMO_CLAN_MEMBERS_PATH")
                .unwrap_or_else(|_| "/v2/clans/{clan_id}/members".to_string()),
            fomo_refresh_secs: env::var("FOMO_REFRESH_SECS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(86_400),
            fomo_poll_secs: env::var("FOMO_POLL_SECS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(60),
            fomo_copy_size_sol: env::var("FOMO_COPY_SIZE_SOL")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(0.05),
            fomo_size_mode: env::var("FOMO_SIZE_MODE").unwrap_or_else(|_| "fixed".to_string()),
            fomo_copy_ratio: env::var("FOMO_COPY_RATIO")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(0.02),
            fomo_mirror_sells: env::var("FOMO_MIRROR_SELLS")
                .map(|v| v.to_lowercase() != "false")
                .unwrap_or(true),
            fomo_min_swap_usd: env::var("FOMO_MIN_SWAP_USD")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(25.0),
            fomo_max_positions: env::var("FOMO_MAX_POSITIONS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(5),
            fomo_solana_network_ids: env::var("FOMO_SOLANA_NETWORK_IDS")
                .unwrap_or_else(|_| "1399811149,101,solana".to_string())
                .split(',')
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect(),

            // Copy Trade Configuration
            treasury_wallet: env::var("TREASURY_WALLET").ok(),
            copy_trade_fee_percent: env::var("COPY_TRADE_FEE_PERCENT")
                .unwrap_or_else(|_| "10.0".to_string())
                .parse()
                .unwrap_or(10.0),

            // Trading Configuration
            demo_mode: env::var("DEMO_MODE")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true), // Default to demo mode
            dry_run_mode: env::var("DRY_RUN_MODE")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false), // Default to false
            max_position_size_sol: env::var("MAX_POSITION_SIZE_SOL")
                .unwrap_or_else(|_| "0.01".to_string())
                .parse()
                .unwrap_or(0.01),
            total_budget_sol: env::var("TOTAL_BUDGET_SOL")
                .unwrap_or_else(|_| "0.1".to_string())
                .parse()
                .unwrap_or(0.1),
            default_stop_loss_percent: env::var("DEFAULT_STOP_LOSS_PERCENT")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .unwrap_or(10),
            default_take_profit_percent: env::var("DEFAULT_TAKE_PROFIT_PERCENT")
                .unwrap_or_else(|_| "50".to_string())
                .parse()
                .unwrap_or(50),
            default_trailing_stop_percent: env::var("DEFAULT_TRAILING_STOP_PERCENT")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .unwrap_or(5),
            max_hold_time_minutes: env::var("MAX_HOLD_TIME_MINUTES")
                .unwrap_or_else(|_| "240".to_string())
                .parse()
                .unwrap_or(240),

            // Risk Parameters
            min_liquidity_sol: env::var("MIN_LIQUIDITY_SOL")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .unwrap_or(10),
            max_risk_level: env::var("MAX_RISK_LEVEL")
                .unwrap_or_else(|_| "50".to_string())
                .parse()
                .unwrap_or(50),
            min_holders: env::var("MIN_HOLDERS")
                .unwrap_or_else(|_| "50".to_string())
                .parse()
                .unwrap_or(50),

            // Transaction Parameters
            default_slippage_bps: env::var("DEFAULT_SLIPPAGE_BPS")
                .unwrap_or_else(|_| "100".to_string())
                .parse()
                .context("Failed to parse DEFAULT_SLIPPAGE_BPS")?,
            default_priority_fee_micro_lamports: env::var("DEFAULT_PRIORITY_FEE_MICRO_LAMPORTS")
                .unwrap_or_else(|_| "50000".to_string())
                .parse()
                .context("Failed to parse DEFAULT_PRIORITY_FEE_MICRO_LAMPORTS")?,
        })
    }
}

/// Parse an optional positive float env var. Empty, unparseable, zero or
/// negative values all mean "rail disabled", which is safer than guessing.
fn parse_optional_f64(key: &str) -> Option<f64> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
}
