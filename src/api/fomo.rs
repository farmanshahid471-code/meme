//! fomo.family data client for the leaderboard-copying strategy.
//!
//! # Why this looks the way it does
//!
//! fomo.family has **no public API**. Their own front end talks to
//! `https://prod-api.fomo.family` with a Privy-issued JWT (about 1 hour TTL) and
//! Cloudflare rejects non-browser clients at the edge (HTTP 430,
//! `{"error":"unauthorized"}`) — several independent projects have confirmed this
//! even when replaying a real browser's headers. So there is no single endpoint
//! we can simply point the bot at and rely on forever.
//!
//! Instead this module speaks a *normalised* set of shapes and can be fed by any
//! of three providers, chosen with `FOMO_PROVIDER`:
//!
//! | Provider | Base URL | Auth | Notes |
//! |----------|----------|------|-------|
//! | `official` | `https://prod-api.fomo.family` | `FOMO_AUTH_TOKEN` (JWT), optional `FOMO_CF_COOKIE` | Clan leaderboard + clan members. Often blocked with 430 from datacentres. |
//! | `fomoapi` | `https://api.fomoapi.io` | `FOMO_API_KEY` (free key) | Documented third-party mirror. Server-friendly. Trader-based. |
//! | `custom` | `FOMO_API_BASE` | `FOMO_API_KEY` | Anything that returns the same JSON: your own proxy, a browser extension bridge, a cache you refresh from the console. |
//!
//! Parsing is deliberately tolerant: responses may be wrapped in
//! `{success, responseObject:{...}}` (official), returned flat (`{traders:[...]}`,
//! `{leaderboard:[...]}`), and field names differ between providers
//! (`userHandle`/`handle`, `pnl24h`/`pnlUsd`, `id`/`userId`). Every reader looks
//! up a list of aliases instead of one exact key, so a field rename upstream
//! degrades to "unknown" rather than a hard failure.
//!
//! Everything reaching the trading engine is normalised into [`FomoClan`],
//! [`FomoTrader`] and [`FomoSwap`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tracing::debug;

/// Which upstream the client talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FomoProvider {
    /// fomo.family's own API (`prod-api.fomo.family`).
    Official,
    /// Documented third-party mirror (`api.fomoapi.io`), server-friendly.
    FomoApiIo,
    /// Any endpoint that speaks the same JSON (proxy, extension bridge, cache).
    Custom,
}

impl FomoProvider {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "official" | "fomo" | "fomo.family" | "prod" => FomoProvider::Official,
            "fomoapi" | "fomoapi.io" | "third_party" | "mirror" => FomoProvider::FomoApiIo,
            _ => FomoProvider::Custom,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FomoProvider::Official => "official",
            FomoProvider::FomoApiIo => "fomoapi",
            FomoProvider::Custom => "custom",
        }
    }

    /// Default base URL when `FOMO_API_BASE` is not set.
    pub fn default_base_url(self) -> &'static str {
        match self {
            FomoProvider::Official => "https://prod-api.fomo.family",
            FomoProvider::FomoApiIo => "https://api.fomoapi.io",
            FomoProvider::Custom => "",
        }
    }
}

/// A clan (fomo's group of traders) as ranked on the clan leaderboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FomoClan {
    pub id: String,
    pub name: String,
    pub pnl_usd: f64,
    pub member_count: Option<u32>,
    pub icon: Option<String>,
}

/// A trader, either from the trader leaderboard or resolved from a handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FomoTrader {
    pub user_id: String,
    pub handle: String,
    pub display_name: String,
    pub pnl_usd: f64,
    /// fomo's *profile* address. Note: not the wallet that executes swaps.
    pub profile_address: Option<String>,
    pub clan_id: Option<String>,
    pub clan_name: Option<String>,
    pub verified: bool,
}

/// One leg of a swap: the token and how much of it moved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FomoTokenLeg {
    pub address: String,
    pub symbol: Option<String>,
    pub human_amount: f64,
    /// Network the leg happened on (fomo network id, e.g. `1399811149` = Solana).
    pub network_id: Option<String>,
}

/// A single swap made by a trader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FomoSwap {
    pub id: String,
    pub created_at: Option<DateTime<Utc>>,
    /// What the trader spent (token/amount).
    pub spent: Option<FomoTokenLeg>,
    /// What the trader received.
    pub received: Option<FomoTokenLeg>,
    /// USD value of the swap, as reported by fomo.
    pub usd_value: f64,
}

/// Which way a swap moved a tradeable token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapSide {
    Buy,
    Sell,
}

/// A swap reduced to what the copy engine actually needs: buy or sell, of which
/// Solana mint, and how big.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MirrorSignal {
    pub side: SwapSide,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub usd_value: f64,
    pub swap_id: String,
    pub at: Option<DateTime<Utc>>,
}

