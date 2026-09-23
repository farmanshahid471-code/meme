//! FOMO leaderboard copy-trading.
//!
//! Daily flow (the thing the operator asked for):
//!
//! 1. **Find the top clan** — read fomo's clan leaderboard for the configured
//!    window (`24h` by default) and take rank 1. If the provider has no clan
//!    board, clans are derived from the traders' `clan` field.
//! 2. **Find that clan's top member** — read the clan's members and take rank 1
//!    by PnL.
//! 3. **Copy their trades** — poll that member's swaps and mirror the ones that
//!    touch a Solana mint: buys become entries, sells close our matching position.
//!
//! Both steps 1 and 2 can be pinned by hand (`FOMO_CLAN_ID`, `FOMO_TRADER_HANDLE`)
//! for when the operator already knows who they want to copy, or when the data
//! source is unreachable and they want the bot to keep running on a known target.
//!
//! Everything the engine decides is funnelled through [`decide`], a pure
//! function, so the interesting behaviour (dedupe, size caps, sell mirroring,
//! position limits, USD thresholds) is unit-testable without any network.
//!
//! ## Modes
//!
//! | Config | Behaviour |
//! |--------|-----------|
//! | `DEMO_MODE=true` | Mirror into simulated demo positions via `PositionManager`. |
//! | `DRY_RUN_MODE=true` | Mirror into `SimulationManager` (same as the scanners). |
//! | neither | Real Jupiter swaps through the shared `execute_buy_task` path. |
//!
//! In all three modes entries pass through the risk guard first, so the daily
//! loss cap, drawdown breaker and per-token cooldown apply to copied trades too.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

use crate::api::fomo::{FomoClan, FomoClient, FomoTrader, MirrorSignal, SwapSide};
use crate::api::jupiter::JupiterClient;
use crate::config::Config;
use crate::solana::wallet::WalletManager;
use crate::trading::position::{PositionManager, PositionStatus};
use crate::trading::risk_guard::RiskGuard;
use crate::trading::simulation::SimulationManager;
use crate::trading::strategy::Strategy;

const STATE_FILE: &str = "data/fomo_state.json";
/// How many processed swap ids we remember (dedupe window).
const SEEN_SWAP_CAP: usize = 500;
/// How many decisions we keep for the dashboard feed.
const DECISION_LOG_CAP: usize = 50;

/// Sizing model for a mirrored entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeMode {
    /// Always spend `FOMO_COPY_SIZE_SOL`.
    Fixed,
    /// Spend `FOMO_COPY_RATIO` of the copied trade's USD notional, converted to
    /// SOL and clamped to the strategy's max position size.
    Proportional,
}

impl SizeMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "proportional" | "ratio" | "scale" => SizeMode::Proportional,
            _ => SizeMode::Fixed,
        }
    }
}

/// Everything the copy engine knows at decision time.
#[derive(Debug, Clone)]
pub struct MirrorContext {
    pub size_mode: SizeMode,
    pub copy_size_sol: f64,
    pub copy_ratio: f64,
    /// USD price of SOL, used to convert a notional into SOL. 0 = unknown.
    pub sol_price_usd: f64,
    pub max_position_size_sol: f64,
    pub min_swap_usd: f64,
    pub mirror_sells: bool,
    pub max_positions: usize,
    pub open_positions: usize,
    pub already_mirrored: bool,
    pub open_position_for_token: bool,
    /// Has this swap id already been processed?
    pub swap_seen: bool,
}

/// What to do about one swap.
#[derive(Debug, Clone, PartialEq)]
pub enum MirrorDecision {
    /// Open (or add to) a position in this mint.
    Buy { token_address: String, size_sol: f64, reason: String },
    /// Close our position in this mint because the copied trader sold.
    Sell { token_address: String, reason: String },
    /// Do nothing, and say why (surfaced in the dashboard feed).
    Skip { token_address: String, reason: String },
}

