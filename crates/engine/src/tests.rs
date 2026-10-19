use proptest::prelude::*;

use crate::*;

const A: AccountId = 1;
const B: AccountId = 2;

fn run(book: &mut OrderBook, o: NewOrder) -> Vec<Event> {
    let mut out = Vec::new();
    book.submit(o, &mut out);
    book.check_invariants();
    out
}

fn fills(ev: &[Event]) -> Vec<(OrderId, i64, i64)> {
    ev.iter()
        .filter_map(|e| match e {
            Event::Fill { maker_id, price, qty, .. } => Some((*maker_id, price.0, qty.0)),
            _ => None,
        })
        .collect()
}

#[test]
fn resting_limit_orders_build_the_book() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Buy, 100, 5));
    run(&mut b, NewOrder::limit(2, A, Side::Buy, 101, 5));
    run(&mut b, NewOrder::limit(3, A, Side::Sell, 105, 5));
    assert_eq!(b.best_bid(), Some(Price(101)));
    assert_eq!(b.best_ask(), Some(Price(105)));
    assert_eq!(b.depth(Side::Buy, 10).len(), 2);
}

#[test]
fn price_then_time_priority() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 101, 5));
    run(&mut b, NewOrder::limit(2, A, Side::Sell, 100, 5)); // better price
    run(&mut b, NewOrder::limit(3, A, Side::Sell, 100, 5)); // same price, later
    let ev = run(&mut b, NewOrder::limit(10, B, Side::Buy, 101, 12));
    assert_eq!(fills(&ev), vec![(2, 100, 5), (3, 100, 5), (1, 101, 2)]);
    assert_eq!(b.order(1).unwrap().qty, Qty(3));
    assert_eq!(b.last_trade(), Some(Price(101)));
}

#[test]
fn partial_fill_rests_remainder() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 3));
    let ev = run(&mut b, NewOrder::limit(2, B, Side::Buy, 100, 10));
    assert_eq!(fills(&ev), vec![(1, 100, 3)]);
    assert_eq!(b.best_bid(), Some(Price(100)));
    assert_eq!(b.order(2).unwrap().qty, Qty(7));
    assert_eq!(b.best_ask(), None);
}

#[test]
fn market_order_sweeps_and_cancels_rest() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Buy, 99, 2));
    run(&mut b, NewOrder::limit(2, A, Side::Buy, 98, 2));
    let ev = run(&mut b, NewOrder::market(3, B, Side::Sell, 10));
    assert_eq!(fills(&ev), vec![(1, 99, 2), (2, 98, 2)]);
    assert!(matches!(ev.last(), Some(Event::Cancelled { leaves: Qty(6), reason: CancelReason::Unfilled, .. })));
    assert_eq!(b.order_count(), 0);
}

#[test]
fn ioc_never_rests() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 2));
    let ev = run(&mut b, NewOrder::limit(2, B, Side::Buy, 100, 5).with_tif(TimeInForce::Ioc));
    assert_eq!(fills(&ev).len(), 1);
    assert!(b.order(2).is_none());
}

#[test]
fn fok_is_all_or_nothing() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 2));
    run(&mut b, NewOrder::limit(2, A, Side::Sell, 101, 2));
    let ev = run(&mut b, NewOrder::limit(3, B, Side::Buy, 101, 5).with_tif(TimeInForce::Fok));
    assert!(matches!(ev[..], [Event::Rejected { reason: RejectReason::FokNotFillable, .. }]));
    assert_eq!(b.order_count(), 2, "book untouched");
    let ev = run(&mut b, NewOrder::limit(4, B, Side::Buy, 101, 4).with_tif(TimeInForce::Fok));
    assert_eq!(fills(&ev).len(), 2);
    assert_eq!(b.order_count(), 0);
}

#[test]
fn post_only_rejects_when_crossing() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 2));
    let ev = run(&mut b, NewOrder::limit(2, B, Side::Buy, 100, 1).post_only());
    assert!(matches!(ev[..], [Event::Rejected { reason: RejectReason::PostOnlyWouldCross, .. }]));
    let ev = run(&mut b, NewOrder::limit(3, B, Side::Buy, 99, 1).post_only());
    assert!(matches!(ev[..], [Event::Accepted { .. }]));
}

