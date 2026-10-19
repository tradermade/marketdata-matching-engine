//! Messages between the async world (HTTP, WebSocket, feed) and the engine thread.

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tm_engine::{AccountId, OrderId, OrderKind, Side, TimeInForce};
use tokio::sync::oneshot;

pub type Reply<T> = oneshot::Sender<T>;

#[derive(Debug, Deserialize)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    #[serde(rename = "type", default = "default_kind")]
    pub kind: OrderKind,
    #[serde(default)]
    pub price: Option<f64>,
    pub qty: f64,
    #[serde(default = "default_tif")]
    pub tif: TimeInForce,
    #[serde(default)]
    pub post_only: bool,
}

fn default_kind() -> OrderKind {
    OrderKind::Limit
}

fn default_tif() -> TimeInForce {
    TimeInForce::Gtc
}

#[derive(Debug, Deserialize)]
pub struct ModifyRequest {
    pub price: f64,
    pub qty: f64,
}

/// FIX-style execution report sent to the order's owner.
#[derive(Debug, Clone, Serialize)]
pub struct ExecReport {
    pub order_id: OrderId,
    pub symbol: String,
    pub side: Side,
    /// NEW, PARTIAL_FILL, FILL, CANCELED, REPLACED, REJECTED
    pub exec_type: &'static str,
    /// The order's limit price; 0 for a market order.
    pub price: f64,
    pub last_px: f64,
    pub last_qty: f64,
    pub cum_qty: f64,
    pub leaves_qty: f64,
    pub avg_px: f64,
    pub text: Option<String>,
    pub ts: i64,
}

#[derive(Debug, Serialize)]
pub struct OrderAck {
    pub order_id: OrderId,
    pub reports: Vec<ExecReport>,
    /// Time spent inside the matching engine for this command.
    pub engine_ns: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub account_id: AccountId,
    pub name: String,
    pub token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedSource {
    Tradermade,
    Synthetic,
}

/// One normalised market data update.
#[derive(Debug)]
pub struct MarketTick {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub bid_vol: Option<f64>,
    pub ask_vol: Option<f64>,
    /// Provider timestamp, ms since epoch.
    pub ts_ms: i64,
    pub source: FeedSource,
    pub recv_at: Instant,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FeedStatus {
    pub mode: String,
    pub connected: bool,
    pub accepted: Vec<String>,
    /// Symbol -> reason (`cfds_not_allowed`, `limit_reached`, `toggle_cooldown`, ...).
    pub denied: std::collections::BTreeMap<String, String>,
    /// Symbols TraderMade doesn't recognise in this form.
    pub invalid: Vec<String>,
    pub message: Option<String>,
}

pub enum Command {
    Session { name: String, reply: Reply<SessionInfo> },
    Submit { account: AccountId, req: OrderRequest, reply: Reply<Result<OrderAck, String>> },
    Cancel { account: AccountId, order_id: OrderId, reply: Reply<Result<OrderAck, String>> },
    Modify { account: AccountId, order_id: OrderId, req: ModifyRequest, reply: Reply<Result<OrderAck, String>> },
    MarketData(MarketTick),
    Feed(FeedStatus),
    /// Everything a freshly connected client needs, as ready-to-send JSON.
    Snapshot { account: Option<AccountId>, reply: Reply<Vec<Arc<str>>> },
    Account { account: AccountId, reply: Reply<serde_json::Value> },
    Backfill { symbol: String, bars: Vec<HistBar> },
    Estimate { account: AccountId, req: EstimateRequest, reply: Reply<Result<serde_json::Value, String>> },
    /// List a new symbol. Replies "listed", "pending" (already requested) or
    /// "requested" (new: the caller should ask the feed to subscribe).
    AddSymbol { symbol: String, reply: Reply<&'static str> },
    /// Take an added symbol off the list. Refused for built-in symbols and
    /// while any account has open orders in it.
    RemoveSymbol { symbol: String, reply: Reply<Result<(), String>> },
    /// Every symbol the streaming key may subscribe to (TraderMade `symbols` action).
    SymbolList(Vec<String>),
}

/// Requests from the API to the feed task.
#[derive(Debug)]
pub enum FeedCtl {
    Subscribe(String),
    Unsubscribe(String),
}

#[derive(Debug, Deserialize)]
pub struct EstimateRequest {
    pub symbol: String,
    pub side: Side,
    pub qty: f64,
    /// Optional limit; omit for a market order.
    #[serde(default)]
    pub price: Option<f64>,
}

/// One historical bar from the REST API, `t` = bar open in ms since epoch.
#[derive(Debug)]
pub struct HistBar {
    pub t: i64,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
}

/// A pre-serialised message for WebSocket clients. `account: None` goes to
/// everyone, `Some(id)` only to that account's sockets.
#[derive(Clone, Debug)]
pub struct Outbound {
    pub account: Option<AccountId>,
    pub json: Arc<str>,
}
