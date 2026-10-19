//! Seed the 1-minute charts from TraderMade's REST `/timeseries` endpoint so
//! the UI has history on start-up. REST bars are OHLC only (no volume).

use std::time::Duration;

use crossbeam_channel::Sender;
use serde::Deserialize;

use crate::messages::{Command, HistBar};

const URL: &str = "https://marketdata.tradermade.com/api/v1/timeseries";

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    quotes: Vec<Bar>,
}

#[derive(Deserialize)]
struct Bar {
    date: String,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    close: Option<f64>,
}

pub async fn run(rest_key: String, symbols: Vec<String>, tx: Sender<Command>) {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(15)).build() {
        Ok(c) => c,
        Err(e) => return tracing::warn!("backfill disabled: {e}"),
    };
    // Minute data is limited to a short window per request; two days covers a
    // weekend gap for FX and still leaves 240 bars to keep.
    let end = chrono::Utc::now();
    let start = end - chrono::Duration::days(2);
    let fmt = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%d-%H:%M").to_string();

    for symbol in symbols {
        let mut req = client.get(URL).query(&[
            ("currency", symbol.as_str()),
            ("api_key", rest_key.as_str()),
            ("start_date", &fmt(start)),
            ("end_date", &fmt(end)),
            ("interval", "minute"),
            ("period", "1"),
            ("format", "records"),
        ]);
        if symbol.starts_with("BTC") || symbol.starts_with("ETH") {
            req = req.query(&[("weekend", "true")]); // crypto trades 24/7
        }
        let bars = match req.send().await.and_then(|r| r.error_for_status()) {
            Ok(r) => match r.json::<Resp>().await {
                Ok(body) => body.quotes,
                // without_url(): the request URL carries the API key
                Err(e) => {
                    tracing::warn!("backfill {symbol}: bad body: {}", e.without_url());
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!("backfill {symbol}: {}", e.without_url());
                continue;
            }
        };
        let bars: Vec<HistBar> = bars
            .into_iter()
            .filter_map(|b| {
                // Normal responses use "2026-10-02 18:00:00"; with weekend=true
                // the same field comes back as "2026-10-02-18:00".
                let t = chrono::NaiveDateTime::parse_from_str(&b.date, "%Y-%m-%d %H:%M:%S")
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(&b.date, "%Y-%m-%d-%H:%M"))
                    .ok()?
                    .and_utc()
                    .timestamp_millis();
                Some(HistBar { t, o: b.open?, h: b.high?, l: b.low?, c: b.close? })
            })
            .collect();
        tracing::info!("backfill {symbol}: {} bars", bars.len());
        if tx.send(Command::Backfill { symbol, bars }).is_err() {
            return;
        }
    }
}