impl MirrorSignal {
    /// Compact one-line description for logs and the dashboard feed.
    pub fn describe(&self) -> String {
        format!(
            "{} {} (${:.2})",
            match self.side {
                SwapSide::Buy => "BUY",
                SwapSide::Sell => "SELL",
            },
            self.token_symbol.clone().unwrap_or_else(|| short(&self.token_address)),
            self.usd_value
        )
    }
}

fn short(address: &str) -> String {
    if address.len() <= 10 {
        address.to_string()
    } else {
        format!("{}…{}", &address[..4], &address[address.len() - 4..])
    }
}

/// Client configuration, derived from [`crate::config::Config`].
#[derive(Debug, Clone)]
pub struct FomoClientConfig {
    pub provider: FomoProvider,
    pub base_url: String,
    /// Bearer token: a `fomoapi.io` key, an official JWT, or a custom-endpoint key.
    pub api_key: Option<String>,
    /// `Cookie` header, e.g. `__cf_bm=…` for the official endpoint.
    pub cookie: Option<String>,
    /// Leaderboard window: `24h`, `7d`, `30d`, `all`.
    pub window: String,
    /// Path template for clan members. `{clan_id}` is substituted.
    pub clan_members_path: String,
    /// Network ids that mean "Solana" for this deployment.
    pub solana_network_ids: Vec<String>,
    pub timeout_secs: u64,
}

impl FomoClientConfig {
    pub fn from_config(config: &crate::config::Config) -> Self {
        let provider = FomoProvider::parse(&config.fomo_provider);
        let base_url = config
            .fomo_api_base
            .clone()
            .filter(|base| !base.trim().is_empty())
            .unwrap_or_else(|| provider.default_base_url().to_string());
        // An explicit auth token wins over the generic key: on the official
        // endpoint the key *is* the short-lived JWT.
        let api_key = config
            .fomo_auth_token
            .clone()
            .or_else(|| config.fomo_api_key.clone())
            .filter(|value| !value.trim().is_empty());

        Self {
            provider,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            cookie: config.fomo_cf_cookie.clone().filter(|c| !c.trim().is_empty()),
            window: config.fomo_window.clone(),
            clan_members_path: config.fomo_clan_members_path.clone(),
            solana_network_ids: config.fomo_solana_network_ids.clone(),
            timeout_secs: 20,
        }
    }

    /// Is this configuration usable at all?
    pub fn is_configured(&self) -> bool {
        !self.base_url.is_empty()
    }
}

/// Fatal-ish upstream conditions worth surfacing to the operator verbatim.
#[derive(Debug, Clone)]
pub enum FomoError {
    /// Cloudflare rejected the client before the app saw the request.
    EdgeBlocked(String),
    /// Authentication/authorisation problem (401/403).
    Unauthorized(String),
    /// Upstream said not found.
    NotFound(String),
    /// Anything else (network, decode, 5xx…).
    Other(String),
}

impl std::fmt::Display for FomoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FomoError::EdgeBlocked(msg) => write!(f, "fomo edge blocked the request: {msg}"),
            FomoError::Unauthorized(msg) => write!(f, "fomo authorization failed: {msg}"),
            FomoError::NotFound(msg) => write!(f, "fomo resource not found: {msg}"),
            FomoError::Other(msg) => write!(f, "fomo request failed: {msg}"),
        }
    }
}

impl std::error::Error for FomoError {}

/// HTTP client for whichever fomo-shaped provider is configured.
pub struct FomoClient {
    http: reqwest::Client,
    config: FomoClientConfig,
}

