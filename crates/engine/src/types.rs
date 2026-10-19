//! Core value types. Prices and quantities are fixed-point integers so the
//! matching path never touches floating point.

use serde::{Deserialize, Serialize};

pub type OrderId = u64;
pub type AccountId = u32;

/// Price in ticks (`real = ticks / 10^price_decimals`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Price(pub i64);

/// Quantity in lots (`real = lots / 10^qty_decimals`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Qty(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderKind {
    Limit,
    Market,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TimeInForce {
    /// Good till cancelled: unfilled remainder rests on the book.
    Gtc,
    /// Immediate or cancel: fill what you can, cancel the rest.
    Ioc,
    /// Fill or kill: fill completely right now or do nothing.
    Fok,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewOrder {
    pub id: OrderId,
    pub account: AccountId,
    pub side: Side,
    pub kind: OrderKind,
    /// Limit price. Ignored for market orders.
    pub price: Price,
    pub qty: Qty,
    pub tif: TimeInForce,
    /// Reject instead of taking liquidity.
    pub post_only: bool,
}

impl NewOrder {
    pub fn limit(id: OrderId, account: AccountId, side: Side, price: i64, qty: i64) -> Self {
        NewOrder {
            id,
            account,
            side,
            kind: OrderKind::Limit,
            price: Price(price),
            qty: Qty(qty),
            tif: TimeInForce::Gtc,
            post_only: false,
        }
    }

    pub fn market(id: OrderId, account: AccountId, side: Side, qty: i64) -> Self {
        NewOrder {
            id,
            account,
            side,
            kind: OrderKind::Market,
            price: Price(0),
            qty: Qty(qty),
            tif: TimeInForce::Ioc,
            post_only: false,
        }
    }

    pub fn with_tif(mut self, tif: TimeInForce) -> Self {
        self.tif = tif;
        self
    }

    pub fn post_only(mut self) -> Self {
        self.post_only = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RejectReason {
    InvalidQty,
    InvalidPrice,
    DuplicateOrderId,
    PostOnlyWouldCross,
    FokNotFillable,
    UnknownOrder,
    NotOwner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CancelReason {
    /// Explicit cancel request.
    User,
    /// IOC / market remainder that could not be filled.
    Unfilled,
    /// Resting order removed by self-trade prevention.
    SelfTrade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Event {
    Accepted {
        id: OrderId,
        account: AccountId,
        side: Side,
        price: Price,
        qty: Qty,
    },
    Rejected {
        id: OrderId,
        account: AccountId,
        reason: RejectReason,
    },
    Fill {
        maker_id: OrderId,
        taker_id: OrderId,
        maker_account: AccountId,
        taker_account: AccountId,
        /// Side of the aggressor; a BUY taker lifts the offer.
        taker_side: Side,
        price: Price,
        qty: Qty,
        maker_leaves: Qty,
        taker_leaves: Qty,
    },
    Cancelled {
        id: OrderId,
        account: AccountId,
        leaves: Qty,
        reason: CancelReason,
    },
    /// Order amended in place (quantity reduced, priority kept).
    Amended {
        id: OrderId,
        account: AccountId,
        price: Price,
        qty: Qty,
    },
}

/// Read-only view of a resting order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct OrderView {
    pub id: OrderId,
    pub account: AccountId,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
}

/// One aggregated price level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LevelView {
    pub price: Price,
    pub qty: Qty,
    pub orders: u32,
}

/// Conversion between real numbers and the fixed-point representation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolSpec {
    pub symbol: String,
    pub price_decimals: u32,
    pub qty_decimals: u32,
}

impl SymbolSpec {
    pub fn new(symbol: &str, price_decimals: u32, qty_decimals: u32) -> Self {
        SymbolSpec {
            symbol: symbol.to_string(),
            price_decimals,
            qty_decimals,
        }
    }

    pub fn price_from_f64(&self, v: f64) -> Price {
        Price((v * 10f64.powi(self.price_decimals as i32)).round() as i64)
    }

    pub fn price_to_f64(&self, p: Price) -> f64 {
        p.0 as f64 / 10f64.powi(self.price_decimals as i32)
    }

    pub fn qty_from_f64(&self, v: f64) -> Qty {
        Qty((v * 10f64.powi(self.qty_decimals as i32)).round() as i64)
    }

    pub fn qty_to_f64(&self, q: Qty) -> f64 {
        q.0 as f64 / 10f64.powi(self.qty_decimals as i32)
    }
}

/// Where a resting order sits in its price level's FIFO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct QueuePosition {
    /// 0 = first in line.
    pub orders_ahead: u32,
    pub qty_ahead: Qty,
    /// Everything resting at this price, including the order itself.
    pub level_qty: Qty,
    pub level_orders: u32,
}

/// Result of walking the book for a hypothetical aggressive order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SweepEstimate {
    /// Quantity that would fill (may be less than requested).
    pub filled: Qty,
    /// Sum of price * qty in ticks*lots, for an exact average.
    pub notional: i128,
    pub best: Option<Price>,
    pub worst: Option<Price>,
    pub levels: u32,
    /// All opposite-side quantity available to this account (no price limit).
    pub available: Qty,
}
