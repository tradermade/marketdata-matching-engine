//! The engine thread. It alone owns every order book, account and metric, so
//! the matching path needs no locks. Async code talks to it over a channel
//! and receives pre-serialised JSON on a broadcast channel.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use hdrhistogram::Histogram;
use serde_json::{json, Value};
use tm_engine::*;
use tokio::sync::broadcast;

use crate::config::Instrument;
use crate::messages::*;

/// Account id of the market maker that quotes TraderMade's best bid and ask.
pub const LP: AccountId = 0;

const BOOK_LEVELS: usize = 15;
const MAX_TRADES: usize = 60;
const MAX_CANDLES: usize = 240;
const MAX_EXECS: usize = 100;
const MAX_LOGS: usize = 200;
const HEAT_LEN: usize = 24;
const OPS: [&str; 5] = ["BUY", "SELL", "CANCEL", "MODIFY", "LP_SYNC"];

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Copy, Default)]
struct Quote {
    bid: f64,
    ask: f64,
    bid_vol: Option<f64>,
    ask_vol: Option<f64>,
    ts_ms: i64,
    source: Option<FeedSource>,
}

#[derive(Clone, Copy, serde::Serialize)]
struct Candle {
    t: i64,
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    /// TraderMade quote updates in this bar (FX "tick volume").
    ticks: u32,
    /// Quantity traded on this exchange in this bar.
    volume: f64,
}

#[derive(Clone, serde::Serialize)]
struct TradeMsg {
    id: u64,
    price: f64,
    qty: f64,
    side: Side,
    ts: i64,
    taker_is_lp: bool,
    maker_is_lp: bool,
}

/// Book-keeping for a user (non-LP) order.
struct UserOrder {
    sym: usize,
    account: AccountId,
    side: Side,
    /// Limit price; 0 for a market order.
    px: f64,
    cum: i64,
    notional: f64,
    /// TraderMade mid when the order reached the engine (TCA benchmark).
    arrival_mid: f64,
}

/// Markout horizons for execution-quality analysis.
const MARKOUTS_MS: [u64; 3] = [1_000, 5_000, 60_000];
const MAX_FILLS: usize = 200;

/// One of the account's fills, with transaction-cost analysis attached.
#[derive(Clone, serde::Serialize)]
struct FillRecord {
    fill_id: u64,
    order_id: OrderId,
    symbol: String,
    side: Side,
    /// TAKER (crossed the spread) or MAKER (was resting and got hit).
    liquidity: &'static str,
    px: f64,
    qty: f64,
    ts: i64,
    arrival_mid: f64,
    mid_at_fill: f64,
    /// Cost against arrival mid, bps. Positive = paid more than mid.
    slippage_bps: f64,
    /// Price move after the fill at +1s, +5s, +60s, bps. Positive = in your favour.
    markouts_bps: [Option<f64>; 3],
}

#[derive(Default, Clone, serde::Serialize)]
struct Position {
    qty: f64,
    avg_px: f64,
    realized: f64,
}

struct Account {
    name: String,
    positions: HashMap<usize, Position>,
    execs: VecDeque<ExecReport>,
    fills: VecDeque<FillRecord>,
}

#[derive(Default)]
struct Lp {
    /// Resting LP order per (side, price) level.
    levels: HashMap<(Side, Price), OrderId>,
    /// Quotes whose size is a default because TraderMade sent none
    /// (e.g. the last price of a closed market).
    estimated: HashSet<(Side, Price)>,
}

struct Metrics {
    hist: HashMap<&'static str, Histogram<u64>>,
    heat: HashMap<&'static str, VecDeque<f64>>,
    ops_this_sec: u64,
    ops_per_sec: u64,
    ticks_this_sec: u64,
    ticks_per_sec: u64,
    /// Feed receive -> book updated, for TraderMade ticks.
    tick_to_book: Histogram<u64>,
}

impl Metrics {
    fn new() -> Self {
        let h = || Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
        Metrics {
            hist: OPS.iter().map(|o| (*o, h())).collect(),
            heat: OPS.iter().map(|o| (*o, VecDeque::with_capacity(HEAT_LEN))).collect(),
            ops_this_sec: 0,
            ops_per_sec: 0,
            ticks_this_sec: 0,
            ticks_per_sec: 0,
            tick_to_book: h(),
        }
    }

    fn record(&mut self, op: &'static str, ns: u64) {
        self.hist.get_mut(op).unwrap().saturating_record(ns.max(1));
        let heat = self.heat.get_mut(op).unwrap();
        if heat.len() == HEAT_LEN {
            heat.pop_front();
        }
        heat.push_back(ns as f64 / 1000.0);
        self.ops_this_sec += 1;
    }
}

pub struct Engine {
    inst: Vec<Instrument>,
    sym_idx: HashMap<String, usize>,
    books: Vec<OrderBook>,
    lps: Vec<Lp>,
    lp_ids: HashMap<OrderId, (usize, Side, Price)>,
    quotes: Vec<Quote>,
    candles: Vec<VecDeque<Candle>>,
    trades: Vec<VecDeque<TradeMsg>>,
    orders: HashMap<OrderId, UserOrder>,
    accounts: HashMap<AccountId, Account>,
    logs: VecDeque<String>,
    metrics: Metrics,
    feed: FeedStatus,
    out: broadcast::Sender<Outbound>,
    next_order: OrderId,
    next_account: AccountId,
    next_trade: u64,
    seq: u64,
    dirty_books: Vec<bool>,
    dirty_accounts: Vec<AccountId>,
    /// Per horizon, fills waiting for their markout: (due, account, fill_id, sym).
    markout_q: [VecDeque<(Instant, AccountId, u64, usize)>; 3],
    next_fill: u64,
    events: Vec<Event>,
    started: Instant,
    /// Symbols requested at runtime, waiting for TraderMade to accept them (or a first quote).
    pending: HashSet<String>,
    /// Every symbol the streaming key can subscribe to.
    available: Vec<String>,
    /// Chart history that arrived before its symbol was listed.
    pending_bars: HashMap<String, Vec<HistBar>>,
    /// Added symbols taken off the list. Their slots stay, so indices never shift.
    hidden: HashSet<usize>,
}

impl Engine {
    pub fn new(inst: Vec<Instrument>, out: broadcast::Sender<Outbound>) -> Self {
        let n = inst.len();
        Engine {
            sym_idx: inst.iter().enumerate().map(|(i, s)| (s.spec.symbol.clone(), i)).collect(),
            books: (0..n).map(|_| OrderBook::with_capacity(4096)).collect(),
            lps: (0..n).map(|_| Lp::default()).collect(),
            lp_ids: HashMap::new(),
            quotes: vec![Quote::default(); n],
            candles: vec![VecDeque::new(); n],
            trades: vec![VecDeque::new(); n],
            orders: HashMap::new(),
            accounts: HashMap::new(),
            logs: VecDeque::new(),
            metrics: Metrics::new(),
            feed: FeedStatus { mode: "starting".into(), ..Default::default() },
            out,
            next_order: 1,
            next_account: 1,
            next_trade: 1,
            seq: 0,
            dirty_books: vec![false; n],
            dirty_accounts: Vec::new(),
            markout_q: Default::default(),
            next_fill: 1,
            events: Vec::with_capacity(256),
            started: Instant::now(),
            pending: HashSet::new(),
            available: Vec::new(),
            pending_bars: HashMap::new(),
            hidden: HashSet::new(),
            inst,
        }
    }

