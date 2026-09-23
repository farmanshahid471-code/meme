//! Portfolio-level safety rails.
//!
//! The guard is the single place that answers *"is this bot allowed to open a
//! new position right now?"* and *"has the day gone badly enough that we should
//! stop?"*.  Everything here is deliberately independent from Solana/Jupiter so
//! the decision logic can be unit-tested without a network.
//!
//! Rails provided:
//!
//! | Rail | Env var | Behaviour |
//! |------|---------|-----------|
//! | Daily loss limit | `DAILY_LOSS_LIMIT_SOL` | Stop opening positions for the rest of the UTC day once realised PnL ≤ −limit |
//! | Max drawdown | `MAX_DRAWDOWN_PERCENT` | Stop once realised equity drops this % below its peak (sticky until reset or recovery) |
//! | Max trades/day | `MAX_TRADES_PER_DAY` | Hard cap on new entries per UTC day |
//! | Consecutive losses | `MAX_CONSECUTIVE_LOSSES` | Pause after N losing positions in a row (re-arms next day) |
//! | Token cooldown | `TOKEN_COOLDOWN_MINUTES` | No re-entry into a mint for N minutes after its last exit |
//! | Kill switch | `POST /api/emergency/stop` | `HaltKind::Manual` — only a human clears it |
//!
//! Exits (stop loss / take profit / trailing stop / max hold) are **never**
//! blocked by the guard — it only gates entries. A halted bot still manages and
//! closes whatever it already holds.

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::config::Config;

/// Why the guard stopped accepting new entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaltKind {
    /// Operator pressed the kill switch.
    Manual,
    /// Realised PnL for the UTC day hit the configured loss limit.
    DailyLoss,
    /// Realised equity fell too far below its peak.
    Drawdown,
    /// Too many losing positions in a row.
    ConsecutiveLosses,
    /// New-entry quota for the day is used up.
    MaxTradesPerDay,
}

impl HaltKind {
    /// Does the rail re-arm automatically when a new UTC day starts?
    pub fn clears_on_new_day(self) -> bool {
        matches!(
            self,
            HaltKind::DailyLoss | HaltKind::ConsecutiveLosses | HaltKind::MaxTradesPerDay
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HaltKind::Manual => "manual",
            HaltKind::DailyLoss => "daily_loss",
            HaltKind::Drawdown => "drawdown",
            HaltKind::ConsecutiveLosses => "consecutive_losses",
            HaltKind::MaxTradesPerDay => "max_trades_per_day",
        }
    }
}

/// A currently active halt, with the human-readable reason for the UI/logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveHalt {
    pub kind: HaltKind,
    pub reason: String,
    pub since: DateTime<Utc>,
}

/// Immutable thresholds, loaded from the environment at boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLimits {
    /// Halt when realised PnL for the UTC day ≤ −value. `None` = disabled.
    pub daily_loss_limit_sol: Option<f64>,
    /// Halt when realised equity is this % below its peak. `None` = disabled.
    pub max_drawdown_percent: Option<f64>,
    /// Maximum number of new entries per UTC day. `None` = unlimited.
    pub max_trades_per_day: Option<u32>,
    /// Pause after this many consecutive losing positions. `None` = disabled.
    pub max_consecutive_losses: Option<u32>,
    /// Minimum minutes between an exit and the next entry on the same mint. 0 = off.
    pub token_cooldown_minutes: u32,
    /// Equity baseline used for the drawdown calculation (per-strategy budget).
    pub starting_equity_sol: f64,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            daily_loss_limit_sol: None,
            max_drawdown_percent: None,
            max_trades_per_day: None,
            max_consecutive_losses: None,
            token_cooldown_minutes: 0,
            starting_equity_sol: 0.0,
        }
    }
}

impl RiskLimits {
    /// Read the rails out of the loaded [`Config`].
    pub fn from_config(config: &Config) -> Self {
        Self {
            daily_loss_limit_sol: config.daily_loss_limit_sol,
            max_drawdown_percent: config.max_drawdown_percent,
            max_trades_per_day: config.max_trades_per_day,
            max_consecutive_losses: config.max_consecutive_losses,
            token_cooldown_minutes: config.token_cooldown_minutes,
            starting_equity_sol: config.total_budget_sol,
        }
    }
}

