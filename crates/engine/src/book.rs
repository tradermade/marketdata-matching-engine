//! Price-time priority limit order book.
//!
//! Layout:
//! - every resting order lives in a `Slab` arena (stable index, no per-order box);
//! - each price level is an intrusive doubly-linked FIFO through that arena;
//! - levels are kept in a `BTreeMap` per side, so best bid/ask is the map's
//!   last/first key;
//! - `index` maps order id -> arena slot for O(1) cancel/amend.

use std::collections::{BTreeMap, HashMap};

use slab::Slab;

use crate::types::*;

const NIL: u32 = u32::MAX;

#[derive(Debug)]
struct Node {
    id: OrderId,
    account: AccountId,
    side: Side,
    price: Price,
    qty: i64,
    prev: u32,
    next: u32,
}

#[derive(Debug)]
struct Level {
    head: u32,
    tail: u32,
    total: i64,
    count: u32,
}

impl Level {
    fn new() -> Self {
        Level { head: NIL, tail: NIL, total: 0, count: 0 }
    }
}

#[derive(Default)]
pub struct OrderBook {
    orders: Slab<Node>,
    index: HashMap<OrderId, u32>,
    bids: BTreeMap<Price, Level>,
    asks: BTreeMap<Price, Level>,
    last_trade: Option<Price>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(orders: usize) -> Self {
        OrderBook {
            orders: Slab::with_capacity(orders),
            index: HashMap::with_capacity(orders),
            ..Default::default()
        }
    }

    // ---------------------------------------------------------------- queries

    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    pub fn last_trade(&self) -> Option<Price> {
        self.last_trade
    }

    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    pub fn order(&self, id: OrderId) -> Option<OrderView> {
        let slot = *self.index.get(&id)?;
        let n = &self.orders[slot as usize];
        Some(OrderView { id: n.id, account: n.account, side: n.side, price: n.price, qty: Qty(n.qty) })
    }

    /// Top `n` aggregated levels, best first.
    pub fn depth(&self, side: Side, n: usize) -> Vec<LevelView> {
        let view = |(p, l): (&Price, &Level)| LevelView { price: *p, qty: Qty(l.total), orders: l.count };
        match side {
            Side::Buy => self.bids.iter().rev().take(n).map(view).collect(),
            Side::Sell => self.asks.iter().take(n).map(view).collect(),
        }
    }

    /// Individual orders across the top `levels` levels, in priority order.
    pub fn orders_by_priority(&self, side: Side, levels: usize) -> Vec<OrderView> {
        let mut out = Vec::new();
        let mut push_level = |l: &Level| {
            let mut cur = l.head;
            while cur != NIL {
                let n = &self.orders[cur as usize];
                out.push(OrderView { id: n.id, account: n.account, side: n.side, price: n.price, qty: Qty(n.qty) });
                cur = n.next;
            }
        };
        match side {
            Side::Buy => self.bids.values().rev().take(levels).for_each(&mut push_level),
            Side::Sell => self.asks.values().take(levels).for_each(&mut push_level),
        }
        out
    }

    /// All resting orders belonging to `account`.
    pub fn orders_of(&self, account: AccountId) -> Vec<OrderView> {
        self.orders
            .iter()
            .filter(|(_, n)| n.account == account)
            .map(|(_, n)| OrderView { id: n.id, account: n.account, side: n.side, price: n.price, qty: Qty(n.qty) })
            .collect()
    }

    /// Queue position of a resting order: how much is ahead of it at its price.
    /// Walks the level from the head, so O(orders ahead).
    pub fn queue_position(&self, id: OrderId) -> Option<QueuePosition> {
        let slot = *self.index.get(&id)?;
        let n = &self.orders[slot as usize];
        let level = match n.side {
            Side::Buy => self.bids.get(&n.price)?,
            Side::Sell => self.asks.get(&n.price)?,
        };
        let (mut cur, mut orders_ahead, mut qty_ahead) = (level.head, 0u32, 0i64);
        while cur != slot {
            let o = &self.orders[cur as usize];
            orders_ahead += 1;
            qty_ahead += o.qty;
            cur = o.next;
        }
        Some(QueuePosition { orders_ahead, qty_ahead: Qty(qty_ahead), level_qty: Qty(level.total), level_orders: level.count })
    }

