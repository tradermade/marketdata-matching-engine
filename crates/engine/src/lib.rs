//! `tm-engine`: a single-threaded, deterministic price-time priority matching
//! engine. No I/O, no clocks, no floats on the matching path — feed it
//! commands, get events back.

mod book;
mod types;

pub use book::OrderBook;
pub use types::*;

#[cfg(test)]
mod tests;