    /// Run until every command sender is dropped.
    pub fn run(mut self, rx: Receiver<Command>) {
        self.log("INIT | engine thread started".into());
        let publish_every = Duration::from_millis(100);
        let mut next_publish = Instant::now() + publish_every;
        let mut next_second = Instant::now() + Duration::from_secs(1);

        loop {
            let timeout = next_publish.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let now = Instant::now();
            self.due_markouts(now);
            if now >= next_publish {
                self.publish_dirty();
                next_publish = now + publish_every;
            }
            if now >= next_second {
                self.metrics.ops_per_sec = std::mem::take(&mut self.metrics.ops_this_sec);
                self.metrics.ticks_per_sec = std::mem::take(&mut self.metrics.ticks_this_sec);
                let m = self.metrics_json();
                self.send(None, m);
                next_second = now + Duration::from_secs(1);
            }
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Session { name, reply } => {
                let _ = reply.send(self.create_session(name));
            }
            Command::Submit { account, req, reply } => {
                let _ = reply.send(self.submit(account, req));
            }
            Command::Cancel { account, order_id, reply } => {
                let _ = reply.send(self.cancel(account, order_id));
            }
            Command::Modify { account, order_id, req, reply } => {
                let _ = reply.send(self.modify(account, order_id, req));
            }
            Command::MarketData(tick) => self.on_tick(tick),
            Command::Feed(status) => {
                // A requested symbol TraderMade won't stream is dropped, with the reason.
                let rejected: Vec<(String, String)> = self
                    .pending
                    .iter()
                    .filter_map(|s| {
                        if status.invalid.contains(s) {
                            Some((s.clone(), "not a TraderMade symbol in this form".to_string()))
                        } else {
                            status.denied.get(s).filter(|r| *r != "toggle_cooldown").map(|r| (s.clone(), r.clone()))
                        }
                    })
                    .collect();
                for (s, why) in rejected {
                    self.pending.remove(&s);
                    self.log(format!("SYMBOL | {s} not added: {why}"));
                }
                // Accepted is enough to list it: a closed market may send no
                // price for hours. The first quote fills in the real settings.
                let accepted: Vec<String> = self.pending.iter().filter(|s| status.accepted.contains(s)).cloned().collect();
                for s in accepted {
                    self.add_instrument(&s, crate::config::instrument_unpriced(&s));
                }
                self.log(format!(
                    "FEED | mode={} connected={} {}",
                    status.mode,
                    status.connected,
                    status.message.clone().unwrap_or_default()
                ));
                self.feed = status;
                let msg = json!({"type": "feed", "feed": self.feed});
                self.send(None, msg);
            }
            Command::Snapshot { account, reply } => {
                let _ = reply.send(self.snapshot(account));
            }
            Command::Account { account, reply } => {
                let _ = reply.send(self.account_json(account));
            }
            Command::Backfill { symbol, bars } => self.backfill(&symbol, bars),
            Command::Estimate { account, req, reply } => {
                let _ = reply.send(self.estimate(account, req));
            }
            Command::AddSymbol { symbol, reply } => {
                let idx = self.sym_idx.get(&symbol).copied();
                let status = if let Some(sym) = idx.filter(|s| self.hidden.remove(s)) {
                    // removed earlier: list it again (the caller resubscribes)
                    self.log(format!("SYMBOL | {} listed again", self.inst[sym].spec.symbol));
                    self.instruments_changed();
                    "requested"
                } else if idx.is_some() {
                    "listed"
                } else if self.pending.insert(symbol.clone()) {
                    self.log(format!("SYMBOL | {symbol} requested; waiting for TraderMade's reply"));
                    "requested"
                } else {
                    "pending"
                };
                let _ = reply.send(status);
            }
            Command::RemoveSymbol { symbol, reply } => {
                let _ = reply.send(self.remove_symbol(&symbol));
            }
            Command::SymbolList(list) => {
                self.available = list;
                let msg = json!({"type": "symbols", "symbols": self.available});
                self.send(None, msg);
            }
        }
    }

    /// List a new instrument and tell every client.
    fn add_instrument(&mut self, symbol: &str, inst: Instrument) -> usize {
        self.pending.remove(symbol);
        let sym = self.inst.len();
        if inst.awaiting_price {
            self.log(format!("SYMBOL | {symbol} accepted by TraderMade; no price yet"));
        } else {
            self.log(format!("SYMBOL | {symbol} added ({} price decimals)", inst.spec.price_decimals));
        }
        self.sym_idx.insert(symbol.to_string(), sym);
        self.inst.push(inst);
        self.books.push(OrderBook::with_capacity(4096));
        self.lps.push(Lp::default());
        self.quotes.push(Quote::default());
        self.candles.push(VecDeque::new());
        self.trades.push(VecDeque::new());
        self.dirty_books.push(false);
        self.instruments_changed();
        if let Some(bars) = self.pending_bars.remove(symbol) {
            self.backfill(symbol, bars);
        }
        sym
    }

    /// Take an added symbol off the list: pull the market maker's quotes and
    /// stop showing it. Its slot stays, so adding it again lists it at once.
    fn remove_symbol(&mut self, symbol: &str) -> Result<(), String> {
        if self.pending.remove(symbol) {
            self.pending_bars.remove(symbol);
            self.log(format!("SYMBOL | {symbol} request withdrawn"));
            return Ok(());
        }
        let sym = self.listed(symbol).ok_or_else(|| format!("{symbol} is not listed"))?;
        if !self.inst[sym].added {
            return Err(format!("{symbol} is built in and can't be removed"));
        }
        if self.orders.values().any(|o| o.sym == sym) {
            return Err(format!("{symbol} has open orders; cancel them first"));
        }
        self.sync_lp(sym, &[], &[]);
        self.hidden.insert(sym);
        self.log(format!("SYMBOL | {symbol} removed"));
        self.instruments_changed();
        Ok(())
    }

    /// Index of a symbol that is listed and not removed.
    fn listed(&self, symbol: &str) -> Option<usize> {
        self.sym_idx.get(symbol).copied().filter(|s| !self.hidden.contains(s))
    }

    fn visible(&self) -> Vec<&Instrument> {
        self.inst.iter().enumerate().filter(|(i, _)| !self.hidden.contains(i)).map(|(_, x)| x).collect()
    }

    /// Tell every client the new list, and save the added symbols so they
    /// come back after a restart.
    fn instruments_changed(&mut self) {
        let msg = json!({"type": "instruments", "instruments": self.visible()});
        let mut added: Vec<String> = self.visible().iter().filter(|i| i.added).map(|i| i.spec.symbol.clone()).collect();
        added.extend(self.pending.iter().cloned());
        self.send(None, msg);
        if !cfg!(test) {
            crate::config::save_added_symbols(&added);
        }
    }

    // ------------------------------------------------------------- sessions