impl FomoClient {
    pub fn new(config: FomoClientConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.max(5)))
            .user_agent("trader-tony-v4/0.1 (+fomo-copy)")
            .build()
            .unwrap_or_default();
        Self { http, config }
    }

    pub fn config(&self) -> &FomoClientConfig {
        &self.config
    }

    pub fn provider(&self) -> FomoProvider {
        self.config.provider
    }

    /// GET a path and decode JSON, mapping upstream failures onto [`FomoError`].
    async fn get_json(&self, path: &str) -> Result<Value, FomoError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.config.base_url, path)
        };

        let mut request = self.http.get(&url);
        if let Some(key) = &self.config.api_key {
            request = request.header("authorization", format!("Bearer {key}"));
        }
        if let Some(cookie) = &self.config.cookie {
            request = request.header("cookie", cookie.clone());
        }
        if self.config.provider == FomoProvider::Official {
            // The web app always sends this; it also keeps us on Solana-friendly
            // network ids when fomo decides to change defaults.
            request = request.header("x-supported-chains", "1399811149,8453,1,56,4663");
            request = request.header("origin", "https://fomo.family");
            request = request.header("referer", "https://fomo.family/");
        }

        debug!("fomo GET {}", url);
        let response = request.send().await.map_err(|e| FomoError::Other(e.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| FomoError::Other(e.to_string()))?;

        if status == 430 || body.contains("\"error\":\"unauthorized\"") && status >= 400 {
            return Err(FomoError::EdgeBlocked(format!(
                "{status} from {url} — Cloudflare rejects non-browser clients. \
                 Use FOMO_PROVIDER=fomoapi with an API key, point FOMO_API_BASE at a proxy/bridge, \
                 or refresh from a browser session via tools/fomo-probe.js"
            )));
        }
        if status == 401 || status == 403 {
            return Err(FomoError::Unauthorized(format!("{status} from {url}: {}", truncate(&body, 200))));
        }
        if status == 404 {
            return Err(FomoError::NotFound(format!("{status} from {url}: {}", truncate(&body, 200))));
        }
        if !(200..300).contains(&status) {
            return Err(FomoError::Other(format!("{status} from {url}: {}", truncate(&body, 200))));
        }

        serde_json::from_str::<Value>(&body)
            .map_err(|e| FomoError::Other(format!("invalid JSON from {url}: {e}")))
    }

    /// Clan leaderboard: the "top clan" half of the strategy.
    ///
    /// `official` uses `/v2/clans/leaderboard?window=…&limit=…`.
    /// `fomoapi`/`custom` fall back to the trader leaderboard and *derive* clans
    /// from the `clan` field, because that mirror has no clan board.
    pub async fn clan_leaderboard(&self, limit: u32) -> Result<Vec<FomoClan>, FomoError> {
        let path = format!(
            "/v2/clans/leaderboard?window={}&limit={}",
            self.config.window, limit
        );
        match self.get_json(&path).await {
            Ok(value) => {
                let clans = parse_clans(&value);
                if clans.is_empty() {
                    // Some deployments expose clans under the trader board only.
                    debug!("clan board returned no rows — falling back to the trader leaderboard");
                    return self.clans_from_traders(limit).await;
                }
                Ok(clans)
            }
            Err(FomoError::NotFound(_)) | Err(FomoError::Other(_)) => self.clans_from_traders(limit).await,
            Err(other) => Err(other),
        }
    }

    /// Derive a clan ranking from the trader leaderboard's `clan` field.
    async fn clans_from_traders(&self, limit: u32) -> Result<Vec<FomoClan>, FomoError> {
        let traders = self.trader_leaderboard(limit.max(50)).await?;
        let mut clans: Vec<FomoClan> = Vec::new();
        for trader in traders {
            let (Some(id), name) = (
                trader.clan_id.clone(),
                trader.clan_name.clone().unwrap_or_else(|| "unknown".to_string()),
            ) else {
                continue;
            };
            match clans.iter_mut().find(|clan| clan.id == id) {
                // Sum member PnL: with no clan board available this is our best proxy.
                Some(existing) => {
                    existing.pnl_usd += trader.pnl_usd;
                    existing.member_count = existing.member_count.map(|count| count + 1);
                }
                None => clans.push(FomoClan {
                    id,
                    name,
                    pnl_usd: trader.pnl_usd,
                    member_count: Some(1),
                    icon: None,
                }),
            }
        }
        clans.sort_by(|a, b| b.pnl_usd.total_cmp(&a.pnl_usd));
        Ok(clans)
    }

    /// Trader leaderboard ranked by PnL for the configured window.
    pub async fn trader_leaderboard(&self, limit: u32) -> Result<Vec<FomoTrader>, FomoError> {
        let path = format!(
            "/v2/leaderboard/{}?limit={}",
            self.config.window,
            limit.min(150)
        );
        let value = self.get_json(&path).await?;
        Ok(parse_traders(&value, &self.fomo_pnl_keys()))
    }

    /// Members of a clan, ranked by PnL (the "top member" half of the strategy).
    pub async fn clan_members(&self, clan_id: &str, limit: u32) -> Result<Vec<FomoTrader>, FomoError> {
        let path = self
            .config
            .clan_members_path
            .replace("{clan_id}", clan_id)
            .replace("{window}", &self.config.window);
        let path = if path.contains('?') {
            format!("{path}&limit={limit}")
        } else {
            format!("{path}?limit={limit}")
        };

        match self.get_json(&path).await {
            Ok(value) => {
                let members = parse_traders(&value, &self.fomo_pnl_keys());
                if !members.is_empty() {
                    return Ok(members);
                }
                debug!("clan members payload had no rows — falling back to the trader leaderboard");
            }
            Err(FomoError::NotFound(_)) => {
                debug!("clan members endpoint returned 404 — falling back to the trader leaderboard");
            }
            Err(FomoError::EdgeBlocked(e)) => return Err(FomoError::EdgeBlocked(e)),
            Err(FomoError::Unauthorized(e)) => return Err(FomoError::Unauthorized(e)),
            Err(FomoError::Other(e)) => debug!("clan members request failed ({e}) — falling back"),
        }

        // Fallback: filter the trader leaderboard by clan id/name.
        let traders = self.trader_leaderboard(150).await?;
        let wanted = clan_id.to_lowercase();
        Ok(traders
            .into_iter()
            .filter(|trader| {
                trader
                    .clan_id
                    .as_deref()
                    .map(|id| id.to_lowercase() == wanted)
                    .unwrap_or(false)
                    || trader
                        .clan_name
                        .as_deref()
                        .map(|name| name.to_lowercase() == wanted)
                        .unwrap_or(false)
            })
            .collect())
    }

    /// Resolve a handle (or user id) to a trader profile.
    pub async fn trader_profile(&self, handle_or_id: &str) -> Result<FomoTrader, FomoError> {
        let path = match self.config.provider {
            // The official API has a distinct lookup route for handles.
            FomoProvider::Official => format!("/v2/users/userHandle/{handle_or_id}"),
            _ => format!("/v2/users/{handle_or_id}"),
        };
        let value = self.get_json(&path).await?;
        parse_traders(&value, &self.fomo_pnl_keys())
            .into_iter()
            .next()
            .ok_or_else(|| FomoError::NotFound(format!("no trader in response for {handle_or_id}")))
    }

    /// Recent swaps made by a trader — the copy signal.
    ///
    /// `official` reads `/v2/users/{id}/swaps`. The mirror exposes the same data
    /// as aggregated positions under `/positions`; we ask for `/swaps` first and
    /// fall back, because positions lose the individual fill timestamps.
    pub async fn swaps(&self, user_id: &str, limit: u32) -> Result<Vec<FomoSwap>, FomoError> {
        let candidates = match self.config.provider {
            FomoProvider::Official => vec![format!("/v2/users/{user_id}/swaps?limit={limit}")],
            _ => vec![
                format!("/v2/users/{user_id}/swaps?limit={limit}"),
                format!("/v2/users/{user_id}/positions?limit={limit}"),
                format!("/v2/users/{user_id}/trades?limit={limit}"),
            ],
        };

        let mut last_error: Option<FomoError> = None;
        for path in candidates {
            match self.get_json(&path).await {
                Ok(value) => {
                    let swaps = parse_swaps(&value);
                    if !swaps.is_empty() {
                        return Ok(swaps);
                    }
                }
                Err(FomoError::EdgeBlocked(e)) => return Err(FomoError::EdgeBlocked(e)),
                Err(FomoError::Unauthorized(e)) => return Err(FomoError::Unauthorized(e)),
                Err(e) => last_error = Some(e),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            FomoError::NotFound(format!("no swaps available for user {user_id}"))
        }))
    }

    fn fomo_pnl_keys(&self) -> Vec<String> {
        let mut keys = vec![
            "pnlUsd".to_string(),
            "pnl".to_string(),
            "realizedPnlUsd".to_string(),
        ];
        match self.config.window.to_lowercase().as_str() {
            "7d" => keys.insert(0, "pnl7d".to_string()),
            "30d" => keys.insert(0, "pnl30d".to_string()),
            "all" => keys.insert(0, "pnlAll".to_string()),
            _ => keys.insert(0, "pnl24h".to_string()),
        }
        keys
    }

    /// Turn a raw swap into a mirror signal if it moved a Solana token.
    pub fn to_mirror_signal(&self, swap: &FomoSwap) -> Option<MirrorSignal> {
        let spent_is_solana = swap
            .spent
            .as_ref()
            .map(|leg| self.is_solana_leg(leg))
            .unwrap_or(false);
        let received_is_solana = swap
            .received
            .as_ref()
            .map(|leg| self.is_solana_leg(leg))
            .unwrap_or(false);

        // A buy: paid with something, received a Solana token.
        // A sell: spent a Solana token, received something else.
        let (side, leg) = match (spent_is_solana, received_is_solana) {
            (false, true) => (SwapSide::Buy, swap.received.as_ref()?),
            (true, false) => (SwapSide::Sell, swap.spent.as_ref()?),
            // Token-for-token or same-chain both sides: not a clean copy signal.
            _ => return None,
        };

        if leg.address.trim().is_empty() {
            return None;
        }

        Some(MirrorSignal {
            side,
            token_address: leg.address.clone(),
            token_symbol: leg.symbol.clone(),
            usd_value: swap.usd_value,
            swap_id: swap.id.clone(),
            at: swap.created_at,
        })
    }

    fn is_solana_leg(&self, leg: &FomoTokenLeg) -> bool {
        match &leg.network_id {
            Some(id) => self
                .config
                .solana_network_ids
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(id.trim())),
            // No id: fall back to address shape. Solana mints are base58 and
            // usually 32–44 chars; pump.fun mints always end in "pump".
            None => looks_like_solana_mint(&leg.address),
        }
    }
}

