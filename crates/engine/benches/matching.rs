use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use tm_engine::*;

/// Book with `levels` price levels per side, `per_level` orders each,
/// around a mid of 100_000 ticks.
fn seeded(levels: i64, per_level: i64) -> (OrderBook, u64) {
    let mut b = OrderBook::with_capacity((levels * per_level * 2) as usize + 1024);
    let mut out = Vec::new();
    let mut id = 0;
    for l in 1..=levels {
        for _ in 0..per_level {
            id += 1;
            b.submit(NewOrder::limit(id, 1, Side::Buy, 100_000 - l, 10), &mut out);
            id += 1;
            b.submit(NewOrder::limit(id, 1, Side::Sell, 100_000 + l, 10), &mut out);
        }
    }
    (b, id)
}

fn benches(c: &mut Criterion) {
    let mut out = Vec::with_capacity(64);

    c.bench_function("add passive limit (no cross)", |bench| {
        bench.iter_batched_ref(
            || seeded(50, 10),
            |(b, id)| {
                *id += 1;
                out.clear();
                b.submit(black_box(NewOrder::limit(*id, 2, Side::Buy, 99_990, 5)), &mut out);
            },
            BatchSize::LargeInput,
        )
    });

    c.bench_function("cancel resting order", |bench| {
        bench.iter_batched_ref(
            || seeded(50, 10),
            |(b, _)| {
                out.clear();
                b.cancel(black_box(250), 1, &mut out);
            },
            BatchSize::LargeInput,
        )
    });

    c.bench_function("aggressive limit: 1 fill", |bench| {
        bench.iter_batched_ref(
            || seeded(50, 10),
            |(b, id)| {
                *id += 1;
                out.clear();
                b.submit(black_box(NewOrder::limit(*id, 2, Side::Buy, 100_001, 5)), &mut out);
            },
            BatchSize::LargeInput,
        )
    });

    c.bench_function("market sweep: 5 levels / 50 fills", |bench| {
        bench.iter_batched_ref(
            || seeded(50, 10),
            |(b, id)| {
                *id += 1;
                out.clear();
                b.submit(black_box(NewOrder::market(*id, 2, Side::Buy, 500)), &mut out);
            },
            BatchSize::LargeInput,
        )
    });
}

criterion_group!(matching, benches);
criterion_main!(matching);