    fn create_session(&mut self, name: String) -> SessionInfo {
        let id = self.next_account;
        self.next_account += 1;
        let token: String = {
            use rand::Rng;
            let mut r = rand::thread_rng();
            (0..32).map(|_| format!("{:x}", r.gen::<u8>() & 0xf)).collect()
        };
        let name = if name.trim().is_empty() { format!("trader{id}") } else { name.trim().chars().take(24).collect() };
        self.accounts.insert(
            id,
            Account { name: name.clone(), positions: HashMap::new(), execs: VecDeque::new(), fills: VecDeque::new() },
        );
        self.log(format!("SESSION | account {id} '{name}' created"));
        SessionInfo { account_id: id, name, token }
    }

    // --------------------------------------------------------------- orders

    fn submit(&mut self, account: AccountId, req: OrderRequest) -> Result<OrderAck, String> {
        let sym = self.listed(&req.symbol).ok_or_else(|| format!("unknown symbol {}", req.symbol))?;
        let inst = &self.inst[sym];
        if inst.awaiting_price || self.quotes[sym].bid <= 0.0 {
            return Err(format!("No price for {} yet; its market may be closed", req.symbol));
        }
        if !(req.qty > 0.0) || req.qty > inst.max_order_qty {
            return Err(format!("qty must be in (0, {}]", inst.max_order_qty));
        }
        let qty = inst.spec.qty_from_f64(req.qty);
        if qty.0 <= 0 {
            return Err("qty below minimum lot".into());
        }
        let price = match req.kind {
            OrderKind::Limit => {
                let p = req.price.ok_or("limit order needs a price")?;
                inst.spec.price_from_f64(p)
            }
            OrderKind::Market => Price(0),
        };

        let id = self.next_order;
        self.next_order += 1;
        let arrival_mid = self.mid(sym);
        self.orders.insert(id, UserOrder { sym, account, side: req.side, px: if req.kind == OrderKind::Limit { inst.spec.price_to_f64(price) } else { 0.0 }, cum: 0, notional: 0.0, arrival_mid });
        let order = NewOrder {
            id,
            account,
            side: req.side,
            kind: req.kind,
            price,
            qty,
            tif: if req.kind == OrderKind::Market { TimeInForce::Ioc } else { req.tif },
            post_only: req.post_only,
        };
        self.seq += 1;
        self.log(format!(
            "SEQ:{} | INGRESS | {} {:?} {:?} {} {} @ {} [#{id} acct {account}]",
            self.seq,
            req.symbol,
            req.side,
            req.kind,
            if req.post_only { "POST_ONLY" } else { "" },
            req.qty,
            req.price.map(|p| p.to_string()).unwrap_or_else(|| "MKT".into())
        ));

        self.events.clear();
        let t0 = Instant::now();
        self.books[sym].submit(order, &mut self.events);
        let ns = t0.elapsed().as_nanos() as u64;
        self.metrics.record(if req.side == Side::Buy { "BUY" } else { "SELL" }, ns);

        let reports = self.apply_events(sym, id);
        Ok(OrderAck { order_id: id, reports, engine_ns: ns })
    }

    fn cancel(&mut self, account: AccountId, id: OrderId) -> Result<OrderAck, String> {
        let sym = self.orders.get(&id).filter(|o| o.account == account).map(|o| o.sym).ok_or("unknown order")?;
        self.seq += 1;
        self.log(format!("SEQ:{} | CANCEL | #{id} acct {account}", self.seq));
        self.events.clear();
        let t0 = Instant::now();
        self.books[sym].cancel(id, account, &mut self.events);
        let ns = t0.elapsed().as_nanos() as u64;
        self.metrics.record("CANCEL", ns);
        let reports = self.apply_events(sym, id);
        Ok(OrderAck { order_id: id, reports, engine_ns: ns })
    }

    fn modify(&mut self, account: AccountId, id: OrderId, req: ModifyRequest) -> Result<OrderAck, String> {
        let sym = self.orders.get(&id).filter(|o| o.account == account).map(|o| o.sym).ok_or("unknown order")?;
        let inst = &self.inst[sym];
        if !(req.qty > 0.0) || req.qty > inst.max_order_qty {
            return Err(format!("qty must be in (0, {}]", inst.max_order_qty));
        }
        let (price, qty) = (inst.spec.price_from_f64(req.price), inst.spec.qty_from_f64(req.qty));
        self.seq += 1;
        self.log(format!("SEQ:{} | MODIFY | #{id} -> {} @ {}", self.seq, req.qty, req.price));
        let px = self.inst[sym].spec.price_to_f64(price);
        if let Some(o) = self.orders.get_mut(&id) {
            o.px = px;
        }
        self.events.clear();
        let t0 = Instant::now();
        self.books[sym].modify(id, account, price, qty, &mut self.events);
        let ns = t0.elapsed().as_nanos() as u64;
        self.metrics.record("MODIFY", ns);
        let reports = self.apply_events(sym, id);
        Ok(OrderAck { order_id: id, reports, engine_ns: ns })
    }

    // ---------------------------------------------------------- market data

    fn on_tick(&mut self, t: MarketTick) {
        if !(t.bid > 0.0 && t.ask > 0.0) {
            return;
        }
        let sym = if let Some(s) = self.sym_idx.get(&t.symbol).copied() {
            if self.hidden.contains(&s) {
                return; // removed; ticks still in flight before the unsubscribe
            }
            s
        } else if self.pending.contains(&t.symbol) && t.ask > t.bid {
            self.add_instrument(&t.symbol, crate::config::instrument_for(&t.symbol, t.bid, t.ask))
        } else {
            return;
        };
        if self.inst[sym].awaiting_price && t.ask > t.bid {
            // First price for a symbol listed on acceptance: size it properly.
            // No orders can exist yet (they're refused without a price).
            self.inst[sym] = crate::config::instrument_for(&t.symbol, t.bid, t.ask);
            self.log(format!("SYMBOL | {} first price ({} price decimals)", t.symbol, self.inst[sym].spec.price_decimals));
            self.instruments_changed();
        }
        // The market maker quotes TraderMade's screened best bid and ask at the
        // sizes TraderMade sends, and nothing behind them: a made-up level is a
        // price someone can be filled at.
        let inst = &self.inst[sym];
        let spec = &inst.spec;
        let size = |v: Option<f64>| spec.qty_from_f64(v.filter(|v| *v > 0.0).unwrap_or(inst.default_level_qty));
        let bid = (spec.price_from_f64(t.bid), size(t.bid_vol));
        let ask = (spec.price_from_f64(t.ask), size(t.ask_vol));
        if bid.0 >= ask.0 || bid.1 .0 <= 0 || ask.1 .0 <= 0 {
            return; // crossed/locked after rounding, or below the minimum size
        }

        // A size TraderMade didn't send (e.g. the last price of a closed market)
        // is a default, and is marked as such.
        let est = &mut self.lps[sym].estimated;
        est.clear();
        if t.bid_vol.filter(|v| *v > 0.0).is_none() {
            est.insert((Side::Buy, bid.0));
        }
        if t.ask_vol.filter(|v| *v > 0.0).is_none() {
            est.insert((Side::Sell, ask.0));
        }

        self.quotes[sym] = Quote {
            bid: t.bid,
            ask: t.ask,
            bid_vol: t.bid_vol,
            ask_vol: t.ask_vol,
            ts_ms: t.ts_ms,
            source: Some(t.source),
        };
        self.metrics.ticks_this_sec += 1;
        self.update_candle(sym, (t.bid + t.ask) / 2.0, 0.0, true, t.ts_ms);

        let t0 = Instant::now();
        self.sync_lp(sym, &[bid], &[ask]);
        let ns = t0.elapsed().as_nanos() as u64;
        self.metrics.record("LP_SYNC", ns);
        if t.source == FeedSource::Tradermade {
            self.metrics.tick_to_book.saturating_record(t.recv_at.elapsed().as_nanos() as u64);
        }
        self.dirty_books[sym] = true;
    }

