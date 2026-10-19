//! Market data: TraderMade WebSocket V2 (best bid/ask with quoted sizes) or, without an API
//! key, a synthetic random-walk feed so the exchange always has liquidity.

use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use crossbeam_channel::Sender;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::config::Instrument;
use crate::engine::now_ms;
use crate::messages::{Command, FeedCtl, FeedSource, FeedStatus, MarketTick};

const TM_URL: &str = "wss://stream.tradermade.com/feedAdv";

/// `subscribe` receives symbols added or removed at runtime (from `/api/symbols`).
pub async fn run(api_key: Option<String>, instruments: Vec<Instrument>, tx: Sender<Command>, subscribe: UnboundedReceiver<FeedCtl>) {
    match api_key {
        Some(key) => tradermade(key, instruments, tx, subscribe).await,
        None => synthetic(instruments, tx).await,
    }
}

fn status(tx: &Sender<Command>, s: FeedStatus) {
    let _ = tx.send(Command::Feed(s));
}

// ------------------------------------------------------------------ TraderMade

async fn tradermade(key: String, instruments: Vec<Instrument>, tx: Sender<Command>, mut subscribe: UnboundedReceiver<FeedCtl>) {
    // Follows symbols as they are added and removed, so a reconnect subscribes to the current list.
    let mut symbols: Vec<String> = instruments.iter().map(|i| i.spec.symbol.clone()).collect();
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match tradermade_session(&key, &mut symbols, &tx, &mut subscribe).await {
            Ok(()) => tracing::warn!("TraderMade stream closed"),
            Err(e) => {
                tracing::error!("TraderMade stream error: {e:#}");
                status(&tx, FeedStatus {
                    mode: "tradermade".into(),
                    connected: false,
                    message: Some(format!("{e:#}")),
                    ..Default::default()
                });
                // Only a key TraderMade doesn't recognise is fatal. Rejections like
                // `duplicate_login_active` (the key is still logged in elsewhere,
                // or a previous session hasn't timed out yet) clear on their own,
                // so keep retrying with back-off.
                let msg = e.to_string();
                if msg.contains("login rejected") && (msg.contains("unknown_key") || msg.contains("invalid")) {
                    tracing::error!("TraderMade does not recognise the streaming key; falling back to synthetic feed");
                    return synthetic(instruments, tx).await;
                }
                if msg.contains("duplicate_login") {
                    tracing::warn!("key is logged in elsewhere (or a stale session is open); retrying");
                    backoff = backoff.max(Duration::from_secs(10));
                }
            }
        }
        if started.elapsed() > Duration::from_secs(30) {
            backoff = Duration::from_secs(1);
        }
        tracing::info!("reconnecting to TraderMade in {backoff:?}");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn tradermade_session(
    key: &str,
    symbols: &mut Vec<String>,
    tx: &Sender<Command>,
    subscribe: &mut UnboundedReceiver<FeedCtl>,
) -> anyhow::Result<()> {
    let (ws, _) = connect_async(TM_URL).await.context("connect")?;
    let (mut write, mut read) = ws.split();

    let login = json!({"action": "login", "key": key, "fmt": "JSON"});
    write.send(Message::Text(login.to_string())).await?;

    let mut st = FeedStatus { mode: "tradermade".into(), ..Default::default() };
    // Symbols denied with `toggle_cooldown` (subscribed again too soon, e.g.
    // right after a reconnect) are retried rather than silently lost.
    let mut retry: Vec<String> = Vec::new();
    let mut retry_at = tokio::time::Instant::now();
    let mut logged_in = false;

    loop {
        let msg = tokio::select! {
            m = read.next() => match m {
                Some(m) => m?,
                None => break,
            },
            Some(ctl) = subscribe.recv() => {
                let sub = match ctl {
                    FeedCtl::Subscribe(sym) => {
                        if !symbols.contains(&sym) {
                            symbols.push(sym.clone());
                        }
                        json!({"action": "subscribe", "send_last": true, "symbols": [format!("{sym}:QUOTE")]})
                    }
                    FeedCtl::Unsubscribe(sym) => {
                        symbols.retain(|s| s != &sym);
                        retry.retain(|s| s != &format!("{sym}:QUOTE"));
                        st.accepted.retain(|s| s != &sym);
                        st.denied.remove(&sym);
                        status(tx, st.clone());
                        json!({"action": "unsubscribe", "symbols": [format!("{sym}:QUOTE")]})
                    }
                };
                // Before login_ok, the change is picked up with the rest.
                if logged_in {
                    write.send(Message::Text(sub.to_string())).await?;
                }
                continue;
            }
            _ = tokio::time::sleep_until(retry_at), if !retry.is_empty() => {
                tracing::info!("retrying subscription: {retry:?}");
                let sub = json!({"action": "subscribe", "send_last": true, "symbols": std::mem::take(&mut retry)});
                write.send(Message::Text(sub.to_string())).await?;
                continue;
            }
        };
        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(p) => {
                write.send(Message::Pong(p)).await?;
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };
        let recv_at = Instant::now();
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            tracing::debug!("non-JSON frame: {text}");
            continue;
        };

        // Price ticks are the hot path: check them first.
        if let Some(t) = v.get("t").and_then(Value::as_str) {
            if t == "QUOTE" || t == "LAST_QUOTE" {
                if let Some(tick) = parse_tick(&v, recv_at) {
                    if tx.send(Command::MarketData(tick)).is_err() {
                        return Ok(());
                    }
                }
                continue;
            }
        }

        match v.get("type").and_then(Value::as_str) {
            Some("login_ok") => {
                st.connected = true;
                tracing::info!("TraderMade login ok");
                let subs: Vec<String> = symbols.iter().map(|s| format!("{s}:QUOTE")).collect();
                let sub = json!({"action": "subscribe", "send_last": true, "symbols": subs});
                write.send(Message::Text(sub.to_string())).await?;
                // Ask which symbols this key can stream, for the symbol search.
                write.send(Message::Text(json!({"action": "symbols"}).to_string())).await?;
                logged_in = true;
            }
            Some("symbols") => {
                let list: Vec<String> = v
                    .get("symbols")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                tracing::info!("TraderMade symbol list: {} symbols", list.len());
                let _ = tx.send(Command::SymbolList(list));
            }
            Some("login_reject") => {
                bail!("login rejected: {}", v.get("reason").and_then(Value::as_str).unwrap_or("?"));
            }
            Some("sub_ack") => {
                let list = |k: &str| -> Vec<String> {
                    v.get(k)
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim_end_matches(":QUOTE").to_string()).collect())
                        .unwrap_or_default()
                };
                // Acks are incremental (retries produce their own), so merge.
                // A symbol can appear in both `accepted` and `denied` when it
                // was requested twice; `denied` wins for that request only.
                let reasons = v.get("denied_reasons").and_then(Value::as_object);
                for s in list("accepted") {
                    if !st.accepted.contains(&s) {
                        st.accepted.push(s);
                    }
                }
                for s in list("invalid") {
                    if !st.invalid.contains(&s) {
                        st.invalid.push(s);
                    }
                }
                for s in list("denied") {
                    let reason = reasons
                        .and_then(|r| r.get(&format!("{s}:QUOTE")))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    if reason == "toggle_cooldown" {
                        retry.push(format!("{s}:QUOTE"));
                        retry_at = tokio::time::Instant::now() + Duration::from_secs(5);
                    } else {
                        st.accepted.retain(|a| a != &s);
                    }
                    st.denied.insert(s, reason);
                }
                for s in &st.accepted {
                    if st.denied.get(s).is_some_and(|r| r == "toggle_cooldown") && !retry.iter().any(|r| r.starts_with(s.as_str())) {
                        st.denied.remove(s);
                    }
                }
                if !st.invalid.is_empty() {
                    tracing::warn!("TraderMade rejected symbols as invalid: {:?} (CFDs need a USD suffix, e.g. UK100USD; stocks AAPL.US)", st.invalid);
                }
                status(tx, st.clone());
            }
            Some("logout") => bail!("server logout: {}", v.get("reason").and_then(Value::as_str).unwrap_or("?")),
            Some("error") => tracing::warn!("TraderMade error: {}", redacted(&v)),
            _ => tracing::debug!("unhandled frame: {}", redacted(&v)),
        }
    }
    Ok(())
}