#[test]
fn self_trade_prevention_cancels_resting() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 2));
    run(&mut b, NewOrder::limit(2, B, Side::Sell, 100, 2));
    let ev = run(&mut b, NewOrder::limit(3, A, Side::Buy, 100, 2));
    assert!(ev.iter().any(|e| matches!(e, Event::Cancelled { id: 1, reason: CancelReason::SelfTrade, .. })));
    assert_eq!(fills(&ev), vec![(2, 100, 2)]);
}

#[test]
fn cancel_and_ownership() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Buy, 100, 2));
    let mut ev = Vec::new();
    b.cancel(1, B, &mut ev);
    assert!(matches!(ev[..], [Event::Rejected { reason: RejectReason::NotOwner, .. }]));
    ev.clear();
    b.cancel(1, A, &mut ev);
    assert!(matches!(ev[..], [Event::Cancelled { reason: CancelReason::User, .. }]));
    b.check_invariants();
    assert_eq!(b.best_bid(), None);
}

#[test]
fn reduce_keeps_priority_reprice_loses_it() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 5));
    run(&mut b, NewOrder::limit(2, A, Side::Sell, 100, 5));
    let mut ev = Vec::new();
    b.modify(1, A, Price(100), Qty(3), &mut ev); // reduce: stays first
    b.check_invariants();
    let ev = run(&mut b, NewOrder::limit(9, B, Side::Buy, 100, 1));
    assert_eq!(fills(&ev), vec![(1, 100, 1)]);

    let mut ev = Vec::new();
    b.modify(1, A, Price(100), Qty(10), &mut ev); // increase: back of queue
    b.check_invariants();
    let ev = run(&mut b, NewOrder::limit(10, B, Side::Buy, 100, 1));
    assert_eq!(fills(&ev), vec![(2, 100, 1)]);
}

#[test]
fn modify_into_cross_trades() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 5));
    run(&mut b, NewOrder::limit(2, B, Side::Buy, 95, 5));
    let mut ev = Vec::new();
    b.modify(2, B, Price(100), Qty(5), &mut ev);
    b.check_invariants();
    assert_eq!(fills(&ev), vec![(1, 100, 5)]);
    assert_eq!(b.order_count(), 0);
}

#[test]
fn rejects_bad_input() {
    let mut b = OrderBook::new();
    let ev = run(&mut b, NewOrder::limit(1, A, Side::Buy, 100, 0));
    assert!(matches!(ev[..], [Event::Rejected { reason: RejectReason::InvalidQty, .. }]));
    let ev = run(&mut b, NewOrder::limit(1, A, Side::Buy, 0, 1));
    assert!(matches!(ev[..], [Event::Rejected { reason: RejectReason::InvalidPrice, .. }]));
    run(&mut b, NewOrder::limit(1, A, Side::Buy, 100, 1));
    let ev = run(&mut b, NewOrder::limit(1, A, Side::Buy, 100, 1));
    assert!(matches!(ev[..], [Event::Rejected { reason: RejectReason::DuplicateOrderId, .. }]));
}

#[test]
fn symbol_spec_round_trips() {
    let s = SymbolSpec::new("EURUSD", 5, 0);
    assert_eq!(s.price_from_f64(1.16185), Price(116185));
    assert!((s.price_to_f64(Price(116185)) - 1.16185).abs() < 1e-12);
}

#[test]
fn queue_position_counts_whats_ahead() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Buy, 100, 5));
    run(&mut b, NewOrder::limit(2, A, Side::Buy, 100, 7));
    run(&mut b, NewOrder::limit(3, B, Side::Buy, 100, 2));
    let q = b.queue_position(3).unwrap();
    assert_eq!((q.orders_ahead, q.qty_ahead, q.level_qty, q.level_orders), (2, Qty(12), Qty(14), 3));
    assert_eq!(b.queue_position(1).unwrap().orders_ahead, 0);
    // fills ahead of you move you up the queue
    run(&mut b, NewOrder::limit(9, B, Side::Sell, 100, 6));
    let q = b.queue_position(3).unwrap();
    assert_eq!((q.orders_ahead, q.qty_ahead), (1, Qty(6)));
    assert!(b.queue_position(99).is_none());
}