    /// Diff the LP's resting orders against the quotes it wants: untouched
    /// levels keep their queue position, changed ones are amended, missing
    /// ones are cancelled, new ones are added. New LP quotes can trade with
    /// resting user orders the market has moved through.
    fn sync_lp(&mut self, sym: usize, bids: &[(Price, Qty)], asks: &[(Price, Qty)]) {
        let mut want: HashMap<(Side, Price), Qty> = HashMap::with_capacity(bids.len() + asks.len());
        for &(p, q) in bids {
            want.insert((Side::Buy, p), q);
        }
        for &(p, q) in asks {
            want.insert((Side::Sell, p), q);
        }

        // 1. cancel stale levels first so new quotes never meet old LP quotes
        let stale: Vec<(Side, Price)> = self.lps[sym].levels.keys().filter(|k| !want.contains_key(k)).copied().collect();
        for key in stale {
            let id = self.lps[sym].levels.remove(&key).unwrap();
            self.lp_ids.remove(&id);
            self.events.clear();
            self.books[sym].cancel(id, LP, &mut self.events);
        }

        // 2. amend or add, inside-out so the touch updates first
        for &(side, levels) in &[(Side::Buy, bids), (Side::Sell, asks)] {
            for &(price, qty) in levels {
                self.events.clear();
                match self.lps[sym].levels.get(&(side, price)).copied() {
                    Some(id) => {
                        let cur = self.books[sym].order(id).map(|o| o.qty);
                        match cur {
                            Some(c) if c == qty => continue,
                            Some(_) => self.books[sym].modify(id, LP, price, qty, &mut self.events),
                            None => {
                                // filled since last tick; requote
                                self.lp_ids.remove(&id);
                                self.lps[sym].levels.remove(&(side, price));
                                self.lp_add(sym, side, price, qty);
                            }
                        }
                    }
                    None => self.lp_add(sym, side, price, qty),
                }
                self.apply_events(sym, 0);
            }
        }
    }

    fn lp_add(&mut self, sym: usize, side: Side, price: Price, qty: Qty) {
        let id = self.next_order;
        self.next_order += 1;
        self.lps[sym].levels.insert((side, price), id);
        self.lp_ids.insert(id, (sym, side, price));
        self.books[sym].submit(NewOrder::limit(id, LP, side, price.0, qty.0), &mut self.events);
    }

    // ---------------------------------------------------------------- events

    /// Turn engine events into trades, positions and execution reports.
    /// Returns the reports for `focus` (the order the caller acted on).
    fn apply_events(&mut self, sym: usize, focus: OrderId) -> Vec<ExecReport> {
        // Fast path for LP quote maintenance: an LP quote that was simply
        // accepted or amended (no trade) needs no reports, trades or logs.
        if focus == 0 && self.events.iter().all(|e| matches!(e, Event::Accepted { account: LP, .. } | Event::Amended { account: LP, .. })) {
            self.events.clear();
            return Vec::new();
        }
        let events = std::mem::take(&mut self.events);
        let mut focus_reports = Vec::new();
        let spec = self.inst[sym].spec.clone();
        let ts = now_ms();

        for e in &events {
            match *e {
                Event::Accepted { id, account, qty, .. } if account != LP => {
                    let r = self.report(id, "NEW", 0.0, 0.0, spec.qty_to_f64(qty), None, &spec, ts);
                    self.deliver(account, r, id == focus, &mut focus_reports);
                }
                Event::Rejected { id, account, reason } if account != LP => {
                    let r = self.report(id, "REJECTED", 0.0, 0.0, 0.0, Some(format!("{reason:?}")), &spec, ts);
                    self.log(format!("REJECT | #{id} {reason:?}"));
                    self.deliver(account, r, id == focus, &mut focus_reports);
                    self.orders.remove(&id);
                }
                Event::Fill { maker_id, taker_id, maker_account, taker_account, taker_side, price, qty, maker_leaves, taker_leaves } => {
                    let (px, q) = (spec.price_to_f64(price), spec.qty_to_f64(qty));
                    let trade = TradeMsg {
                        id: self.next_trade,
                        price: px,
                        qty: q,
                        side: taker_side,
                        ts,
                        taker_is_lp: taker_account == LP,
                        maker_is_lp: maker_account == LP,
                    };
                    self.next_trade += 1;
                    self.log(format!(
                        "MATCH | {} {q} @ {px} | taker #{taker_id}{} maker #{maker_id}{}",
                        spec.symbol,
                        if taker_account == LP { "(LP)" } else { "" },
                        if maker_account == LP { "(LP)" } else { "" }
                    ));
                    let tq = &mut self.trades[sym];
                    if tq.len() == MAX_TRADES {
                        tq.pop_back();
                    }
                    tq.push_front(trade.clone());
                    self.send(None, json!({"type": "trade", "symbol": spec.symbol, "trade": trade}));
                    self.update_candle(sym, px, q, false, ts);

                    for (id, acct, leaves, side) in [
                        (maker_id, maker_account, maker_leaves, taker_side.opposite()),
                        (taker_id, taker_account, taker_leaves, taker_side),
                    ] {
                        if acct == LP {
                            if leaves.0 == 0 {
                                if let Some((s, sd, p)) = self.lp_ids.remove(&id) {
                                    self.lps[s].levels.remove(&(sd, p));
                                }
                            }
                            continue;
                        }
                        if let Some(o) = self.orders.get_mut(&id) {
                            o.cum += qty.0;
                            o.notional += px * q;
                        }
                        self.update_position(acct, sym, side, px, q);
                        self.record_fill(acct, sym, id, side, id == taker_id, px, q, ts);
                        let exec_type = if leaves.0 == 0 { "FILL" } else { "PARTIAL_FILL" };
                        let r = self.report(id, exec_type, px, q, spec.qty_to_f64(leaves), None, &spec, ts);
                        self.deliver(acct, r, id == focus, &mut focus_reports);
                        if leaves.0 == 0 && id != taker_id {
                            self.orders.remove(&id);
                        }
                    }
                    self.dirty_books[sym] = true;
                }
                Event::Cancelled { id, account, reason, .. } => {
                    if account == LP {
                        if let Some((s, sd, p)) = self.lp_ids.remove(&id) {
                            self.lps[s].levels.remove(&(sd, p));
                        }
                        continue;
                    }
                    let text = match reason {
                        CancelReason::User => None,
                        CancelReason::Unfilled => Some("unfilled remainder (IOC/FOK/market)".to_string()),
                        CancelReason::SelfTrade => Some("self-trade prevention".to_string()),
                    };
                    let r = self.report(id, "CANCELED", 0.0, 0.0, 0.0, text, &spec, ts);
                    self.deliver(account, r, id == focus, &mut focus_reports);
                    self.orders.remove(&id);
                    self.dirty_books[sym] = true;
                }
                Event::Amended { id, account, qty, .. } if account != LP => {
                    let r = self.report(id, "REPLACED", 0.0, 0.0, spec.qty_to_f64(qty), None, &spec, ts);
                    self.deliver(account, r, id == focus, &mut focus_reports);
                    self.dirty_books[sym] = true;
                }
                _ => {}
            }
        }

        // A fully filled taker is done once all its events are processed.
        if let Some(o) = self.orders.get(&focus) {
            if self.books[o.sym].order(focus).is_none() && events.iter().any(|e| matches!(e, Event::Fill { taker_leaves: Qty(0), taker_id, .. } if *taker_id == focus)) {
                self.orders.remove(&focus);
            }
        }

        self.events = events;
        self.events.clear();
        if focus != 0 {
            self.dirty_books[sym] = true;
        }
        focus_reports
    }