/// Cheap structural check used only when the payload omits a network id.
pub fn looks_like_solana_mint(address: &str) -> bool {
    let trimmed = address.trim();
    if trimmed.starts_with("0x") || trimmed.len() < 32 || trimmed.len() > 44 {
        return false;
    }
    trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() && c != '0' && c != 'O' && c != 'I' && c != 'l')
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}

// ============================================================================
// Tolerant parsing helpers
// ============================================================================

/// Unwrap fomo's `{success, message, responseObject}` envelope when present.
fn unwrap_envelope(value: &Value) -> &Value {
    value.get("responseObject").unwrap_or(value)
}

/// Find the first array under any of the given keys, searching the envelope too.
fn find_array<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Vec<Value>> {
    let candidates = [
        value,
        unwrap_envelope(value),
        value.get("data").unwrap_or(value),
    ];
    for candidate in candidates {
        for key in keys {
            if let Some(array) = candidate.get(key).and_then(|v| v.as_array()) {
                if !array.is_empty() {
                    return Some(array);
                }
            }
        }
    }
    // Single-object responses (e.g. /v2/users/{handle}) are treated as one row.
    None
}

/// Read a float from the first present alias.
fn number(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        match value.get(*key) {
            Some(Value::Number(n)) => return n.as_f64(),
            Some(Value::String(s)) => {
                if let Ok(parsed) = s.trim().replace(',', "").replace('$', "").parse::<f64>() {
                    return Some(parsed);
                }
            }
            _ => {}
        }
    }
    None
}