    /// What an aggressive order of `qty` would do right now, without touching
    /// the book. Skips `account`'s own orders, as self-trade prevention would.
    pub fn estimate(&self, side: Side, qty: Qty, limit: Option<Price>, account: AccountId) -> SweepEstimate {
        let levels: Box<dyn Iterator<Item = (&Price, &Level)>> = match side {
            Side::Buy => Box::new(self.asks.iter()),
            Side::Sell => Box::new(self.bids.iter().rev()),
        };
        let mut est = SweepEstimate { filled: Qty(0), notional: 0, best: None, worst: None, levels: 0, available: Qty(0) };
        for (p, l) in levels {
            let mut level_avail = 0i64;
            let mut cur = l.head;
            while cur != NIL {
                let n = &self.orders[cur as usize];
                if n.account != account {
                    level_avail += n.qty;
                }
                cur = n.next;
            }
            if level_avail == 0 {
                continue;
            }
            est.available.0 += level_avail;
            let within = limit.map_or(true, |lim| price_ok(side, *p, lim));
            let need = qty.0 - est.filled.0;
            if within && need > 0 {
                let take = need.min(level_avail);
                est.filled.0 += take;
                est.notional += p.0 as i128 * take as i128;
                est.best.get_or_insert(*p);
                est.worst = Some(*p);
                est.levels += 1;
            }
        }
        est
    }

    // --------------------------------------------------------------- commands

    /// Submit a new order. Events are appended to `out` in causal order:
    /// `Accepted`, then any `Fill`s / self-trade `Cancelled`s, then a final
    /// `Cancelled{Unfilled}` if an IOC/FOK/market remainder is discarded.
    pub fn submit(&mut self, o: NewOrder, out: &mut Vec<Event>) {
        if let Some(reason) = self.validate(&o) {
            out.push(Event::Rejected { id: o.id, account: o.account, reason });
            return;
        }

        let limit = match o.kind {
            OrderKind::Limit => Some(o.price),
            OrderKind::Market => None,
        };

        if o.post_only && self.crosses(o.side, limit) {
            out.push(Event::Rejected { id: o.id, account: o.account, reason: RejectReason::PostOnlyWouldCross });
            return;
        }
        if o.tif == TimeInForce::Fok && !self.can_fill(o.side, limit, o.account, o.qty.0) {
            out.push(Event::Rejected { id: o.id, account: o.account, reason: RejectReason::FokNotFillable });
            return;
        }

        out.push(Event::Accepted { id: o.id, account: o.account, side: o.side, price: o.price, qty: o.qty });

        let leaves = self.match_incoming(&o, limit, o.qty.0, out);
        if leaves == 0 {
            return;
        }
        if o.kind == OrderKind::Limit && o.tif == TimeInForce::Gtc {
            self.insert(o.id, o.account, o.side, o.price, leaves);
        } else {
            out.push(Event::Cancelled { id: o.id, account: o.account, leaves: Qty(leaves), reason: CancelReason::Unfilled });
        }
    }

    pub fn cancel(&mut self, id: OrderId, account: AccountId, out: &mut Vec<Event>) {
        let Some(&slot) = self.index.get(&id) else {
            out.push(Event::Rejected { id, account, reason: RejectReason::UnknownOrder });
            return;
        };
        if self.orders[slot as usize].account != account {
            out.push(Event::Rejected { id, account, reason: RejectReason::NotOwner });
            return;
        }
        let n = self.remove(slot);
        out.push(Event::Cancelled { id, account, leaves: Qty(n.qty), reason: CancelReason::User });
    }