/// Everything the guard remembers. Serialised straight into the API response.
#[derive(Debug, Clone, Serialize)]
pub struct RiskGuardState {
    /// The UTC day the daily counters belong to.
    pub day: NaiveDate,
    /// Realised PnL since the last UTC day rollover.
    pub realized_pnl_today_sol: f64,
    /// Realised PnL since the process started (rebuilt from closed positions).
    pub realized_pnl_total_sol: f64,
    /// New entries taken today.
    pub trades_today: u32,
    /// New entries taken since start.
    pub trades_total: u64,
    pub wins: u64,
    pub losses: u64,
    /// Losing trades in a row, reset by any winner.
    pub consecutive_losses: u32,
    /// Highest realised equity seen (used by the drawdown rail).
    pub peak_equity_sol: f64,
    /// Current realised equity (`starting_equity_sol` + total realised PnL).
    pub equity_sol: f64,
    /// Set when entries are blocked.
    pub halt: Option<ActiveHalt>,
    /// Last exit time per mint, used by the cooldown rail.
    pub last_exit_by_token: HashMap<String, DateTime<Utc>>,
    /// How many entries the guard refused (visibility into the rails firing).
    pub entry_refusals: u64,
}

impl RiskGuardState {
    pub fn new(starting_equity_sol: f64) -> Self {
        Self {
            day: Utc::now().date_naive(),
            realized_pnl_today_sol: 0.0,
            realized_pnl_total_sol: 0.0,
            trades_today: 0,
            trades_total: 0,
            wins: 0,
            losses: 0,
            consecutive_losses: 0,
            peak_equity_sol: starting_equity_sol,
            equity_sol: starting_equity_sol,
            halt: None,
            last_exit_by_token: HashMap::new(),
            entry_refusals: 0,
        }
    }
}

fn make_halt(kind: HaltKind, reason: String, now: DateTime<Utc>) -> ActiveHalt {
    ActiveHalt { kind, reason, since: now }
}

/// Reset the per-day counters when the UTC day changes.
///
/// Halts that are inherently daily (`daily_loss`, `consecutive_losses`,
/// `max_trades_per_day`) clear here; a drawdown breaker or a manual kill switch
/// survives until someone resets it (or equity recovers, see [`apply_exit`]).
fn roll_day(state: &mut RiskGuardState, today: NaiveDate) {
    if state.day == today {
        return;
    }
    state.day = today;
    state.realized_pnl_today_sol = 0.0;
    state.trades_today = 0;
    if let Some(halt) = &state.halt {
        if halt.kind.clears_on_new_day() {
            state.halt = None;
        }
    }
}

/// Decide whether a new entry is allowed. Pure — safe to unit test.
pub fn evaluate_entry(
    limits: &RiskLimits,
    state: &RiskGuardState,
    token_address: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    if let Some(halt) = &state.halt {
        return Err(format!(
            "trading halted by {} rail: {}",
            halt.kind.as_str(),
            halt.reason
        ));
    }

    if let Some(limit) = limits.daily_loss_limit_sol {
        let limit = limit.abs();
        if limit > 0.0 && state.realized_pnl_today_sol <= -limit {
            return Err(format!(
                "daily loss limit reached ({:.4} SOL of -{:.4} SOL)",
                state.realized_pnl_today_sol, limit
            ));
        }
    }

    if let Some(limit) = limits.max_trades_per_day {
        if state.trades_today >= limit {
            return Err(format!(
                "daily trade cap reached ({}/{} entries today)",
                state.trades_today, limit
            ));
        }
    }

    if let Some(limit) = limits.max_consecutive_losses {
        if limit > 0 && state.consecutive_losses >= limit {
            return Err(format!(
                "{} consecutive losing trades (limit {})",
                state.consecutive_losses, limit
            ));
        }
    }

    if limits.token_cooldown_minutes > 0 {
        if let Some(last_exit) = state.last_exit_by_token.get(token_address) {
            let cooldown = ChronoDuration::minutes(limits.token_cooldown_minutes as i64);
            let elapsed = now.signed_duration_since(*last_exit);
            if elapsed < cooldown {
                let remaining = (cooldown - elapsed).num_seconds().max(0);
                return Err(format!(
                    "token {} is in post-exit cooldown for another {}s",
                    token_address, remaining
                ));
            }
        }
    }

    Ok(())
}