/// A frame for the logs, without the API key TraderMade echoes back in some replies.
fn redacted(v: &Value) -> String {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        if o.contains_key("key") {
            o.insert("key".into(), json!("<redacted>"));
        }
    }
    v.to_string()
}

/// TraderMade sends numbers as strings; accept either.
fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// `"20260515-12:36:35.588"` -> ms since epoch (UTC).
fn parse_ts(s: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y%m%d-%H:%M:%S%.3f").ok().map(|t| t.and_utc().timestamp_millis())
}

fn parse_tick(v: &Value, recv_at: Instant) -> Option<MarketTick> {
    Some(MarketTick {
        symbol: v.get("s")?.as_str()?.to_string(),
        bid: num(v.get("b"))?,
        ask: num(v.get("a"))?,
        // the size quoted at the best bid and ask
        bid_vol: num(v.get("bv")),
        ask_vol: num(v.get("av")),
        ts_ms: v.get("ts").and_then(Value::as_str).and_then(parse_ts).unwrap_or_else(now_ms),
        source: FeedSource::Tradermade,
        recv_at,
    })
}

// ------------------------------------------------------------------- synthetic

async fn synthetic(instruments: Vec<Instrument>, tx: Sender<Command>) {
    tracing::warn!("no TRADERMADE_API_KEY: using synthetic feed");
    status(&tx, FeedStatus {
        mode: "synthetic".into(),
        connected: true,
        accepted: instruments.iter().map(|i| i.spec.symbol.clone()).collect(),
        message: Some("set TRADERMADE_API_KEY for live TraderMade prices".into()),
        ..Default::default()
    });
    let mut mids: Vec<f64> = instruments.iter().map(|i| i.seed_price).collect();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    loop {
        interval.tick().await;
        let mut rng = rand::thread_rng();
        for (i, inst) in instruments.iter().enumerate() {
            if rng.gen_bool(0.35) {
                continue; // not every symbol ticks every interval
            }
            // ~1bp random walk with a slight pull back to the seed
            let drift = (inst.seed_price - mids[i]) / inst.seed_price * 0.02;
            mids[i] *= 1.0 + rng.gen_range(-1.0..1.0) * 0.0001 + drift * 0.0001;
            let half = mids[i] * inst.seed_spread / 2.0;
            let size = |r: &mut rand::rngs::ThreadRng| inst.default_level_qty * r.gen_range(0.5..2.0);
            let tick = MarketTick {
                symbol: inst.spec.symbol.clone(),
                bid: mids[i] - half,
                ask: mids[i] + half,
                bid_vol: Some(size(&mut rng)),
                ask_vol: Some(size(&mut rng)),
                ts_ms: now_ms(),
                source: FeedSource::Synthetic,
                recv_at: Instant::now(),
            };
            if tx.send(Command::MarketData(tick)).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_live_quote() {
        // A real EURUSD message from 2026-10-19.
        let v: Value = serde_json::from_str(
            r#"{"a":"1.121570000","av":"1000000","b":"1.121560000","bv":"100000","s":"EURUSD","t":"QUOTE","ts":"20261019-14:19:55.073"}"#,
        )
        .unwrap();
        let t = parse_tick(&v, Instant::now()).unwrap();
        assert_eq!((t.symbol.as_str(), t.bid, t.ask), ("EURUSD", 1.12156, 1.12157));
        assert_eq!((t.bid_vol, t.ask_vol), (Some(100_000.0), Some(1_000_000.0)));
        assert_eq!(t.ts_ms, parse_ts("20261019-14:19:55.073").unwrap());
    }

    #[test]
    fn logged_frames_never_show_the_key() {
        let frame = json!({"type": "logout", "reason": "duplicate_login", "key": "secret-key-123"});
        let line = redacted(&frame);
        assert!(!line.contains("secret-key-123") && line.contains("duplicate_login"));
    }

    #[test]
    fn parses_last_quote_without_volume() {
        let v: Value = serde_json::from_str(
            r#"{"a":"1.1621","b":"1.16207","m":"1.162085","s":"EURUSD","t":"LAST_QUOTE","ts":"20260515-12:39:21.783"}"#,
        )
        .unwrap();
        let t = parse_tick(&v, Instant::now()).unwrap();
        assert!(t.bid_vol.is_none() && t.ask_vol.is_none());
    }
}