    /// Change price and/or remaining quantity.
    ///
    /// Same price with a smaller quantity amends in place and keeps queue
    /// priority. Anything else is cancel/replace under the same id and goes to
    /// the back of the queue (and may trade if the new price crosses).
    pub fn modify(&mut self, id: OrderId, account: AccountId, price: Price, qty: Qty, out: &mut Vec<Event>) {
        let Some(&slot) = self.index.get(&id) else {
            out.push(Event::Rejected { id, account, reason: RejectReason::UnknownOrder });
            return;
        };
        let (owner, side, cur_price, cur_qty) = {
            let n = &self.orders[slot as usize];
            (n.account, n.side, n.price, n.qty)
        };
        if owner != account {
            out.push(Event::Rejected { id, account, reason: RejectReason::NotOwner });
            return;
        }
        if qty.0 <= 0 {
            out.push(Event::Rejected { id, account, reason: RejectReason::InvalidQty });
            return;
        }
        if price.0 <= 0 {
            out.push(Event::Rejected { id, account, reason: RejectReason::InvalidPrice });
            return;
        }

        if price == cur_price && qty.0 <= cur_qty {
            let delta = cur_qty - qty.0;
            self.orders[slot as usize].qty = qty.0;
            self.side_mut(side).get_mut(&price).expect("level exists").total -= delta;
            out.push(Event::Amended { id, account, price, qty });
            return;
        }

        self.remove(slot);
        out.push(Event::Amended { id, account, price, qty });
        let leaves = self.match_incoming(
            &NewOrder::limit(id, account, side, price.0, qty.0),
            Some(price),
            qty.0,
            out,
        );
        if leaves > 0 {
            self.insert(id, account, side, price, leaves);
        }
    }

    // -------------------------------------------------------------- internals

    fn validate(&self, o: &NewOrder) -> Option<RejectReason> {
        if o.qty.0 <= 0 {
            return Some(RejectReason::InvalidQty);
        }
        if o.kind == OrderKind::Limit && o.price.0 <= 0 {
            return Some(RejectReason::InvalidPrice);
        }
        if self.index.contains_key(&o.id) {
            return Some(RejectReason::DuplicateOrderId);
        }
        None
    }