/// Fold a closed position into the guard's book-keeping and trip any rails that
/// are now breached. Pure — safe to unit test.
///
/// Returns the halt that was newly triggered, if any.
pub fn apply_exit(
    limits: &RiskLimits,
    state: &mut RiskGuardState,
    token_address: &str,
    realized_pnl_sol: f64,
    now: DateTime<Utc>,
) -> Option<ActiveHalt> {
    state.realized_pnl_today_sol += realized_pnl_sol;
    state.realized_pnl_total_sol += realized_pnl_sol;

    if realized_pnl_sol >= 0.0 {
        state.wins += 1;
        state.consecutive_losses = 0;
    } else {
        state.losses += 1;
        state.consecutive_losses += 1;
    }

    state
        .last_exit_by_token
        .insert(token_address.to_string(), now);

    state.equity_sol = limits.starting_equity_sol + state.realized_pnl_total_sol;
    if state.equity_sol > state.peak_equity_sol {
        state.peak_equity_sol = state.equity_sol;
    }

    // Equity recovery clears a drawdown halt (the drawdown itself is unchanged,
    // but trading below the threshold again is by definition allowed).
    if let Some(halt) = &state.halt {
        if halt.kind == HaltKind::Drawdown {
            if let Some(limit) = limits.max_drawdown_percent {
                let floor = state.peak_equity_sol * (1.0 - limit.abs() / 100.0);
                if state.equity_sol > floor {
                    state.halt = None;
                }
            }
        }
    }

    if state.halt.is_some() {
        return None;
    }

    // --- Trip rails, most severe first -------------------------------------
    if let Some(limit) = limits.daily_loss_limit_sol {
        let limit = limit.abs();
        if limit > 0.0 && state.realized_pnl_today_sol <= -limit {
            let halt = make_halt(
                HaltKind::DailyLoss,
                format!(
                    "realised PnL today {:.4} SOL ≤ -{:.4} SOL",
                    state.realized_pnl_today_sol, limit
                ),
                now,
            );
            state.halt = Some(halt.clone());
            return Some(halt);
        }
    }

    if let Some(limit) = limits.max_drawdown_percent {
        let limit = limit.abs();
        if limit > 0.0 && state.peak_equity_sol > 0.0 {
            let floor = state.peak_equity_sol * (1.0 - limit / 100.0);
            if state.equity_sol <= floor {
                let drawdown = if state.peak_equity_sol > 0.0 {
                    (state.peak_equity_sol - state.equity_sol) / state.peak_equity_sol * 100.0
                } else {
                    0.0
                };
                let halt = make_halt(
                    HaltKind::Drawdown,
                    format!(
                        "equity {:.4} SOL is {:.2}% below peak {:.4} SOL (limit {:.2}%)",
                        state.equity_sol, drawdown, state.peak_equity_sol, limit
                    ),
                    now,
                );
                state.halt = Some(halt.clone());
                return Some(halt);
            }
        }
    }

    if let Some(limit) = limits.max_consecutive_losses {
        if limit > 0 && state.consecutive_losses >= limit {
            let halt = make_halt(
                HaltKind::ConsecutiveLosses,
                format!("{} losing trades in a row (limit {})", state.consecutive_losses, limit),
                now,
            );
            state.halt = Some(halt.clone());
            return Some(halt);
        }
    }

    None
}

/// The guard itself: thin async wrapper around the pure logic above.
pub struct RiskGuard {
    limits: RiskLimits,
    state: RwLock<RiskGuardState>,
}

impl RiskGuard {
    pub fn new(limits: RiskLimits) -> Self {
        let state = RiskGuardState::new(limits.starting_equity_sol);
        Self { limits, state: RwLock::new(state) }
    }

    /// Build a guard straight from the app config.
    pub fn from_config(config: &Config) -> Self {
        Self::new(RiskLimits::from_config(config))
    }

    pub fn limits(&self) -> &RiskLimits {
        &self.limits
    }

    /// Gate a new entry. `Err` carries the reason, which callers log/return.
    pub async fn check_entry(&self, token_address: &str) -> Result<(), String> {
        let now = Utc::now();
        let mut state = self.state.write().await;
        roll_day(&mut state, now.date_naive());
        match evaluate_entry(&self.limits, &state, token_address, now) {
            Ok(()) => Ok(()),
            Err(reason) => {
                state.entry_refusals += 1;
                Err(reason)
            }
        }
    }

    /// Count a filled entry against today's quota.
    pub async fn record_entry(&self) {
        let now = Utc::now();
        let mut state = self.state.write().await;
        roll_day(&mut state, now.date_naive());
        state.trades_today += 1;
        state.trades_total += 1;
    }