    #[allow(clippy::too_many_arguments)]
    fn report(&self, id: OrderId, exec_type: &'static str, last_px: f64, last_qty: f64, leaves: f64, text: Option<String>, spec: &SymbolSpec, ts: i64) -> ExecReport {
        let (side, price, cum, avg) = match self.orders.get(&id) {
            Some(o) => {
                let cum = spec.qty_to_f64(Qty(o.cum));
                (o.side, o.px, cum, if cum > 0.0 { o.notional / cum } else { 0.0 })
            }
            None => (Side::Buy, 0.0, 0.0, 0.0),
        };
        ExecReport { order_id: id, symbol: spec.symbol.clone(), side, exec_type, price, last_px, last_qty, cum_qty: cum, leaves_qty: leaves, avg_px: avg, text, ts }
    }

    fn deliver(&mut self, account: AccountId, r: ExecReport, is_focus: bool, focus: &mut Vec<ExecReport>) {
        if let Some(a) = self.accounts.get_mut(&account) {
            if a.execs.len() == MAX_EXECS {
                a.execs.pop_back();
            }
            a.execs.push_front(r.clone());
        }
        self.send(Some(account), json!({"type": "exec", "report": r}));
        if !self.dirty_accounts.contains(&account) {
            self.dirty_accounts.push(account);
        }
        if is_focus {
            focus.push(r);
        }
    }

    fn update_position(&mut self, account: AccountId, sym: usize, side: Side, px: f64, q: f64) {
        let Some(a) = self.accounts.get_mut(&account) else { return };
        let p = a.positions.entry(sym).or_default();
        let signed = if side == Side::Buy { q } else { -q };
        if p.qty == 0.0 || p.qty.signum() == signed.signum() {
            let new_qty = p.qty + signed;
            p.avg_px = (p.avg_px * p.qty.abs() + px * q) / new_qty.abs();
            p.qty = new_qty;
        } else {
            let closing = q.min(p.qty.abs());
            p.realized += closing * (px - p.avg_px) * p.qty.signum();
            let new_qty = p.qty + signed;
            if new_qty == 0.0 {
                p.avg_px = 0.0;
            } else if new_qty.signum() != p.qty.signum() {
                p.avg_px = px; // flipped through zero
            }
            p.qty = new_qty;
        }
    }

    fn update_candle(&mut self, sym: usize, px: f64, vol: f64, is_tick: bool, ts: i64) {
        let bucket = ts - ts.rem_euclid(60_000);
        let cs = &mut self.candles[sym];
        match cs.back_mut() {
            Some(c) if c.t == bucket => {
                c.h = c.h.max(px);
                c.l = c.l.min(px);
                c.c = px;
                c.volume += vol;
                c.ticks += is_tick as u32;
            }
            Some(c) if c.t > bucket => {} // late tick for a closed bar
            _ => {
                if cs.len() == MAX_CANDLES {
                    cs.pop_front();
                }
                cs.push_back(Candle { t: bucket, o: px, h: px, l: px, c: px, ticks: is_tick as u32, volume: vol });
            }
        }
    }

    /// Prepend historical 1m bars from TraderMade REST (`/timeseries`).
    /// REST bars carry no volume, so `ticks` and `volume` stay zero.
    fn backfill(&mut self, symbol: &str, bars: Vec<HistBar>) {
        let Some(&sym) = self.sym_idx.get(symbol) else {
            if self.pending.contains(symbol) {
                self.pending_bars.insert(symbol.to_string(), bars);
            }
            return;
        };
        let live = std::mem::take(&mut self.candles[sym]);
        let first_live = live.front().map(|c| c.t).unwrap_or(i64::MAX);
        let mut merged: VecDeque<Candle> = bars
            .into_iter()
            .filter(|b| b.t < first_live)
            .map(|b| Candle { t: b.t, o: b.o, h: b.h, l: b.l, c: b.c, ticks: 0, volume: 0.0 })
            .collect();
        let added = merged.len();
        merged.extend(live);
        while merged.len() > MAX_CANDLES {
            merged.pop_front();
        }
        self.candles[sym] = merged;
        self.log(format!("BACKFILL | {symbol} {added} x 1m bars from REST /timeseries"));
        let msg = json!({"type": "candles", "symbol": symbol, "candles": self.candles[sym]});
        self.send(None, msg);
    }

    // ------------------------------------------------------ execution quality