    fn side_mut(&mut self, side: Side) -> &mut BTreeMap<Price, Level> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }

    fn crosses(&self, side: Side, limit: Option<Price>) -> bool {
        let best = match side {
            Side::Buy => self.best_ask(),
            Side::Sell => self.best_bid(),
        };
        match (best, limit) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(b), Some(l)) => price_ok(side, b, l),
        }
    }

    /// Liquidity available to `account` (ignores its own resting orders,
    /// which self-trade prevention would cancel rather than fill).
    fn can_fill(&self, side: Side, limit: Option<Price>, account: AccountId, need: i64) -> bool {
        let levels: Box<dyn Iterator<Item = (&Price, &Level)>> = match side {
            Side::Buy => Box::new(self.asks.iter()),
            Side::Sell => Box::new(self.bids.iter().rev()),
        };
        let mut got = 0i64;
        for (p, l) in levels {
            if let Some(lim) = limit {
                if !price_ok(side, *p, lim) {
                    break;
                }
            }
            let mut cur = l.head;
            while cur != NIL {
                let n = &self.orders[cur as usize];
                if n.account != account {
                    got += n.qty;
                    if got >= need {
                        return true;
                    }
                }
                cur = n.next;
            }
        }
        false
    }

    /// Match `qty` of an incoming order against the opposite side.
    /// Returns the unfilled remainder.
    fn match_incoming(&mut self, o: &NewOrder, limit: Option<Price>, qty: i64, out: &mut Vec<Event>) -> i64 {
        let mut remaining = qty;
        while remaining > 0 {
            let entry = match o.side {
                Side::Buy => self.asks.first_entry(),
                Side::Sell => self.bids.last_entry(),
            };
            let Some(mut entry) = entry else { break };
            let px = *entry.key();
            if let Some(lim) = limit {
                if !price_ok(o.side, px, lim) {
                    break;
                }
            }

            let level = entry.get_mut();
            while remaining > 0 && level.head != NIL {
                let slot = level.head;
                let maker = &mut self.orders[slot as usize];

                if maker.account == o.account {
                    // Self-trade prevention: cancel the resting order.
                    let (mid, leaves) = (maker.id, maker.qty);
                    pop_front(&mut self.orders, level);
                    self.orders.remove(slot as usize);
                    self.index.remove(&mid);
                    out.push(Event::Cancelled { id: mid, account: o.account, leaves: Qty(leaves), reason: CancelReason::SelfTrade });
                    continue;
                }

                let fill = remaining.min(maker.qty);
                maker.qty -= fill;
                level.total -= fill;
                remaining -= fill;
                let (mid, macct, mleaves) = (maker.id, maker.account, maker.qty);

                out.push(Event::Fill {
                    maker_id: mid,
                    taker_id: o.id,
                    maker_account: macct,
                    taker_account: o.account,
                    taker_side: o.side,
                    price: px,
                    qty: Qty(fill),
                    maker_leaves: Qty(mleaves),
                    taker_leaves: Qty(remaining),
                });
                self.last_trade = Some(px);

                if mleaves == 0 {
                    // maker.qty is now 0, so pop_front leaves `total` alone.
                    pop_front(&mut self.orders, level);
                    self.orders.remove(slot as usize);
                    self.index.remove(&mid);
                }
            }
            if level.count == 0 {
                entry.remove();
            }
        }
        remaining
    }

    fn insert(&mut self, id: OrderId, account: AccountId, side: Side, price: Price, qty: i64) {
        let slot = self.orders.insert(Node { id, account, side, price, qty, prev: NIL, next: NIL }) as u32;
        self.index.insert(id, slot);
        let level = match side {
            Side::Buy => self.bids.entry(price).or_insert_with(Level::new),
            Side::Sell => self.asks.entry(price).or_insert_with(Level::new),
        };
        if level.tail == NIL {
            level.head = slot;
        } else {
            self.orders[level.tail as usize].next = slot;
            self.orders[slot as usize].prev = level.tail;
        }
        level.tail = slot;
        level.total += qty;
        level.count += 1;
    }

    /// Unlink an arbitrary resting order and free its slot.
    fn remove(&mut self, slot: u32) -> Node {
        let (side, price, prev, next, qty) = {
            let n = &self.orders[slot as usize];
            (n.side, n.price, n.prev, n.next, n.qty)
        };
        if prev != NIL {
            self.orders[prev as usize].next = next;
        }
        if next != NIL {
            self.orders[next as usize].prev = prev;
        }
        let book = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = book.get_mut(&price).expect("level exists");
        if level.head == slot {
            level.head = next;
        }
        if level.tail == slot {
            level.tail = prev;
        }
        level.total -= qty;
        level.count -= 1;
        if level.count == 0 {
            book.remove(&price);
        }
        let node = self.orders.remove(slot as usize);
        self.index.remove(&node.id);
        node
    }

    /// Debug check of all structural invariants. Used by tests.
    #[doc(hidden)]
    pub fn check_invariants(&self) {
        if let (Some(b), Some(a)) = (self.best_bid(), self.best_ask()) {
            assert!(b < a, "book crossed: bid {b:?} >= ask {a:?}");
        }
        let mut seen = 0usize;
        for (side, book) in [(Side::Buy, &self.bids), (Side::Sell, &self.asks)] {
            for (p, l) in book {
                assert!(l.count > 0, "empty level left in book");
                let (mut cur, mut prev, mut total, mut count) = (l.head, NIL, 0i64, 0u32);
                while cur != NIL {
                    let n = &self.orders[cur as usize];
                    assert_eq!(n.side, side);
                    assert_eq!(n.price, *p);
                    assert_eq!(n.prev, prev);
                    assert!(n.qty > 0);
                    assert_eq!(self.index[&n.id], cur);
                    total += n.qty;
                    count += 1;
                    prev = cur;
                    cur = n.next;
                }
                assert_eq!(l.tail, prev);
                assert_eq!(l.total, total, "level total mismatch at {p:?}");
                assert_eq!(l.count, count);
                seen += count as usize;
            }
        }
        assert_eq!(seen, self.orders.len());
        assert_eq!(seen, self.index.len());
    }
}

/// Is a resting price `px` acceptable for an incoming `side` order limited at `lim`?
#[inline]
fn price_ok(side: Side, px: Price, lim: Price) -> bool {
    match side {
        Side::Buy => px <= lim,
        Side::Sell => px >= lim,
    }
}

/// Unlink the head of `level`, subtracting its remaining quantity from the
/// level total. Does not free the arena slot.
#[inline]
fn pop_front(orders: &mut Slab<Node>, level: &mut Level) {
    let slot = level.head;
    let (next, qty) = {
        let n = &orders[slot as usize];
        (n.next, n.qty)
    };
    level.head = next;
    if next == NIL {
        level.tail = NIL;
    } else {
        orders[next as usize].prev = NIL;
    }
    level.total -= qty;
    level.count -= 1;
}