    /// Fold a closed position into the guard. Returns a newly tripped halt.
    pub async fn record_exit(
        &self,
        token_address: &str,
        realized_pnl_sol: f64,
    ) -> Option<ActiveHalt> {
        let now = Utc::now();
        let mut state = self.state.write().await;
        roll_day(&mut state, now.date_naive());
        apply_exit(&self.limits, &mut state, token_address, realized_pnl_sol, now)
    }

    /// Trip the kill switch (or any other manual halt).
    pub async fn halt(&self, kind: HaltKind, reason: impl Into<String>) -> ActiveHalt {
        let now = Utc::now();
        let mut state = self.state.write().await;
        let halt = make_halt(kind, reason.into(), now);
        state.halt = Some(halt.clone());
        halt
    }

    /// Convenience for the HTTP kill switch.
    pub async fn halt_manual(&self, reason: impl Into<String>) -> ActiveHalt {
        self.halt(HaltKind::Manual, reason).await
    }

    /// Clear a halt. `clear_counters` also zeroes today's PnL/trade counters —
    /// useful when the operator wants a clean slate mid-day.
    pub async fn reset(&self, clear_counters: bool) -> RiskGuardState {
        let now = Utc::now();
        let mut state = self.state.write().await;
        roll_day(&mut state, now.date_naive());
        state.halt = None;
        if clear_counters {
            state.realized_pnl_today_sol = 0.0;
            state.trades_today = 0;
            state.consecutive_losses = 0;
            state.entry_refusals = 0;
        }
        state.clone()
    }

    pub async fn is_halted(&self) -> bool {
        let state = self.state.read().await;
        state.halt.is_some()
    }