/// Pure decision function. Order matters: cheap/explanatory refusals first.
pub fn decide(signal: &MirrorSignal, ctx: &MirrorContext) -> MirrorDecision {
    let token = signal.token_address.clone();

    if ctx.swap_seen {
        return MirrorDecision::Skip {
            token_address: token,
            reason: "already processed this swap".to_string(),
        };
    }

    if ctx.min_swap_usd > 0.0 && signal.usd_value < ctx.min_swap_usd {
        return MirrorDecision::Skip {
            token_address: token,
            reason: format!(
                "swap is ${:.2}, below FOMO_MIN_SWAP_USD ${:.2}",
                signal.usd_value, ctx.min_swap_usd
            ),
        };
    }

    match signal.side {
        SwapSide::Sell => {
            if !ctx.mirror_sells {
                return MirrorDecision::Skip {
                    token_address: token,
                    reason: "FOMO_MIRROR_SELLS is off".to_string(),
                };
            }
            if !ctx.open_position_for_token {
                return MirrorDecision::Skip {
                    token_address: token,
                    reason: "copied trader sold, but we hold nothing in this token".to_string(),
                };
            }
            MirrorDecision::Sell {
                token_address: token,
                reason: format!("copied trader sold {}", signal.describe()),
            }
        }
        SwapSide::Buy => {
            if ctx.already_mirrored {
                return MirrorDecision::Skip {
                    token_address: token,
                    reason: "already hold this token (no pyramiding)".to_string(),
                };
            }
            if ctx.max_positions > 0 && ctx.open_positions >= ctx.max_positions {
                return MirrorDecision::Skip {
                    token_address: token,
                    reason: format!(
                        "{} open copied positions (FOMO_MAX_POSITIONS {})",
                        ctx.open_positions, ctx.max_positions
                    ),
                };
            }

            let size = match ctx.size_mode {
                SizeMode::Fixed => ctx.copy_size_sol,
                SizeMode::Proportional => {
                    if ctx.sol_price_usd <= 0.0 {
                        // Without a SOL price we cannot scale honestly; fall back.
                        ctx.copy_size_sol
                    } else {
                        (signal.usd_value / ctx.sol_price_usd) * ctx.copy_ratio
                    }
                }
            };
            let size = size.min(ctx.max_position_size_sol).max(0.0);

            if size <= 0.0 {
                return MirrorDecision::Skip {
                    token_address: token,
                    reason: "computed copy size is zero".to_string(),
                };
            }

            MirrorDecision::Buy {
                token_address: token,
                size_sol: size,
                reason: format!("copied buy {}", signal.describe()),
            }
        }
    }
}

/// Persisted engine state so a restart does not re-mirror old trades.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FomoState {
    #[serde(default)]
    pub target_clan: Option<FomoClan>,
    #[serde(default)]
    pub target_trader: Option<FomoTrader>,
    #[serde(default)]
    pub last_discovery: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_poll: Option<DateTime<Utc>>,
    #[serde(default)]
    pub seen_swap_ids: Vec<String>,
    #[serde(default)]
    pub mirrored: u64,
    #[serde(default)]
    pub skipped: u64,
    #[serde(default)]
    pub failed: u64,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Manual overrides from the API — survive a restart.
    #[serde(default)]
    pub pinned_clan_id: Option<String>,
    #[serde(default)]
    pub pinned_trader: Option<String>,
}

/// One line of the dashboard feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FomoDecisionLog {
    pub at: DateTime<Utc>,
    pub action: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub detail: String,
}

/// Aggregated counters shared with the API/dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct FomoStats {
    pub provider: String,
    pub base_url: String,
    pub enabled: bool,
    pub mode: String,
    pub window: String,
    pub target_clan: Option<FomoClan>,
    pub target_trader: Option<FomoTrader>,
    pub last_discovery: Option<DateTime<Utc>>,
    pub last_poll: Option<DateTime<Utc>>,
    pub mirrored: u64,
    pub skipped: u64,
    pub failed: u64,
    pub last_error: Option<String>,
    pub recent: Vec<FomoDecisionLog>,
}

/// The engine. Owns discovery + mirroring loops for one active strategy.
pub struct FomoCopyEngine {
    config: Arc<Config>,
    client: Arc<FomoClient>,
    position_manager: Arc<PositionManager>,
    jupiter: Arc<JupiterClient>,
    wallet: Arc<WalletManager>,
    risk_guard: Arc<RiskGuard>,
    simulation_manager: Option<Arc<SimulationManager>>,
    strategy: Strategy,
    state: RwLock<FomoState>,
    decisions: RwLock<VecDeque<FomoDecisionLog>>,
    running: RwLock<bool>,
}

