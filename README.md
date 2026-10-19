# TM Match Core

A paper-trading exchange on TraderMade's live prices: a price-time priority matching
engine written in Rust, a market maker that quotes TraderMade's best bid and offer, and a
React trading screen.

Use it to see how a matching engine behaves against a real market, to test order types
and execution analytics, or as a starting point for your own venue. Accounts are demo
accounts: no passwords, no real money.

## Features

- **Matching.** Price-time priority with limit (GTC, IOC, fill-or-kill), market and
  post-only orders, modify, cancel and self-trade prevention. Prices are integer ticks,
  never floats.
- **Live liquidity.** A market-maker account quotes TraderMade's best bid and ask at the
  sizes TraderMade sends. A resting order fills when the market moves through it, as it
  would on a real venue.
- **Any TraderMade symbol.** EURUSD, GBPUSD, USDJPY, XAUUSD and BTCUSD are built in. Add
  any symbol your key can stream from the search: FX, metals, crypto, CFDs, US and German
  stocks and ETFs. Added symbols are kept across restarts.
- **Execution analytics.** Queue position for resting orders, a pre-trade cost estimate,
  slippage against the arrival mid, and markouts at 1, 5 and 60 seconds.
- **Measured latency.** Matching times are recorded inside the engine thread with HDR
  histograms (p50, p99, p99.9), not estimated.
- **Chart history.** One-minute candles, backfilled from TraderMade's REST `/timeseries`
  endpoint.

Without an API key the server runs a synthetic random-walk feed, so you can try it before
connecting to TraderMade.

## Quick start

You need [Rust](https://rustup.rs) (stable) and [Node.js](https://nodejs.org) 20 or later.

```bash
# 1. Build the trading screen
cd web && npm install && npm run build && cd ..

# 2. Add your TraderMade keys (optional: without them a synthetic feed is used)
cp .env.example .env

# 3. Run the server; it also serves the trading screen from web/dist
cargo run --release -p tm-server
```

Open http://127.0.0.1:8080, enter a name and start trading.

TraderMade keys are available from [tradermade.com](https://tradermade.com). A streaming
key gives live prices; a REST key gives chart history. Use the streaming key in one place
at a time: a second login with the same key closes the first connection.

For UI development, run the server and then `cd web && npm run dev`. Vite proxies `/api`
and `/ws` to `http://127.0.0.1:8080`; set `BACKEND` if the server listens elsewhere.

## Configuration

Set these in `.env` or the environment.

| Variable | Default | Purpose |
|---|---|---|
| `TRADERMADE_WS_API_KEY` | — | Streaming key for live prices (WebSocket V2) |
| `TRADERMADE_REST_API_KEY` | — | REST key for chart history |
| `STATIC_DIR` | `web/dist` | Built trading screen to serve |
| `RUST_LOG` | `info` | Log level |

API keys stay on the server. They are never sent to the browser or written to the logs.

## How it works

```
TraderMade WebSocket ──► feed task ──┐
(synthetic if no key)                │ channel
                                     ▼
 Browser ◄─WS── axum ──REST──► ENGINE THREAD (owns every order, account and metric)
                ◄── broadcast ───┘ pre-serialised JSON, filtered per account
```

- **One engine thread** owns all state, so there are no locks on the matching path and the
  same sequence of commands always gives the same result.
- **The market maker** quotes TraderMade's best bid and ask and nothing behind them. On
  each tick it compares its resting quotes with the new prices: an unchanged quote keeps
  its place in the queue, a changed one is amended, and a stale one is cancelled before
  anything new is added, so new quotes never trade against old ones. If TraderMade sends
  no size (for example the last price of a closed market), a default size is used and
  marked as estimated.
- **Adding a symbol.** TraderMade's subscription reply decides. An accepted symbol is
  listed straight away, even if its market is closed and no price arrives for hours; an
  invalid or denied one is refused with the reason. Orders are accepted from the first
  price, which also sets the symbol's price decimals from its asset class and spread.
- **The trading screen** receives updates over one WebSocket, batched to one render per
  frame. The Monitoring tab shows the feed status, matching latency, the live
  TraderMade quotes, the engine log and FIX-style execution reports.

### Symbol forms

TraderMade's codes say what an instrument is.

| Example | Instrument |
|---|---|
| `GBPJPY` | FX pair |
| `XAUUSD` | Metal |
| `BTCUSD` | Crypto |
| `AAPLUSD`, `NAS100USD`, `UKOILUSD` | CFDs on stocks, indices and commodities |
| `AAPL.US` | US stock |
| `SAPG.DE` | German stock |
| `SPY.ETF` | ETF |

`AAPL.US` and `AAPLUSD` stream the same Apple price; the stock form also carries sizes.
The search labels every result and also matches names, so "apple", "dax" or "oil" find
the right instruments.

## API

Authenticated calls use `Authorization: Bearer <token>` from `POST /api/session`.
Sessions are held in memory, so a restart signs everyone out.

| Method | Path | Body or result |
|---|---|---|
| `POST` | `/api/session` | `{"name":"alex"}` → `{token, account_id}` |
| `GET` | `/api/me` | Positions, open orders, execution reports, fills |
| `POST` | `/api/orders` | `{"symbol":"EURUSD","side":"BUY","type":"LIMIT","price":1.1251,"qty":100000,"tif":"GTC","post_only":false}` |
| `PUT` | `/api/orders/:id` | `{"price":1.1250,"qty":50000}` |
| `DELETE` | `/api/orders/:id` | Cancel |
| `GET` | `/api/estimate?symbol=EURUSD&side=BUY&qty=1000000` | Pre-trade cost, without trading |
| `POST` | `/api/symbols` | `{"symbol":"GBPJPY"}` → `listed`, `requested` or `pending` |
| `DELETE` | `/api/symbols/:symbol` | Remove an added symbol (no open orders in it) |
| `GET` | `/api/health` | `ok` |
| `WS` | `/ws?token=…` | `hello`, `instruments`, `quote`, `trade`, `candle`, `account`, `exec`, `metrics`, `feed`, `log` |

## Performance

`cargo bench -p tm-engine --bench matching`, on a book with 50 levels of 10 orders a side
(release build with LTO, Windows desktop):

| Operation | Time |
|---|---|
| Add a passive limit order | ~92 ns |
| Cancel a resting order | ~93 ns |
| Aggressive limit order, one fill | ~78 ns |
| Market order sweeping 5 levels / 50 fills | ~1.25 µs |

## Tests

```bash
cargo test --workspace
```

Unit tests cover each order type and the market maker's behaviour on live quotes. A
property test sends random command streams and checks that the book never crosses, its
level totals stay consistent and no quantity is created or lost.

## Project layout

```
crates/engine   order book and matching (no I/O), tests, benchmarks
crates/server   axum server, TraderMade feed, engine thread, market maker, metrics
web/            React 19, Vite, Tailwind CSS 4, lightweight-charts
```

## Licence

MIT. See [LICENSE](LICENSE). The licence covers this code; market data is supplied by
TraderMade under the terms of your TraderMade account.