    /// Snapshot for `/api/risk/status`.
    pub async fn snapshot(&self) -> RiskGuardState {
        let now = Utc::now();
        let mut state = self.state.write().await;
        roll_day(&mut state, now.date_naive());
        state.clone()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> RiskLimits {
        RiskLimits {
            daily_loss_limit_sol: Some(0.10),
            max_drawdown_percent: Some(25.0),
            max_trades_per_day: Some(3),
            max_consecutive_losses: Some(2),
            token_cooldown_minutes: 30,
            starting_equity_sol: 1.0,
        }
    }

    fn fresh(limits: &RiskLimits) -> RiskGuardState {
        RiskGuardState::new(limits.starting_equity_sol)
    }

    #[test]
    fn entry_allowed_when_no_rails_are_breached() {
        let l = limits();
        let s = fresh(&l);
        assert!(evaluate_entry(&l, &s, "MintA", Utc::now()).is_ok());
    }

    #[test]
    fn daily_loss_limit_blocks_and_halts() {
        let l = limits();
        let mut s = fresh(&l);
        let now = Utc::now();

        // A single -0.10 SOL loss trips the limit exactly.
        let halt = apply_exit(&l, &mut s, "MintA", -0.10, now).expect("halt expected");
        assert_eq!(halt.kind, HaltKind::DailyLoss);

        let err = evaluate_entry(&l, &s, "MintB", now).unwrap_err();
        assert!(err.contains("halted"), "unexpected error: {err}");
    }

    #[test]
    fn daily_loss_limit_survives_until_next_utc_day() {
        let l = limits();
        let mut s = fresh(&l);
        apply_exit(&l, &mut s, "MintA", -0.25, Utc::now());

        // Same day: still halted.
        roll_day(&mut s, Utc::now().date_naive());
        assert!(s.halt.is_some());

        // Next UTC day: daily rails re-arm, counters are zeroed.
        let tomorrow = Utc::now().date_naive() + ChronoDuration::days(1);
        roll_day(&mut s, tomorrow);
        assert!(s.halt.is_none());
        assert_eq!(s.realized_pnl_today_sol, 0.0);
        assert_eq!(s.trades_today, 0);
        // …but the all-time book-keeping is untouched.
        assert!((s.realized_pnl_total_sol + 0.25).abs() < 1e-9);
    }

    #[test]
    fn trade_cap_blocks_after_configured_entries() {
        let l = limits();
        let mut s = fresh(&l);
        s.trades_today = 3;
        let err = evaluate_entry(&l, &s, "MintA", Utc::now()).unwrap_err();
        assert!(err.contains("daily trade cap"), "unexpected error: {err}");
    }

    #[test]
    fn token_cooldown_blocks_re_entry() {
        let l = limits();
        let mut s = fresh(&l);
        let now = Utc::now();
        apply_exit(&l, &mut s, "MintA", 0.05, now);

        // Immediately after the exit the mint is cooling down.
        let err = evaluate_entry(&l, &s, "MintA", now).unwrap_err();
        assert!(err.contains("cooldown"), "unexpected error: {err}");

        // A different mint is unaffected.
        assert!(evaluate_entry(&l, &s, "MintB", now).is_ok());

        // After the cooldown window it is tradeable again.
        let later = now + ChronoDuration::minutes(31);
        assert!(evaluate_entry(&l, &s, "MintA", later).is_ok());
    }

    #[test]
    fn consecutive_losses_pause_trading_until_next_day() {
        let l = limits();
        let mut s = fresh(&l);
        let now = Utc::now();

        apply_exit(&l, &mut s, "MintA", -0.01, now);
        assert!(s.halt.is_none(), "one loss must not halt");

        let halt = apply_exit(&l, &mut s, "MintB", -0.02, now).expect("second loss halts");
        assert_eq!(halt.kind, HaltKind::ConsecutiveLosses);

        // A winner would normally reset the streak…
        let mut recovered = fresh(&l);
        apply_exit(&l, &mut recovered, "MintA", -0.01, now);
        apply_exit(&l, &mut recovered, "MintB", 0.05, now);
        assert_eq!(recovered.consecutive_losses, 0);
        assert!(recovered.halt.is_none());
    }

    #[test]
    fn drawdown_rail_trips_and_recovers() {
        let l = RiskLimits { max_consecutive_losses: None, ..limits() };
        let mut s = fresh(&l);
        let now = Utc::now();

        // Start with a win so the peak moves up to 1.20 SOL.
        apply_exit(&l, &mut s, "MintA", 0.20, now);
        assert!((s.peak_equity_sol - 1.20).abs() < 1e-9);

        // Losing 0.05 SOL keeps us inside the 25% band (floor 0.90).
        assert!(apply_exit(&l, &mut s, "MintB", -0.05, now).is_none());

        // A further 0.30 SOL loss ($0.85 equity) is below the floor → halt.
        let halt = apply_exit(&l, &mut s, "MintC", -0.30, now).expect("drawdown halts");
        assert_eq!(halt.kind, HaltKind::Drawdown);
        assert!(evaluate_entry(&l, &s, "MintD", now).is_err());

        // Drawdown halts do NOT clear just because the date changed…
        roll_day(&mut s, Utc::now().date_naive() + ChronoDuration::days(1));
        assert!(s.halt.is_some());

        // …but a recovery above the floor clears them.
        apply_exit(&l, &mut s, "MintE", 0.60, now);
        assert!(s.halt.is_none(), "recovery should clear the drawdown halt");
    }

    #[tokio::test]
    async fn guard_refuses_entries_after_manual_kill_switch() {
        let guard = RiskGuard::new(limits());
        assert!(guard.check_entry("MintA").await.is_ok());

        let halt = guard.halt_manual("operator pressed stop").await;
        assert_eq!(halt.kind, HaltKind::Manual);

        let err = guard.check_entry("MintA").await.unwrap_err();
        assert!(err.contains("manual"), "unexpected error: {err}");

        // A new day must not silently re-arm a manual halt.
        {
            let mut state = guard.state.write().await;
            roll_day(&mut state, Utc::now().date_naive() + ChronoDuration::days(1));
        }
        assert!(guard.check_entry("MintA").await.is_err());

        // Only an explicit reset clears it.
        let snapshot = guard.reset(false).await;
        assert!(snapshot.halt.is_none());
        assert!(guard.check_entry("MintA").await.is_ok());
    }

    #[tokio::test]
    async fn record_entry_and_exit_update_counters() {
        let guard = RiskGuard::new(limits());
        guard.record_entry().await;
        guard.record_entry().await;

        let snapshot = guard.snapshot().await;
        assert_eq!(snapshot.trades_today, 2);
        assert_eq!(snapshot.trades_total, 2);

        guard.record_exit("MintA", 0.07).await;
        let snapshot = guard.snapshot().await;
        assert_eq!(snapshot.wins, 1);
        assert!((snapshot.realized_pnl_today_sol - 0.07).abs() < 1e-9);
        assert!((snapshot.equity_sol - 1.07).abs() < 1e-9);
    }

    #[tokio::test]
    async fn refused_entries_are_counted_for_visibility() {
        let guard = RiskGuard::new(limits());
        guard.halt_manual("test").await;
        let _ = guard.check_entry("MintA").await;
        let _ = guard.check_entry("MintB").await;
        assert_eq!(guard.snapshot().await.entry_refusals, 2);
    }
}