/// Read a string from the first present alias.
fn string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        match value.get(*key) {
            Some(Value::String(s)) if !s.trim().is_empty() => return Some(s.trim().to_string()),
            Some(Value::Number(n)) => return Some(n.to_string()),
            _ => {}
        }
    }
    None
}

/// `clan` may be an object (`{id,name,icon,role}`) or a plain string.
fn parse_clan_fields(value: &Value) -> (Option<String>, Option<String>, Option<String>) {
    match value.get("clan") {
        Some(Value::Object(_)) => {
            let clan = value.get("clan").unwrap();
            (
                string(clan, &["id", "clanId", "clan_id"]),
                string(clan, &["name", "displayName", "clanName"]),
                string(clan, &["icon", "image", "avatar"]),
            )
        }
        Some(Value::String(name)) if !name.trim().is_empty() => {
            (None, Some(name.trim().to_string()), None)
        }
        _ => (
            string(value, &["clanId", "clan_id"]),
            string(value, &["clanName", "clan_name"]),
            None,
        ),
    }
}

pub fn parse_traders(value: &Value, pnl_keys: &[String]) -> Vec<FomoTrader> {
    let keys: Vec<&str> = pnl_keys.iter().map(String::as_str).collect();
    let rows: Vec<&Value> = match find_array(
        value,
        &["leaderboard", "traders", "members", "users", "items", "rows", "data"],
    ) {
        Some(array) => array.iter().collect(),
        None => {
            let single = unwrap_envelope(value);
            if single.get("userHandle").is_some()
                || single.get("handle").is_some()
                || single.get("userHandle").is_some()
                || single.get("id").is_some()
            {
                vec![single]
            } else {
                Vec::new()
            }
        }
    };

    rows.into_iter()
        .filter_map(|row| {
            let handle = string(row, &["userHandle", "handle", "username", "user_handle"])?;
            let (clan_id, clan_name, _) = parse_clan_fields(row);
            let profile_address = string(row, &["address", "profileAddress"])
                .or_else(|| {
                    row.get("wallets")
                        .and_then(|w| string(w, &["solana", "sol"]))
                });
            Some(FomoTrader {
                user_id: string(row, &["id", "userId", "user_id", "uuid"])
                    .unwrap_or_else(|| handle.clone()),
                handle,
                display_name: string(row, &["displayName", "display_name", "name"])
                    .unwrap_or_else(|| "unknown".to_string()),
                pnl_usd: number(row, &keys).unwrap_or(0.0),
                profile_address,
                clan_id,
                clan_name,
                verified: row
                    .get("verified")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

pub fn parse_clans(value: &Value) -> Vec<FomoClan> {
    let rows: Vec<&Value> = match find_array(value, &["leaderboard", "clans", "items", "rows", "data"]) {
        Some(array) => array.iter().collect(),
        None => Vec::new(),
    };

    rows.into_iter()
        .filter_map(|row| {
            // Some payloads nest the clan under "clan"; others are flat rows.
            let node = row.get("clan").filter(|c| c.is_object()).unwrap_or(row);
            let name = string(node, &["name", "displayName", "clanName"])
                .or_else(|| string(row, &["name", "displayName"]))?;
            let id = string(node, &["id", "clanId", "clan_id", "uuid"])
                .or_else(|| string(row, &["id", "clanId", "clan_id", "uuid"]))
                .unwrap_or_else(|| name.clone());

            Some(FomoClan {
                id,
                name,
                pnl_usd: number(
                    row,
                    &[
                        "pnlUsd", "pnl", "pnl24h", "pnl7d", "pnl30d", "totalPnl", "profit",
                    ],
                )
                .or_else(|| number(node, &["pnlUsd", "pnl"]))
                .unwrap_or(0.0),
                member_count: number(row, &["memberCount", "members", "numMembers", "member_count"])
                    .or_else(|| number(node, &["memberCount", "members"]))
                    .map(|n| n as u32),
                icon: string(node, &["icon", "image", "avatar"]),
            })
        })
        .collect()
}

pub fn parse_swaps(value: &Value) -> Vec<FomoSwap> {
    let rows: Vec<&Value> = match find_array(
        value,
        &["swaps", "trades", "positions", "items", "rows", "activities", "data"],
    ) {
        Some(array) => array.iter().collect(),
        None => Vec::new(),
    };

    rows.into_iter()
        .filter_map(|row| {
            let id = string(row, &["id", "swapId", "tradeId"])
                .or_else(|| string(row, &["createdAt", "timestamp"]))
                .unwrap_or_else(|| "unknown".to_string());
            let created_at = string(row, &["createdAt", "timestamp", "time", "openedAt", "occurredAt"])
                .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
                .map(|dt| dt.with_timezone(&Utc));

            let spent = parse_leg(row, "in");
            let received = parse_leg(row, "out");

            if spent.is_none() && received.is_none() {
                // Aggregated position shape (`/positions`): treat the bought side
                // as "received" so it still reads as a buy.
                let token = parse_leg(row, "token");
                if let Some(leg) = token {
                    let usd = number(
                        row,
                        &[
                            "costBasisUsd",
                            "humanUsdAmountOut",
                            "valueUsd",
                            "usdValue",
                        ],
                    )
                    .unwrap_or(0.0);
                    return Some(FomoSwap {
                        id,
                        created_at,
                        spent: None,
                        received: Some(leg),
                        usd_value: usd,
                    });
                }
                return None;
            }

            let usd_value = number(
                row,
                &[
                    "humanUsdAmountOut",
                    "humanUsdAmountIn",
                    "usdValue",
                    "valueUsd",
                    "amountUsd",
                ],
            )
            .or_else(|| received.as_ref().map(|_| 0.0))
            .unwrap_or(0.0);

            Some(FomoSwap {
                id,
                created_at,
                spent,
                received,
                usd_value,
            })
        })
        .collect()
}

/// Parse one swap leg. `prefix` is `"in"` (`inTokenAddress`, `inHumanAmount`…) or
/// `"out"`, and the aggregated form uses `token`/`amount`.
fn parse_leg(row: &Value, prefix: &str) -> Option<FomoTokenLeg> {
    let (address_keys, symbol_keys): (Vec<String>, Vec<String>) = if prefix == "token" {
        (
            vec!["tokenAddress".into(), "address".into()],
            vec!["symbol".into(), "tokenSymbol".into()],
        )
    } else {
        (
            vec![format!("{prefix}TokenAddress"), format!("{prefix}Mint")],
            vec![format!("{prefix}TokenSymbol"), format!("{prefix}Symbol")],
        )
    };
    let address_keys: Vec<&str> = address_keys.iter().map(String::as_str).collect();
    let symbol_keys: Vec<&str> = symbol_keys.iter().map(String::as_str).collect();

    let address = string(row, &address_keys)?;
    // Nested `token: {symbol, address}` shape from the mirror's API.
    let nested = row.get("token").filter(|t| t.is_object());
    let symbol = string(row, &symbol_keys).or_else(|| {
        nested.and_then(|token| string(token, &["symbol", "name"]))
    });

    let amount_keys: Vec<String> = if prefix == "token" {
        vec!["amount".into(), "humanAmount".into()]
    } else {
        vec![
            format!("{prefix}HumanAmount"),
            format!("{prefix}Amount"),
            format!("humanAmount{}", capitalize(prefix)),
            format!("amount{}", capitalize(prefix)),
        ]
    };
    let amount_keys: Vec<&str> = amount_keys.iter().map(String::as_str).collect();

    let human_amount = number(row, &amount_keys)
        .or_else(|| nested.and_then(|token| number(token, &["amount", "humanAmount"])))
        .unwrap_or(0.0);

    // `networkId` applies to the "in" leg; the "out" leg uses `outNetworkId`.
    let network_keys: Vec<String> = if prefix == "token" {
        vec!["networkId".to_string(), "network_id".to_string()]
    } else {
        vec![
            format!("{prefix}NetworkId"),
            "networkId".to_string(),
            format!("{prefix}Network"),
        ]
    };
    let network_keys: Vec<&str> = network_keys.iter().map(String::as_str).collect();
    let network_id = string(row, &network_keys);

    Some(FomoTokenLeg {
        address,
        symbol,
        human_amount,
        network_id,
    })
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ============================================================================
// Tests — fixtures mirror the shapes captured from both providers
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn client(provider: FomoProvider) -> FomoClient {
        FomoClient::new(FomoClientConfig {
            provider,
            base_url: "http://localhost".to_string(),
            api_key: Some("test".to_string()),
            cookie: None,
            window: "24h".to_string(),
            clan_members_path: "/v2/clans/{clan_id}/members".to_string(),
            solana_network_ids: vec!["1399811149".into(), "101".into(), "solana".into()],
            timeout_secs: 5,
        })
    }

    fn pnl_keys() -> Vec<String> {
        vec!["pnl24h".into(), "pnlUsd".into(), "pnl".into()]
    }

    #[test]
    fn parses_official_clan_leaderboard_envelope() {
        let payload = serde_json::json!({
            "success": true,
            "message": "ok",
            "statusCode": 200,
            "responseObject": {
                "leaderboard": [
                    { "id": "clan-1", "name": "Conviction Capital", "pnl24h": 772254.03, "memberCount": 22 },
                    { "id": "clan-2", "name": "Control", "pnl24h": 437800.0, "memberCount": 10 }
                ]
            }
        });

        let clans = parse_clans(&payload);
        assert_eq!(clans.len(), 2);
        assert_eq!(clans[0].name, "Conviction Capital");
        assert_eq!(clans[0].member_count, Some(22));
        assert!((clans[0].pnl_usd - 772254.03).abs() < 0.01);
        assert_eq!(clans[1].id, "clan-2");
    }

    #[test]
    fn parses_official_clan_members_with_clan_field() {
        let payload = serde_json::json!({
            "success": true,
            "responseObject": {
                "members": [
                    {
                        "id": "u-1",
                        "userHandle": "MrMetavers3",
                        "displayName": "MrMetavers3",
                        "pnl24h": 454800.0,
                        "address": "A5SEXYJY4jTEi6sjMLfZs5KAP8SVFvLDPDV67GgSSZSk",
                        "clan": { "id": "clan-1", "name": "Conviction Capital", "role": "member" }
                    },
                    {
                        "id": "u-2",
                        "userHandle": "PoorGoat_",
                        "displayName": "PoorGoat",
                        "pnl24h": 88400.0
                    }
                ]
            }
        });

        let members = parse_traders(&payload, &pnl_keys());
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].handle, "MrMetavers3");
        assert_eq!(members[0].clan_id.as_deref(), Some("clan-1"));
        assert_eq!(members[0].clan_name.as_deref(), Some("Conviction Capital"));
        assert_eq!(members[1].user_id, "u-2");
        assert!(members[1].clan_id.is_none());
    }

    #[test]
    fn parses_fomoapi_trader_leaderboard_with_wallets() {
        let payload = serde_json::json!({
            "window": "24h",
            "source": "fomo",
            "count": 1,
            "traders": [
                {
                    "rank": 1,
                    "handle": "CryptoKaleo",
                    "userId": "1f08e6ab-5c73-5443-9225-bfc496cde51f",
                    "displayName": "K A L E O",
                    "pnlUsd": 151383,
                    "verified": true,
                    "wallets": { "solana": "5AhfPStn66hRYoNNDfJHSDgCH7fBbwMQZUECRrhTo62F", "evm": "0x7b4d" }
                }
            ]
        });

        let traders = parse_traders(&payload, &pnl_keys());
        assert_eq!(traders.len(), 1);
        assert_eq!(traders[0].handle, "CryptoKaleo");
        assert_eq!(traders[0].user_id, "1f08e6ab-5c73-5443-9225-bfc496cde51f");
        assert!((traders[0].pnl_usd - 151383.0).abs() < 0.01);
        assert!(traders[0].verified);
        assert_eq!(
            traders[0].profile_address.as_deref(),
            Some("5AhfPStn66hRYoNNDfJHSDgCH7fBbwMQZUECRrhTo62F")
        );
    }

    #[test]
    fn parses_official_swaps_and_builds_buy_and_sell_signals() {
        let payload = serde_json::json!({
            "responseObject": {
                "swaps": [
                    {
                        "id": "swap-1",
                        "inTokenAddress": "So11111111111111111111111111111111111111112",
                        "inNetworkId": 1399811149,
                        "inHumanAmount": 0.5,
                        "outTokenAddress": "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump",
                        "outNetworkId": 1399811149,
                        "outHumanAmount": 1234567.0,
                        "humanUsdAmountOut": 99.05,
                        "createdAt": "2026-09-23T18:04:00Z"
                    },
                    {
                        "id": "swap-2",
                        "inTokenAddress": "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump",
                        "inNetworkId": 1399811149,
                        "inHumanAmount": 500000.0,
                        "outTokenAddress": "So11111111111111111111111111111111111111112",
                        "outNetworkId": 1399811149,
                        "outHumanAmount": 0.31,
                        "humanUsdAmountOut": 74.5,
                        "createdAt": "2026-09-23T18:34:00Z"
                    }
                ]
            }
        });

        let client = client(FomoProvider::Official);
        let swaps = parse_swaps(&payload);
        assert_eq!(swaps.len(), 2);

        let buy = client.to_mirror_signal(&swaps[0]).expect("buy signal");
        assert_eq!(buy.side, SwapSide::Buy);
        assert_eq!(buy.token_address, "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump");
        assert!((buy.usd_value - 99.05).abs() < 0.01);
        assert!(buy.at.is_some());

        let sell = client.to_mirror_signal(&swaps[1]).expect("sell signal");
        assert_eq!(sell.side, SwapSide::Sell);
        assert_eq!(sell.token_address, "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump");
    }

    #[test]
    fn ignores_non_solana_swaps() {
        let payload = serde_json::json!({
            "swaps": [
                {
                    "id": "swap-evm",
                    "inTokenAddress": "0x7b4d16237683fe1765e727eadf99c6f02adf0b59",
                    "inNetworkId": 8453,
                    "outTokenAddress": "0x8226dda5f73619dedc671e09be738fa308da1944",
                    "outNetworkId": 4663,
                    "humanUsdAmountOut": 500.0
                }
            ]
        });

        let client = client(FomoProvider::Official);
        let swaps = parse_swaps(&payload);
        assert_eq!(swaps.len(), 1);
        assert!(
            client.to_mirror_signal(&swaps[0]).is_none(),
            "EVM-only swaps must not produce a Solana copy signal"
        );
    }

    #[test]
    fn parses_fomoapi_positions_shape_as_buys() {
        let payload = serde_json::json!({
            "available": true,
            "positions": [
                {
                    "tradeId": "t-1",
                    "token": { "symbol": "VBUCKS", "address": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263" },
                    "status": "open",
                    "costBasisUsd": 9.75,
                    "boughtAmount": 1000,
                    "createdAt": "2026-09-23T10:00:00Z"
                }
            ]
        });

        let client = client(FomoProvider::FomoApiIo);
        let swaps = parse_swaps(&payload);
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0].id, "t-1");

        let signal = client.to_mirror_signal(&swaps[0]).expect("signal");
        assert_eq!(signal.side, SwapSide::Buy);
        assert_eq!(signal.token_symbol.as_deref(), Some("VBUCKS"));
        assert!((signal.usd_value - 9.75).abs() < 0.01);
    }

    #[test]
    fn network_id_fallback_accepts_solana_shaped_mints() {
        // No network id anywhere: the address shape decides.
        let payload = serde_json::json!({
            "swaps": [
                {
                    "id": "swap-3",
                    "inTokenAddress": "So11111111111111111111111111111111111111112",
                    "outTokenAddress": "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump",
                    "humanUsdAmountOut": 42.0
                }
            ]
        });

        let client = client(FomoProvider::Official);
        let swaps = parse_swaps(&payload);
        let signal = client.to_mirror_signal(&swaps[0]).expect("solana-shaped signal");
        assert_eq!(signal.side, SwapSide::Buy);
    }

    #[test]
    fn amount_aliases_are_read() {
        let payload = serde_json::json!({
            "swaps": [
                {
                    "id": "swap-4",
                    "inTokenAddress": "So11111111111111111111111111111111111111112",
                    "inAmount": 1000000000,
                    "outTokenAddress": "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump",
                    "outNetworkId": "1399811149",
                    "outAmount": 5000,
                    "usdValue": 12.5
                }
            ]
        });

        let swaps = parse_swaps(&payload);
        assert!((swaps[0].received.as_ref().unwrap().human_amount - 5000.0).abs() < 0.01);
        assert!((swaps[0].usd_value - 12.5).abs() < 0.01);
    }

    #[test]
    fn single_object_profile_response_is_one_trader() {
        let payload = serde_json::json!({
            "handle": "MrMetavers3",
            "userId": "u-1",
            "displayName": "MrMetavers3",
            "pnlUsd": 454800,
            "clan": { "id": "clan-1", "name": "Conviction Capital", "icon": "x.png" }
        });

        let traders = parse_traders(&payload, &pnl_keys());
        assert_eq!(traders.len(), 1);
        assert_eq!(traders[0].clan_name.as_deref(), Some("Conviction Capital"));
    }

    #[test]
    fn provider_aliases_are_accepted() {
        assert_eq!(FomoProvider::parse("OFFICIAL"), FomoProvider::Official);
        assert_eq!(FomoProvider::parse("fomo.family"), FomoProvider::Official);
        assert_eq!(FomoProvider::parse("fomoapi.io"), FomoProvider::FomoApiIo);
        assert_eq!(FomoProvider::parse("my-proxy"), FomoProvider::Custom);
        assert_eq!(
            FomoProvider::Official.default_base_url(),
            "https://prod-api.fomo.family"
        );
        assert_eq!(FomoProvider::FomoApiIo.default_base_url(), "https://api.fomoapi.io");
    }

    #[test]
    fn solana_mint_shape_check_rejects_evm_and_short_strings() {
        assert!(looks_like_solana_mint("8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump"));
        assert!(!looks_like_solana_mint("0x8226dda5f73619dedc671e09be738fa308da1944"));
        assert!(!looks_like_solana_mint("So1111"));
        assert!(!looks_like_solana_mint("has0OIl-invalid-base58-address-here-1234567890"));
    }

    #[test]
    fn signal_describe_is_human_readable() {
        let signal = MirrorSignal {
            side: SwapSide::Buy,
            token_address: "8mCt5QnoD4izGiBncq4C2kkzPDqJNvHY9twnxiAapump".into(),
            token_symbol: Some("CATGPT".into()),
            usd_value: 99.05,
            swap_id: "swap-1".into(),
            at: None,
        };
        assert_eq!(signal.describe(), "BUY CATGPT ($99.05)");
    }
}