#[test]
fn estimate_walks_book_without_trading() {
    let mut b = OrderBook::new();
    run(&mut b, NewOrder::limit(1, A, Side::Sell, 100, 5));
    run(&mut b, NewOrder::limit(2, A, Side::Sell, 101, 5));
    run(&mut b, NewOrder::limit(3, B, Side::Sell, 102, 5)); // B's own order: skipped for B
    let e = b.estimate(Side::Buy, Qty(8), None, B);
    assert_eq!((e.filled, e.notional, e.best, e.worst, e.levels), (Qty(8), 100 * 5 + 101 * 3, Some(Price(100)), Some(Price(101)), 2));
    assert_eq!(e.available, Qty(10));
    let e = b.estimate(Side::Buy, Qty(20), Some(Price(100)), B);
    assert_eq!(e.filled, Qty(5), "limit stops the walk");
    assert_eq!(b.order_count(), 3, "book untouched");
    // agrees with what really happens
    let ev = run(&mut b, NewOrder::limit(10, B, Side::Buy, 101, 8));
    let notional: i128 = ev.iter().filter_map(|e| match e { Event::Fill { price, qty, .. } => Some(price.0 as i128 * qty.0 as i128), _ => None }).sum();
    assert_eq!(notional, 100 * 5 + 101 * 3);
}

#[derive(Debug, Clone)]
enum Op {
    Submit { side: bool, px: i64, qty: i64, acct: u32, tif: u8, market: bool },
    Cancel(u64),
    Modify(u64, i64, i64),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => (any::<bool>(), 95i64..106, 1i64..20, 1u32..4, 0u8..3, prop::bool::weighted(0.1))
            .prop_map(|(side, px, qty, acct, tif, market)| Op::Submit { side, px, qty, acct, tif, market }),
        2 => (0u64..200).prop_map(Op::Cancel),
        1 => (0u64..200, 95i64..106, 1i64..20).prop_map(|(id, px, q)| Op::Modify(id, px, q)),
    ]
}

proptest! {
    /// Random command streams never break book structure and conserve quantity:
    /// every lot submitted is filled (counted twice: maker + taker), cancelled,
    /// or still resting.
    #[test]
    fn invariants_hold(ops in prop::collection::vec(op(), 1..300)) {
        let mut b = OrderBook::new();
        let mut owner = std::collections::HashMap::new();
        let mut ev = Vec::new();
        let (mut submitted, mut filled, mut cancelled, mut amended_delta) = (0i64, 0i64, 0i64, 0i64);

        for (i, op) in ops.into_iter().enumerate() {
            let id = i as u64;
            ev.clear();
            match op {
                Op::Submit { side, px, qty, acct, tif, market } => {
                    let side = if side { Side::Buy } else { Side::Sell };
                    let mut o = if market {
                        NewOrder::market(id, acct, side, qty)
                    } else {
                        NewOrder::limit(id, acct, side, px, qty)
                    };
                    if !market {
                        o.tif = [TimeInForce::Gtc, TimeInForce::Ioc, TimeInForce::Fok][tif as usize];
                    }
                    owner.insert(id, acct);
                    b.submit(o, &mut ev);
                }
                Op::Cancel(target) => {
                    let acct = *owner.get(&target).unwrap_or(&1);
                    b.cancel(target, acct, &mut ev);
                }
                Op::Modify(target, px, q) => {
                    let acct = *owner.get(&target).unwrap_or(&1);
                    let before = b.order(target).map(|o| o.qty.0);
                    b.modify(target, acct, Price(px), Qty(q), &mut ev);
                    if let Some(before) = before {
                        if ev.iter().any(|e| matches!(e, Event::Amended { .. })) {
                            amended_delta += q - before;
                        }
                    }
                }
            }
            for e in &ev {
                match e {
                    Event::Accepted { qty, .. } => submitted += qty.0,
                    Event::Fill { qty, .. } => filled += qty.0,
                    Event::Cancelled { leaves, .. } => cancelled += leaves.0,
                    _ => {}
                }
            }
            b.check_invariants();
        }

        let resting: i64 = b.depth(Side::Buy, usize::MAX).iter()
            .chain(b.depth(Side::Sell, usize::MAX).iter())
            .map(|l| l.qty.0).sum();
        prop_assert_eq!(submitted + amended_delta, 2 * filled + cancelled + resting);
    }
}