impl FomoCopyEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<Config>,
        client: Arc<FomoClient>,
        position_manager: Arc<PositionManager>,
        jupiter: Arc<JupiterClient>,
        wallet: Arc<WalletManager>,
        risk_guard: Arc<RiskGuard>,
        simulation_manager: Option<Arc<SimulationManager>>,
        strategy: Strategy,
    ) -> Self {
        Self {
            config,
            client,
            position_manager,
            jupiter,
            wallet,
            risk_guard,
            simulation_manager,
            strategy,
            state: RwLock::new(FomoState::default()),
            decisions: RwLock::new(VecDeque::new()),
            running: RwLock::new(false),
        }
    }

    /// Human-readable mode string for logs and the dashboard.
    fn mode(&self) -> &'static str {
        if self.config.demo_mode {
            "demo"
        } else if self.config.dry_run_mode {
            "dry_run"
        } else {
            "live"
        }
    }

    fn size_mode(&self) -> SizeMode {
        SizeMode::parse(&self.config.fomo_size_mode)
    }

    /// Load persisted state (target + processed swaps).
    pub async fn load_state(&self) -> Result<()> {
        match tokio::fs::read_to_string(STATE_FILE).await {
            Ok(raw) => match serde_json::from_str::<FomoState>(&raw) {
                Ok(parsed) => {
                    info!(
                        "Loaded FOMO copy state: target={:?} seen_swaps={} mirrored={}",
                        parsed
                            .target_trader
                            .as_ref()
                            .map(|t| t.handle.clone())
                            .or_else(|| parsed.target_clan.as_ref().map(|c| c.name.clone())),
                        parsed.seen_swap_ids.len(),
                        parsed.mirrored
                    );
                    *self.state.write().await = parsed;
                }
                Err(e) => warn!("Could not parse {}: {} — starting fresh", STATE_FILE, e),
            },
            Err(_) => debug!("No FOMO copy state file yet ({}), starting fresh", STATE_FILE),
        }
        Ok(())
    }

    async fn save_state(&self) -> Result<()> {
        let snapshot = self.state.read().await.clone();
        if let Some(parent) = std::path::Path::new(STATE_FILE).parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let json = serde_json::to_string_pretty(&snapshot)?;
        tokio::fs::write(STATE_FILE, json)
            .await
            .context("Failed to persist FOMO copy state")?;
        Ok(())
    }

    /// Non-blocking friendly variant used from async code paths.
    async fn record_decision(&self, log: FomoDecisionLog) {
        let mut decisions = self.decisions.write().await;
        decisions.push_front(log);
        decisions.truncate(DECISION_LOG_CAP);
    }

    /// Pin a specific clan/trader (or clear the pins with `None`).
    pub async fn set_pins(&self, clan_id: Option<String>, trader: Option<String>) -> Result<()> {
        {
            let mut state = self.state.write().await;
            state.pinned_clan_id = clan_id;
            state.pinned_trader = trader;
            // Force rediscovery on the next tick.
            state.last_discovery = None;
        }
        self.save_state().await
    }

    /// Snapshot for `GET /api/fomo/status`.
    pub async fn stats(&self) -> FomoStats {
        let state = self.state.read().await;
        let recent = self.decisions.read().await.iter().cloned().collect();
        FomoStats {
            provider: self.client.provider().as_str().to_string(),
            base_url: self.client.config().base_url.clone(),
            enabled: self.client.config().is_configured(),
            mode: self.mode().to_string(),
            window: self.config.fomo_window.clone(),
            target_clan: state.target_clan.clone(),
            target_trader: state.target_trader.clone(),
            last_discovery: state.last_discovery,
            last_poll: state.last_poll,
            mirrored: state.mirrored,
            skipped: state.skipped,
            failed: state.failed,
            last_error: state.last_error.clone(),
            recent,
        }
    }

    /// Discovery step: top clan, then its top member.
    pub async fn refresh_target(&self) -> Result<()> {
        let pins = {
            let state = self.state.read().await;
            (state.pinned_clan_id.clone(), state.pinned_trader.clone())
        };

        // --- A pinned trader short-circuits everything -------------------------
        if let Some(handle) = pins.1 {
            info!("🎯 FOMO copy: resolving pinned trader '{}'", handle);
            match self.client.trader_profile(&handle).await {
                Ok(trader) => {
                    let mut state = self.state.write().await;
                    state.target_trader = Some(trader.clone());
                    state.target_clan = None;
                    state.last_discovery = Some(Utc::now());
                    drop(state);
                    self.save_state().await?;
                    info!(
                        "🎯 FOMO copy target: @{} (${:.0} {})",
                        trader.handle, trader.pnl_usd, self.config.fomo_window
                    );
                    return Ok(());
                }
                Err(e) => {
                    warn!("FOMO copy: could not resolve pinned trader '{}': {e}", handle);
                    return Err(anyhow!(e.to_string()));
                }
            }
        }

        // --- 1. Top clan ------------------------------------------------------
        let clan = match &pins.0 {
            Some(clan_id) => {
                info!("🎯 FOMO copy: clan pinned to {}", clan_id);
                // We only have the id; fetch members directly and name it later.
                FomoClan {
                    id: clan_id.clone(),
                    name: clan_id.clone(),
                    pnl_usd: 0.0,
                    member_count: None,
                    icon: None,
                }
            }
            None => {
                let clans = self
                    .client
                    .clan_leaderboard(50)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))
                    .context("FOMO copy: clan leaderboard lookup failed")?;

                match clans.into_iter().next() {
                    Some(clan) => {
                        info!(
                            "🏆 FOMO copy: top clan for {} is '{}' (${:.0})",
                            self.config.fomo_window, clan.name, clan.pnl_usd
                        );
                        clan
                    }
                    None => {
                        warn!("FOMO copy: clan leaderboard was empty — falling back to the top trader");
                        let traders = self
                            .client
                            .trader_leaderboard(10)
                            .await
                            .map_err(|e| anyhow!(e.to_string()))?;
                        let trader = traders
                            .into_iter()
                            .next()
                            .ok_or_else(|| anyhow!("FOMO copy: trader leaderboard was empty too"))?;
                        let mut state = self.state.write().await;
                        state.target_clan = None;
                        state.target_trader = Some(trader.clone());
                        state.last_discovery = Some(Utc::now());
                        drop(state);
                        self.save_state().await?;
                        info!(
                            "🎯 FOMO copy target (no clans available): @{} (${:.0})",
                            trader.handle, trader.pnl_usd
                        );
                        return Ok(());
                    }
                }
            }
        };

        // --- 2. Top member of that clan --------------------------------------
        let members = self
            .client
            .clan_members(&clan.id, 100)
            .await
            .map_err(|e| anyhow!(e.to_string()))
            .context("FOMO copy: clan member lookup failed")?;

        let mut members = members;
        members.sort_by(|a, b| b.pnl_usd.total_cmp(&a.pnl_usd));
        let top = members.into_iter().next().ok_or_else(|| {
            anyhow!(
                "FOMO copy: clan '{}' returned no members (try FOMO_TRADER_HANDLE to pin a trader)",
                clan.name
            )
        })?;

        info!(
            "👑 FOMO copy: top member of '{}' is @{} (${:.0} {}) — copying their swaps",
            clan.name, top.handle, top.pnl_usd, self.config.fomo_window
        );

        let mut state = self.state.write().await;
        state.target_clan = Some(clan);
        state.target_trader = Some(top);
        state.last_discovery = Some(Utc::now());
        state.last_error = None;
        drop(state);
        self.save_state().await?;
        Ok(())
    }

    /// Mirror step: poll the target's swaps and act on new ones.
    pub async fn poll_once(&self) -> Result<usize> {
        let trader = {
            let state = self.state.read().await;
            state.target_trader.clone()
        };
        let Some(trader) = trader else {
            return Err(anyhow!("FOMO copy: no target trader resolved yet"));
        };

        let swaps = self
            .client
            .swaps(&trader.user_id, 50)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut handled = 0usize;
        for swap in swaps {
            let Some(signal) = self.client.to_mirror_signal(&swap) else {
                continue; // not a Solana buy/sell
            };
            if self.apply_signal(&signal, &trader).await? {
                handled += 1;
            }
        }

        {
            let mut state = self.state.write().await;
            state.last_poll = Some(Utc::now());
            state.last_error = None;
        }
        self.save_state().await?;
        Ok(handled)
    }

    /// Evaluate one signal and either act on it or record why not.
    /// Returns true when a trade was actually taken/closed.
    async fn apply_signal(&self, signal: &MirrorSignal, trader: &FomoTrader) -> Result<bool> {
        let (swap_seen, open_for_token, open_count) = {
            let state = self.state.read().await;
            (
                state.seen_swap_ids.iter().any(|id| id == &signal.swap_id),
                self.position_manager.has_active_position(&signal.token_address).await,
                self.position_manager.get_active_positions().await.len(),
            )
        };

        let ctx = MirrorContext {
            size_mode: self.size_mode(),
            copy_size_sol: self.config.fomo_copy_size_sol,
            copy_ratio: self.config.fomo_copy_ratio,
            sol_price_usd: self.sol_price_usd().await,
            max_position_size_sol: self.strategy.max_position_size_sol,
            min_swap_usd: self.config.fomo_min_swap_usd,
            mirror_sells: self.config.fomo_mirror_sells,
            max_positions: self.config.fomo_max_positions as usize,
            open_positions: open_count,
            already_mirrored: open_for_token,
            open_position_for_token: open_for_token,
            swap_seen,
        };

        let decision = decide(signal, &ctx);

        // Mark the swap processed before acting: a failure must not cause us to
        // fire the same trade again on the next poll.
        {
            let mut state = self.state.write().await;
            if !state.seen_swap_ids.iter().any(|id| id == &signal.swap_id) {
                state.seen_swap_ids.push(signal.swap_id.clone());
                if state.seen_swap_ids.len() > SEEN_SWAP_CAP {
                    let excess = state.seen_swap_ids.len() - SEEN_SWAP_CAP;
                    state.seen_swap_ids.drain(0..excess);
                }
            }
        }

        match decision {
            MirrorDecision::Skip { token_address, reason } => {
                if !ctx.swap_seen {
                    debug!(
                        "FOMO copy: skipping {} ({}) — {}",
                        signal.describe(),
                        short(&token_address),
                        reason
                    );
                    self.state.write().await.skipped += 1;
                    self.record_decision(FomoDecisionLog {
                        at: Utc::now(),
                        action: "skip".to_string(),
                        token_address,
                        token_symbol: signal.token_symbol.clone(),
                        detail: reason,
                    })
                    .await;
                }
                Ok(false)
            }
            MirrorDecision::Buy { token_address, size_sol, reason } => {
                info!(
                    "📈 FOMO copy: {} for {:.4} SOL (copying @{}, {})",
                    signal.describe(),
                    size_sol,
                    trader.handle,
                    reason
                );
                match self.open_mirrored_position(signal, &token_address, size_sol, trader).await {
                    Ok(()) => {
                        self.state.write().await.mirrored += 1;
                        self.record_decision(FomoDecisionLog {
                            at: Utc::now(),
                            action: "buy".to_string(),
                            token_address,
                            token_symbol: signal.token_symbol.clone(),
                            detail: format!("{:.4} SOL — {reason}", size_sol),
                        })
                        .await;
                        Ok(true)
                    }
                    Err(e) => {
                        warn!("FOMO copy: mirrored buy failed: {:?}", e);
                        self.state.write().await.failed += 1;
                        self.record_decision(FomoDecisionLog {
                            at: Utc::now(),
                            action: "failed".to_string(),
                            token_address,
                            token_symbol: signal.token_symbol.clone(),
                            detail: e.to_string(),
                        })
                        .await;
                        Ok(false)
                    }
                }
            }
            MirrorDecision::Sell { token_address, reason } => {
                info!("📉 FOMO copy: closing our {} position ({})", short(&token_address), reason);
                match self
                    .position_manager
                    .close_positions_by_token(&token_address, PositionStatus::ManualClose)
                    .await
                {
                    Ok(closed) => {
                        self.state.write().await.mirrored += 1;
                        self.record_decision(FomoDecisionLog {
                            at: Utc::now(),
                            action: "sell".to_string(),
                            token_address,
                            token_symbol: signal.token_symbol.clone(),
                            detail: format!("closed {closed} position(s) — {reason}"),
                        })
                        .await;
                        Ok(closed > 0)
                    }
                    Err(e) => {
                        warn!("FOMO copy: mirrored sell failed: {:?}", e);
                        self.state.write().await.failed += 1;
                        self.record_decision(FomoDecisionLog {
                            at: Utc::now(),
                            action: "failed".to_string(),
                            token_address,
                            token_symbol: signal.token_symbol.clone(),
                            detail: e.to_string(),
                        })
                        .await;
                        Ok(false)
                    }
                }
            }
        }
    }

    /// Open a copied position in whichever mode the bot is running in.
    async fn open_mirrored_position(
        &self,
        signal: &MirrorSignal,
        token_address: &str,
        size_sol: f64,
        trader: &FomoTrader,
    ) -> Result<()> {
        // The risk guard gates copied entries exactly like scanned ones.
        if let Err(reason) = self.risk_guard.check_entry(token_address).await {
            return Err(anyhow!("risk guard refused copied entry: {reason}"));
        }

        let symbol = signal
            .token_symbol
            .clone()
            .unwrap_or_else(|| "COPY".to_string());
        let name = format!("FOMO copy of @{}", trader.handle);
        let reason = format!(
            "fomo_copy: top member @{} of clan {} ({})",
            trader.handle,
            self.strategy.name,
            signal.describe()
        );

        // --- Mode 1: demo positions -------------------------------------------
        if self.config.demo_mode {
            self.position_manager
                .create_demo_position(token_address, &name, &symbol, &self.strategy.id, size_sol)
                .await?;
            info!("🧪 [DEMO] Mirrored {} into a demo position", signal.describe());
            return Ok(());
        }

        // --- Mode 2: dry run / simulation -------------------------------------
        if let Some(simulation) = &self.simulation_manager {
            let price = self
                .reference_price_sol(token_address)
                .await
                .unwrap_or(0.0);
            if price <= 0.0 {
                return Err(anyhow!(
                    "dry run: no reference price available for {} yet",
                    token_address
                ));
            }
            simulation
                .simulate_buy(
                    token_address,
                    &symbol,
                    &name,
                    price,
                    size_sol,
                    0, // risk score: scoring is the scanner's job, not the copier's
                    vec!["fomo leaderboard copy".to_string()],
                    reason,
                    self.strategy.id.clone(),
                )
                .await?;
            info!("🔍 [DRY RUN] Simulated mirror of {}", signal.describe());
            return Ok(());
        }

        // --- Mode 3: live swap through the shared execution path ---------------
        let metadata = crate::models::token::TokenMetadata {
            address: token_address.to_string(),
            name,
            symbol,
            decimals: 9, // pump.fun assumption, same as the Telegram sniper path
            supply: None,
            logo_uri: None,
            creation_time: Some(Utc::now()),
        };

        crate::trading::autotrader::execute_buy_task(
            &metadata,
            &self.strategy,
            &self.position_manager,
            &self.jupiter,
            &self.wallet,
            &self.config,
            None,
        )
        .await?;
        Ok(())
    }

    /// Best-effort SOL price in USD, used by proportional sizing.
    async fn sol_price_usd(&self) -> f64 {
        let url = "https://api.dexscreener.com/latest/dex/tokens/So11111111111111111111111111111111111111112";
        match reqwest::get(url).await {
            Ok(response) => match response.json::<serde_json::Value>().await {
                Ok(json) => json
                    .get("pairs")
                    .and_then(|pairs| pairs.as_array())
                    .and_then(|pairs| pairs.first())
                    .and_then(|pair| pair.get("priceUsd"))
                    .and_then(|price| price.as_str())
                    .and_then(|price| price.parse::<f64>().ok())
                    .unwrap_or(0.0),
                Err(_) => 0.0,
            },
            Err(_) => 0.0,
        }
    }

    /// Reference price for dry-run bookkeeping only.
    async fn reference_price_sol(&self, mint: &str) -> Option<f64> {
        let url = format!("https://api.dexscreener.com/latest/dex/tokens/{mint}");
        let response = reqwest::get(&url).await.ok()?;
        let json: serde_json::Value = response.json().await.ok()?;
        let pair = json.get("pairs")?.as_array()?.first()?;
        let price_usd = pair.get("priceUsd")?.as_str()?.parse::<f64>().ok()?;
        let sol_price = self.sol_price_usd().await;
        if sol_price <= 0.0 {
            return None;
        }
        Some(price_usd / sol_price)
    }

    /// Run the discovery + mirror loops until [`Self::stop`] is called.
    pub async fn run(self: Arc<Self>) {
        if !self.client.config().is_configured() {
            warn!("FOMO copy: client is not configured (set FOMO_API_BASE/FOMO_API_KEY) — engine idle");
            return;
        }

        {
            let mut running = self.running.write().await;
            if *running {
                warn!("FOMO copy engine already running");
                return;
            }
            *running = true;
        }

        info!(
            "🦍 FOMO copy engine started — provider={} window={} mode={} refresh={}s poll={}s",
            self.client.provider().as_str(),
            self.config.fomo_window,
            self.mode(),
            self.config.fomo_refresh_secs,
            self.config.fomo_poll_secs
        );

        let mut poll = interval(Duration::from_secs(self.config.fomo_poll_secs.max(10)));
        loop {
            if !*self.running.read().await {
                break;
            }
            poll.tick().await;

            // --- Daily (configurable) discovery -------------------------------
            let needs_discovery = {
                let state = self.state.read().await;
                match state.last_discovery {
                    None => true,
                    Some(last) => {
                        Utc::now().signed_duration_since(last)
                            >= chrono::Duration::seconds(self.config.fomo_refresh_secs as i64)
                    }
                }
            };

            if needs_discovery {
                if let Err(e) = self.refresh_target().await {
                    error!("FOMO copy: discovery failed: {:?}", e);
                    let mut state = self.state.write().await;
                    state.last_error = Some(e.to_string());
                    let has_target = state.target_trader.is_some();
                    drop(state);
                    self.save_state().await.ok();
                    if !has_target {
                        // Nothing to copy yet — try again next tick.
                        continue;
                    }
                    warn!("FOMO copy: keeping the previous target while discovery is down");
                }
            }

            if let Err(e) = self.poll_once().await {
                debug!("FOMO copy poll error: {:?}", e);
                let mut state = self.state.write().await;
                state.last_error = Some(e.to_string());
                drop(state);
                self.save_state().await.ok();
            }
        }

        info!("FOMO copy engine stopped");
    }

    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }

    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}