    fn mid(&self, sym: usize) -> f64 {
        let q = &self.quotes[sym];
        if q.bid > 0.0 && q.ask > 0.0 {
            (q.bid + q.ask) / 2.0
        } else {
            0.0
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_fill(&mut self, account: AccountId, sym: usize, order_id: OrderId, side: Side, taker: bool, px: f64, qty: f64, ts: i64) {
        let mid_at_fill = self.mid(sym);
        let arrival_mid = self.orders.get(&order_id).map(|o| o.arrival_mid).filter(|m| *m > 0.0).unwrap_or(mid_at_fill);
        let sign = if side == Side::Buy { 1.0 } else { -1.0 };
        let slippage_bps = if arrival_mid > 0.0 { sign * (px - arrival_mid) / arrival_mid * 1e4 } else { 0.0 };
        let fill_id = self.next_fill;
        self.next_fill += 1;
        let symbol = self.inst[sym].spec.symbol.clone();
        let Some(a) = self.accounts.get_mut(&account) else { return };
        if a.fills.len() == MAX_FILLS {
            a.fills.pop_back();
        }
        a.fills.push_front(FillRecord {
            fill_id,
            order_id,
            symbol,
            side,
            liquidity: if taker { "TAKER" } else { "MAKER" },
            px,
            qty,
            ts,
            arrival_mid,
            mid_at_fill,
            slippage_bps,
            markouts_bps: [None; 3],
        });
        let now = Instant::now();
        for (i, ms) in MARKOUTS_MS.iter().enumerate() {
            self.markout_q[i].push_back((now + Duration::from_millis(*ms), account, fill_id, sym));
        }
    }

    /// Fill in markouts whose horizon has passed, using the TraderMade mid then.
    fn due_markouts(&mut self, now: Instant) {
        for h in 0..MARKOUTS_MS.len() {
            while let Some(&(due, account, fill_id, sym)) = self.markout_q[h].front() {
                if due > now {
                    break;
                }
                self.markout_q[h].pop_front();
                let mid = self.mid(sym);
                let Some(f) = self.accounts.get_mut(&account).and_then(|a| a.fills.iter_mut().find(|f| f.fill_id == fill_id)) else {
                    continue;
                };
                if mid > 0.0 {
                    let sign = if f.side == Side::Buy { 1.0 } else { -1.0 };
                    f.markouts_bps[h] = Some(sign * (mid - f.px) / f.px * 1e4);
                }
                if !self.dirty_accounts.contains(&account) {
                    self.dirty_accounts.push(account);
                }
            }
        }
    }

    /// Pre-trade estimate: what this order would cost right now.
    fn estimate(&self, account: AccountId, req: EstimateRequest) -> Result<Value, String> {
        let sym = self.listed(&req.symbol).ok_or_else(|| format!("unknown symbol {}", req.symbol))?;
        let spec = &self.inst[sym].spec;
        if !(req.qty > 0.0) {
            return Err("qty must be positive".into());
        }
        let qty = spec.qty_from_f64(req.qty);
        let limit = req.price.map(|p| spec.price_from_f64(p));
        let e = self.books[sym].estimate(req.side, qty, limit, account);
        let mid = self.mid(sym);
        let filled = spec.qty_to_f64(e.filled);
        let avg = if e.filled.0 > 0 {
            e.notional as f64 / e.filled.0 as f64 / 10f64.powi(spec.price_decimals as i32)
        } else {
            0.0
        };
        let sign = if req.side == Side::Buy { 1.0 } else { -1.0 };
        let bps = |p: f64| if mid > 0.0 && p > 0.0 { Some(sign * (p - mid) / mid * 1e4) } else { None };
        let worst = e.worst.map(|p| spec.price_to_f64(p)).unwrap_or(0.0);
        Ok(json!({
            "symbol": req.symbol,
            "side": req.side,
            "qty": req.qty,
            "filled_qty": filled,
            "fill_ratio": filled / req.qty,
            "avg_px": if filled > 0.0 { Some(avg) } else { None },
            "best_px": e.best.map(|p| spec.price_to_f64(p)),
            "worst_px": if filled > 0.0 { Some(worst) } else { None },
            "levels": e.levels,
            "mid": mid,
            "cost_bps": bps(avg),
            "worst_bps": bps(worst),
            // cost in quote currency versus trading the whole size at mid
            "cost_quote": if filled > 0.0 && mid > 0.0 { Some(sign * (avg - mid) * filled) } else { None },
            "coverage": spec.qty_to_f64(e.available) / req.qty,
        }))
    }

    // ------------------------------------------------------------ publishing

    fn send(&self, account: Option<AccountId>, v: Value) {
        let _ = self.out.send(Outbound { account, json: v.to_string().into() });
    }

    fn log(&mut self, line: String) {
        let line = format!("[{}] {line}", chrono::Utc::now().format("%H:%M:%S%.3f"));
        if self.logs.len() == MAX_LOGS {
            self.logs.pop_back();
        }
        self.logs.push_front(line.clone());
        self.send(None, json!({"type": "log", "line": line}));
    }

    fn publish_dirty(&mut self) {
        // Queue positions move whenever the book does, so refresh every
        // account with a resting order in a changed symbol.
        for o in self.orders.values() {
            if self.dirty_books[o.sym] && !self.dirty_accounts.contains(&o.account) {
                self.dirty_accounts.push(o.account);
            }
        }
        for sym in 0..self.inst.len() {
            if std::mem::take(&mut self.dirty_books[sym]) {
                let book = self.book_json(sym);
                self.send(None, book);
                let quote = self.quote_json(sym);
                self.send(None, quote);
                if let Some(c) = self.candles[sym].back() {
                    let msg = json!({"type": "candle", "symbol": self.inst[sym].spec.symbol, "candle": c});
                    self.send(None, msg);
                }
            }
        }
        for acct in std::mem::take(&mut self.dirty_accounts) {
            let v = self.account_json(acct);
            self.send(Some(acct), v);
        }
    }

    fn book_json(&self, sym: usize) -> Value {
        let spec = &self.inst[sym].spec;
        let book = &self.books[sym];
        let est = &self.lps[sym].estimated;
        let side_json = |side: Side| -> Vec<Value> {
            // user (non-LP) quantity per level, so the UI can highlight it
            let mut user: HashMap<Price, i64> = HashMap::new();
            for o in book.orders_by_priority(side, BOOK_LEVELS) {
                if o.account != LP {
                    *user.entry(o.price).or_default() += o.qty.0;
                }
            }
            book.depth(side, BOOK_LEVELS)
                .into_iter()
                .map(|l| {
                    json!([
                        spec.price_to_f64(l.price),
                        spec.qty_to_f64(l.qty),
                        l.orders,
                        spec.qty_to_f64(Qty(*user.get(&l.price).unwrap_or(&0))),
                        est.contains(&(side, l.price))
                    ])
                })
                .collect()
        };
        json!({
            "type": "book",
            "symbol": spec.symbol,
            "bids": side_json(Side::Buy),
            "asks": side_json(Side::Sell),
            "last": book.last_trade().map(|p| spec.price_to_f64(p)),
            "orders": book.order_count(),
            "seq": self.seq,
        })
    }

    fn quote_json(&self, sym: usize) -> Value {
        let q = &self.quotes[sym];
        json!({
            "type": "quote",
            "symbol": self.inst[sym].spec.symbol,
            "bid": q.bid, "ask": q.ask, "mid": (q.bid + q.ask) / 2.0,
            "bid_vol": q.bid_vol, "ask_vol": q.ask_vol,
            "ts": q.ts_ms, "source": q.source,
        })
    }

    fn account_json(&self, account: AccountId) -> Value {
        let Some(a) = self.accounts.get(&account) else { return json!({"type": "account", "error": "unknown account"}) };
        let open: Vec<Value> = self
            .orders
            .iter()
            .filter(|(_, o)| o.account == account)
            .filter_map(|(id, o)| {
                let v = self.books[o.sym].order(*id)?;
                let spec = &self.inst[o.sym].spec;
                Some(json!({
                    "order_id": id, "symbol": spec.symbol, "side": v.side,
                    "price": spec.price_to_f64(v.price), "leaves_qty": spec.qty_to_f64(v.qty),
                    "cum_qty": spec.qty_to_f64(Qty(o.cum)),
                    "queue": self.books[o.sym].queue_position(*id).map(|q| json!({
                        "orders_ahead": q.orders_ahead,
                        "qty_ahead": spec.qty_to_f64(q.qty_ahead),
                        "level_qty": spec.qty_to_f64(q.level_qty),
                        "level_orders": q.level_orders,
                    })),
                }))
            })
            .collect();
        let positions: Vec<Value> = a
            .positions
            .iter()
            .map(|(sym, p)| {
                let q = &self.quotes[*sym];
                let mid = (q.bid + q.ask) / 2.0;
                let unreal = if p.qty != 0.0 && mid > 0.0 { (mid - p.avg_px) * p.qty } else { 0.0 };
                json!({
                    "symbol": self.inst[*sym].spec.symbol, "qty": p.qty, "avg_px": p.avg_px,
                    "realized": p.realized, "unrealized": unreal, "mark": mid,
                })
            })
            .collect();
        json!({
            "type": "account", "account_id": account, "name": a.name,
            "open_orders": open, "positions": positions, "execs": a.execs,
            "fills": a.fills.iter().take(50).collect::<Vec<_>>(),
            "tca": tca_summary(&a.fills),
        })
    }

    fn metrics_json(&self) -> Value {
        let ops: serde_json::Map<String, Value> = OPS
            .iter()
            .map(|op| {
                let h = &self.metrics.hist[op];
                let v = json!({
                    "count": h.len(),
                    "p50_us": h.value_at_quantile(0.50) as f64 / 1000.0,
                    "p99_us": h.value_at_quantile(0.99) as f64 / 1000.0,
                    "p999_us": h.value_at_quantile(0.999) as f64 / 1000.0,
                    "max_us": h.max() as f64 / 1000.0,
                    "heat": self.metrics.heat[op],
                });
                (op.to_string(), v)
            })
            .collect();
        let t2b = &self.metrics.tick_to_book;
        json!({
            "type": "metrics",
            "ops": ops,
            "ops_per_sec": self.metrics.ops_per_sec,
            "ticks_per_sec": self.metrics.ticks_per_sec,
            "tick_to_book_p50_us": t2b.value_at_quantile(0.5) as f64 / 1000.0,
            "tick_to_book_p99_us": t2b.value_at_quantile(0.99) as f64 / 1000.0,
            "resting_orders": self.books.iter().map(|b| b.order_count()).sum::<usize>(),
            "accounts": self.accounts.len(),
            "uptime_s": self.started.elapsed().as_secs(),
            "seq": self.seq,
        })
    }

    fn snapshot(&self, account: Option<AccountId>) -> Vec<Arc<str>> {
        let mut out: Vec<Value> = vec![json!({
            "type": "hello",
            "instruments": self.visible(),
            "symbols": self.available,
            "feed": self.feed,
            "logs": self.logs.iter().take(100).collect::<Vec<_>>(),
        })];
        for sym in (0..self.inst.len()).filter(|s| !self.hidden.contains(s)) {
            let symbol = &self.inst[sym].spec.symbol;
            out.push(self.book_json(sym));
            out.push(self.quote_json(sym));
            out.push(json!({"type": "trades", "symbol": symbol, "trades": self.trades[sym]}));
            out.push(json!({"type": "candles", "symbol": symbol, "candles": self.candles[sym]}));
        }
        out.push(self.metrics_json());
        if let Some(a) = account {
            out.push(self.account_json(a));
        }
        out.into_iter().map(|v| Arc::from(v.to_string())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(bid: f64, ask: f64, bv: f64, av: f64) -> MarketTick {
        MarketTick {
            symbol: "BTCUSD".into(),
            bid,
            ask,
            bid_vol: Some(bv),
            ask_vol: Some(av),
            ts_ms: now_ms(),
            source: FeedSource::Tradermade,
            recv_at: Instant::now(),
        }
    }

    fn engine() -> Engine {
        let (tx, _rx) = broadcast::channel(1 << 16);
        Engine::new(crate::config::instruments(), tx)
    }

    fn eurusd() -> MarketTick {
        // A real EURUSD update from 2026-10-15, as TraderMade sent it.
        MarketTick {
            symbol: "EURUSD".into(),
            bid: 1.12067,
            ask: 1.12068,
            bid_vol: Some(100_000.0),
            ask_vol: Some(1_000_000.0),
            ts_ms: 1_792_060_400_000,
            source: FeedSource::Tradermade,
            recv_at: Instant::now(),
        }
    }

    fn levels(e: &Engine, sym: usize, side: Side) -> Vec<(f64, f64)> {
        let spec = &e.inst[sym].spec;
        e.books[sym].depth(side, 20).iter().map(|l| (spec.price_to_f64(l.price), spec.qty_to_f64(l.qty))).collect()
    }

    #[test]
    fn quotes_best_bid_and_ask_at_tradermade_sizes() {
        let mut e = engine();
        e.on_tick(eurusd());
        assert_eq!(levels(&e, 0, Side::Buy), vec![(1.12067, 100_000.0)]);
        assert_eq!(levels(&e, 0, Side::Sell), vec![(1.12068, 1_000_000.0)]);
        assert!(e.lps[0].estimated.is_empty(), "both sizes came from TraderMade");
    }

    #[test]
    fn a_missing_size_is_a_marked_default() {
        let mut e = engine();
        e.on_tick(MarketTick { bid_vol: None, ..eurusd() });
        assert_eq!(levels(&e, 0, Side::Buy), vec![(1.12067, e.inst[0].default_level_qty)]);
        assert!(e.lps[0].estimated.contains(&(Side::Buy, e.books[0].best_bid().unwrap())));
        assert!(!e.lps[0].estimated.contains(&(Side::Sell, e.books[0].best_ask().unwrap())));
    }

    #[test]
    fn a_new_price_replaces_the_old_quote() {
        let mut e = engine();
        e.on_tick(eurusd());
        // the market jumps past the old offer: no self-trade, no stale quote left
        e.on_tick(MarketTick { bid: 1.12070, ask: 1.12071, ..eurusd() });
        assert_eq!(levels(&e, 0, Side::Buy), vec![(1.12070, 100_000.0)]);
        assert_eq!(levels(&e, 0, Side::Sell), vec![(1.12071, 1_000_000.0)]);
        assert!(e.trades[0].is_empty());
    }

    /// Tutorial check: overwriting (conflating) the engine's input loses fills.
    #[test]
    fn conflating_engine_input_misses_brief_touches() {
        let quote = |bid: f64, ask: f64, ms: i64| MarketTick { bid, ask, ts_ms: 1_792_060_400_000 + ms, ..eurusd() };
        // the market dips to 1.12065 for one update, then recovers
        let burst = [quote(1.12067, 1.12068, 0), quote(1.12064, 1.12065, 10), quote(1.12067, 1.12068, 20)];
        let run = |ticks: &[MarketTick]| {
            let mut e = engine();
            e.on_tick(quote(1.12067, 1.12068, -100));
            let acct = e.create_session("t".into()).account_id;
            let req = OrderRequest { symbol: "EURUSD".into(), side: Side::Buy, kind: OrderKind::Limit, price: Some(1.12065), qty: 100_000.0, tif: TimeInForce::Gtc, post_only: true };
            let id = e.submit(acct, req).unwrap().order_id;
            for t in ticks {
                e.on_tick(MarketTick { symbol: t.symbol.clone(), ..*t });
            }
            e.books[0].order(id).is_none() // gone from the book = filled
        };
        assert!(run(&burst), "every update processed: the dip fills the resting bid");
        assert!(!run(&burst[2..]), "only the latest update processed: the fill is missed");
    }

    #[test]
    fn symbols_added_at_runtime_get_sensible_settings() {
        use crate::config::instrument_for;
        let dp = |s: &str, b: f64, a: f64| {
            let i = instrument_for(s, b, a);
            (i.spec.price_decimals, i.spec.qty_decimals)
        };
        assert_eq!(dp("GBPJPY", 200.123, 200.126), (3, 0));
        assert_eq!(dp("USDMXN", 18.12345, 18.12395), (5, 0));
        assert_eq!(dp("XAGUSD", 30.123, 30.126), (3, 2));
        assert_eq!(dp("ETHUSD", 3000.12, 3000.62), (2, 4));
        assert_eq!(dp("UK100USD", 8000.1, 8001.1), (2, 2));
        // a spread finer than the class default still gets its own tick
        assert_eq!(dp("NATGASUSD", 3.1234, 3.12345).0, 5);
        // listed symbols keep their hand-tuned settings
        assert_eq!(dp("EURUSD", 1.12071, 1.12072), (5, 0));
    }

    #[test]
    fn requested_symbol_is_listed_on_its_first_quote() {
        let mut e = engine();
        let n = e.inst.len();
        let mut t = tick(200.123, 200.126, 1_000_000.0, 1_000_000.0);
        t.symbol = "GBPJPY".into();
        e.on_tick(MarketTick { symbol: "GBPJPY".into(), ..tick(200.123, 200.126, 1e6, 1e6) });
        assert!(!e.sym_idx.contains_key("GBPJPY"), "not listed unless requested");
        e.pending.insert("GBPJPY".into());
        e.on_tick(t);
        let sym = e.sym_idx["GBPJPY"];
        assert_eq!((sym, e.inst.len(), e.books.len()), (n, n + 1, n + 1));
        assert_eq!(e.books[sym].best_bid().map(|p| e.inst[sym].spec.price_to_f64(p)), Some(200.123));
        assert!(e.pending.is_empty());
    }

    #[test]
    fn accepted_symbol_is_listed_before_its_first_price() {
        let mut e = engine();
        e.pending.insert("AAPL.US".into());
        e.pending.insert("NOPE.US".into());
        // TraderMade's sub_ack: one accepted, one invalid; no prices (market closed)
        e.handle(Command::Feed(FeedStatus {
            mode: "tradermade".into(),
            connected: true,
            accepted: vec!["AAPL.US".into()],
            invalid: vec!["NOPE.US".into()],
            ..Default::default()
        }));
        let sym = e.sym_idx["AAPL.US"];
        assert!(e.inst[sym].awaiting_price && !e.sym_idx.contains_key("NOPE.US") && e.pending.is_empty());

        let acct = e.create_session("t".into()).account_id;
        let req = || OrderRequest { symbol: "AAPL.US".into(), side: Side::Buy, kind: OrderKind::Market, price: None, qty: 1.0, tif: TimeInForce::Ioc, post_only: false };
        assert!(e.submit(acct, req()).unwrap_err().contains("No price"));

        // the first quote sets the real settings, and trading starts
        e.on_tick(MarketTick { symbol: "AAPL.US".into(), ..tick(342.93, 343.12, 21.0, 1.0) });
        assert!(!e.inst[sym].awaiting_price);
        assert_eq!(e.inst[sym].spec.price_decimals, 3);
        assert!(e.submit(acct, req()).is_ok());
    }

    #[test]
    fn added_symbol_can_be_removed_and_added_again() {
        let mut e = engine();
        let gbpjpy = || MarketTick { symbol: "GBPJPY".into(), ..tick(200.123, 200.126, 1e6, 1e6) };
        e.pending.insert("GBPJPY".into());
        e.on_tick(gbpjpy());
        let sym = e.sym_idx["GBPJPY"];
        assert!(e.remove_symbol("EURUSD").is_err(), "built-in symbols stay");

        // open orders block removal
        let acct = e.create_session("t".into()).account_id;
        let req = |px| OrderRequest { symbol: "GBPJPY".into(), side: Side::Buy, kind: OrderKind::Limit, price: Some(px), qty: 1e5, tif: TimeInForce::Gtc, post_only: false };
        let id = e.submit(acct, req(199.0)).unwrap().order_id;
        assert!(e.remove_symbol("GBPJPY").is_err());
        e.cancel(acct, id).unwrap();

        e.remove_symbol("GBPJPY").unwrap();
        assert!(e.books[sym].best_bid().is_none(), "market maker's quotes pulled");
        assert!(e.submit(acct, req(199.0)).is_err(), "no trading in a removed symbol");
        e.on_tick(gbpjpy());
        assert!(e.books[sym].best_bid().is_none(), "late ticks ignored");

        // adding it again reuses the slot
        let (reply, _) = tokio::sync::oneshot::channel();
        e.handle(Command::AddSymbol { symbol: "GBPJPY".into(), reply });
        e.on_tick(gbpjpy());
        assert_eq!((e.sym_idx["GBPJPY"], e.books[sym].best_bid().is_some()), (sym, true));
    }

    /// `cargo test --release -p tm-server lp_sync_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lp_sync_cost() {
        let (tx, _rx) = broadcast::channel(1 << 16);
        let mut e = Engine::new(crate::config::instruments(), tx);
        let n = 20_000;
        let mut px = 85_000.0;
        let t0 = Instant::now();
        for i in 0..n {
            px += if i % 3 == 0 { 0.5 } else { -0.25 };
            let v = 2.0 + (i % 7) as f64 * 0.3;
            e.on_tick(tick(px, px + 0.5, v, v + 0.4));
        }
        let per = t0.elapsed() / n;
        let h = &e.metrics.hist["LP_SYNC"];
        println!("on_tick avg {per:?}; LP_SYNC p50 {}ns p99 {}ns", h.value_at_quantile(0.5), h.value_at_quantile(0.99));
        e.books[e.sym_idx["BTCUSD"]].check_invariants();
    }
}

/// Notional-weighted averages over an account's fills.
fn tca_summary(fills: &VecDeque<FillRecord>) -> Value {
    let wavg = |f: &dyn Fn(&FillRecord) -> Option<f64>| -> Option<f64> {
        let (mut num, mut den) = (0.0, 0.0);
        for r in fills {
            if let Some(v) = f(r) {
                // weight by notional so EURUSD and BTCUSD fills are comparable
                let w = (r.px * r.qty).abs();
                num += v * w;
                den += w;
            }
        }
        (den > 0.0).then(|| num / den)
    };
    json!({
        "fills": fills.len(),
        "taker_fills": fills.iter().filter(|f| f.liquidity == "TAKER").count(),
        "slippage_bps": wavg(&|f| Some(f.slippage_bps)),
        "markout_1s_bps": wavg(&|f| f.markouts_bps[0]),
        "markout_5s_bps": wavg(&|f| f.markouts_bps[1]),
        "markout_60s_bps": wavg(&|f| f.markouts_bps[2]),
    })
}