fn short(address: &str) -> String {
    if address.len() <= 10 {
        address.to_string()
    } else {
        format!("{}…{}", &address[..4], &address[address.len() - 4..])
    }
}

// ============================================================================
// Tests — the decision table is the interesting part, so it gets covered
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(side: SwapSide, usd: f64, swap_id: &str) -> MirrorSignal {
        MirrorSignal {
            side,
            token_address: "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump".to_string(),
            token_symbol: Some("CATGPT".to_string()),
            usd_value: usd,
            swap_id: swap_id.to_string(),
            at: Some(Utc::now()),
        }
    }

    fn ctx() -> MirrorContext {
        MirrorContext {
            size_mode: SizeMode::Fixed,
            copy_size_sol: 0.05,
            copy_ratio: 0.02,
            sol_price_usd: 200.0,
            max_position_size_sol: 0.2,
            min_swap_usd: 25.0,
            mirror_sells: true,
            max_positions: 5,
            open_positions: 0,
            already_mirrored: false,
            open_position_for_token: false,
            swap_seen: false,
        }
    }

    #[test]
    fn mirrors_a_buy_at_the_configured_size() {
        let decision = decide(&signal(SwapSide::Buy, 99.05, "s1"), &ctx());
        match decision {
            MirrorDecision::Buy { size_sol, .. } => assert!((size_sol - 0.05).abs() < 1e-9),
            other => panic!("expected buy, got {other:?}"),
        }
    }

    #[test]
    fn proportional_sizing_scales_with_the_copied_notional_and_caps_at_strategy_max() {
        let mut context = ctx();
        context.size_mode = SizeMode::Proportional;
        context.copy_ratio = 0.5; // half their notional
        context.max_position_size_sol = 0.2;

        // $100 at $200/SOL = 0.5 SOL notional → half of that = 0.25 → capped to 0.2
        match decide(&signal(SwapSide::Buy, 100.0, "s1"), &context) {
            MirrorDecision::Buy { size_sol, .. } => assert!((size_sol - 0.2).abs() < 1e-9),
            other => panic!("expected buy, got {other:?}"),
        }

        // $10 notional → 0.05 SOL → half = 0.025, under the cap
        match decide(&signal(SwapSide::Buy, 10.0, "s2"), &context) {
            MirrorDecision::Buy { size_sol, .. } => assert!((size_sol - 0.025).abs() < 1e-9),
            other => panic!("expected buy, got {other:?}"),
        }
    }

    #[test]
    fn proportional_sizing_falls_back_when_sol_price_is_unknown() {
        let mut context = ctx();
        context.size_mode = SizeMode::Proportional;
        context.sol_price_usd = 0.0;
        match decide(&signal(SwapSide::Buy, 100.0, "s1"), &context) {
            MirrorDecision::Buy { size_sol, .. } => assert!((size_sol - 0.05).abs() < 1e-9),
            other => panic!("expected buy, got {other:?}"),
        }
    }

    #[test]
    fn ignores_dust_swaps() {
        match decide(&signal(SwapSide::Buy, 9.75, "s1"), &ctx()) {
            MirrorDecision::Skip { reason, .. } => assert!(reason.contains("FOMO_MIN_SWAP_USD")),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn never_processes_the_same_swap_twice() {
        let mut context = ctx();
        context.swap_seen = true;
        match decide(&signal(SwapSide::Buy, 99.0, "s1"), &context) {
            MirrorDecision::Skip { reason, .. } => assert!(reason.contains("already processed")),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn does_not_pyramid_into_a_token_we_already_hold() {
        let mut context = ctx();
        context.already_mirrored = true;
        match decide(&signal(SwapSide::Buy, 99.0, "s1"), &context) {
            MirrorDecision::Skip { reason, .. } => assert!(reason.contains("already hold")),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn respects_the_open_position_cap() {
        let mut context = ctx();
        context.open_positions = 5;
        match decide(&signal(SwapSide::Buy, 99.0, "s1"), &context) {
            MirrorDecision::Skip { reason, .. } => assert!(reason.contains("FOMO_MAX_POSITIONS")),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn mirrors_sells_when_we_hold_the_token() {
        let mut context = ctx();
        context.open_position_for_token = true;
        match decide(&signal(SwapSide::Sell, 74.5, "s2"), &context) {
            MirrorDecision::Sell { reason, .. } => assert!(reason.contains("copied trader sold")),
            other => panic!("expected sell, got {other:?}"),
        }
    }

    #[test]
    fn sell_mirroring_can_be_switched_off() {
        let mut context = ctx();
        context.open_position_for_token = true;
        context.mirror_sells = false;
        match decide(&signal(SwapSide::Sell, 74.5, "s2"), &context) {
            MirrorDecision::Skip { reason, .. } => assert!(reason.contains("FOMO_MIRROR_SELLS")),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn sell_of_a_token_we_do_not_hold_is_a_no_op() {
        match decide(&signal(SwapSide::Sell, 74.5, "s2"), &ctx()) {
            MirrorDecision::Skip { reason, .. } => assert!(reason.contains("hold nothing")),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn size_mode_aliases_parse() {
        assert_eq!(SizeMode::parse("fixed"), SizeMode::Fixed);
        assert_eq!(SizeMode::parse("FIXED"), SizeMode::Fixed);
        assert_eq!(SizeMode::parse("proportional"), SizeMode::Proportional);
        assert_eq!(SizeMode::parse("ratio"), SizeMode::Proportional);
    }
}
