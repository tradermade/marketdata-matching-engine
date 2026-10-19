//! HTTP + WebSocket API. Handlers only translate requests into engine
//! commands; all state lives on the engine thread.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use crossbeam_channel::Sender;
use serde::Deserialize;
use serde_json::json;
use tm_engine::{AccountId, OrderId};
use tokio::sync::{broadcast, oneshot};

use crate::messages::*;

#[derive(Clone)]
pub struct AppState {
    pub tx: Sender<Command>,
    pub out: broadcast::Sender<Outbound>,
    pub tokens: Arc<RwLock<HashMap<String, AccountId>>>,
    /// Asks the feed task to subscribe to (or drop) a symbol added at runtime.
    pub subscribe: tokio::sync::mpsc::UnboundedSender<FeedCtl>,
    /// True when running on TraderMade's live feed rather than the synthetic one.
    pub live: bool,
    pub rest_key: Option<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/session", post(create_session))
        .route("/api/me", get(me))
        .route("/api/symbols", post(add_symbol))
        .route("/api/symbols/:symbol", delete(remove_symbol))
        .route("/api/orders", post(submit))
        .route("/api/estimate", get(estimate))
        .route("/api/orders/:id", put(modify).delete(cancel))
        .route("/ws", get(ws))
        .with_state(state)
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    (code, Json(json!({"error": msg.into()})))
}

impl AppState {
    fn account(&self, headers: &HeaderMap) -> Result<AccountId, ApiError> {
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing bearer token"))?;
        self.tokens
            .read()
            .unwrap()
            .get(token)
            .copied()
            .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "unknown session; create one with POST /api/session"))
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Command) -> Result<T, ApiError> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(make(reply)).map_err(|_| err(StatusCode::SERVICE_UNAVAILABLE, "engine stopped"))?;
        rx.await.map_err(|_| err(StatusCode::SERVICE_UNAVAILABLE, "engine stopped"))
    }
}

#[derive(Deserialize)]
struct SessionReq {
    #[serde(default)]
    name: String,
}

async fn create_session(State(s): State<AppState>, body: Option<Json<SessionReq>>) -> Result<Json<SessionInfo>, ApiError> {
    let name = body.map(|b| b.0.name).unwrap_or_default();
    let info = s.ask(|reply| Command::Session { name, reply }).await?;
    s.tokens.write().unwrap().insert(info.token.clone(), info.account_id);
    Ok(Json(info))
}

#[derive(Deserialize)]
struct AddSymbolReq {
    symbol: String,
}

/// `POST /api/symbols {"symbol":"GBPJPY"}`: list a symbol. The engine adds it
/// once TraderMade accepts the subscription; clients get an `instruments` message.
async fn add_symbol(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<AddSymbolReq>) -> Result<Json<serde_json::Value>, ApiError> {
    s.account(&headers)?;
    let upper = req.symbol.trim().to_uppercase();
    let symbol = upper.strip_suffix(":QUOTE").unwrap_or(&upper).to_string();
    if !(3..=20).contains(&symbol.len()) || !symbol.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
        return Err(err(StatusCode::BAD_REQUEST, "use letters, digits and dots, e.g. GBPJPY or AAPL.US"));
    }
    if !s.live {
        return Err(err(StatusCode::BAD_REQUEST, "adding symbols needs a TraderMade streaming key"));
    }
    let name = symbol.clone();
    let status = s.ask(|reply| Command::AddSymbol { symbol: name, reply }).await?;
    if status == "requested" {
        let _ = s.subscribe.send(FeedCtl::Subscribe(symbol.clone()));
        if let Some(key) = s.rest_key.clone() {
            tokio::spawn(crate::backfill::run(key, vec![symbol.clone()], s.tx.clone()));
        }
    }
    Ok(Json(json!({"symbol": symbol, "status": status})))
}

/// `DELETE /api/symbols/GBPJPY`: take an added symbol off the list.
async fn remove_symbol(State(s): State<AppState>, headers: HeaderMap, Path(symbol): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    s.account(&headers)?;
    let symbol = symbol.trim().to_uppercase();
    let name = symbol.clone();
    s.ask(|reply| Command::RemoveSymbol { symbol: name, reply }).await?.map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let _ = s.subscribe.send(FeedCtl::Unsubscribe(symbol.clone()));
    Ok(Json(json!({"symbol": symbol, "status": "removed"})))
}

async fn me(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiError> {
    let account = s.account(&headers)?;
    Ok(Json(s.ask(|reply| Command::Account { account, reply }).await?))
}

fn ack_response(res: Result<OrderAck, String>, started: Instant) -> Response {
    match res {
        Ok(ack) => {
            let mut v = serde_json::to_value(&ack).unwrap();
            v["roundtrip_us"] = json!(started.elapsed().as_nanos() as f64 / 1000.0);
            Json(v).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn submit(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<OrderRequest>) -> Result<Response, ApiError> {
    let started = Instant::now();
    let account = s.account(&headers)?;
    let res = s.ask(|reply| Command::Submit { account, req, reply }).await?;
    Ok(ack_response(res, started))
}

async fn cancel(State(s): State<AppState>, headers: HeaderMap, Path(order_id): Path<OrderId>) -> Result<Response, ApiError> {
    let started = Instant::now();
    let account = s.account(&headers)?;
    let res = s.ask(|reply| Command::Cancel { account, order_id, reply }).await?;
    Ok(ack_response(res, started))
}

async fn modify(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(order_id): Path<OrderId>,
    Json(req): Json<ModifyRequest>,
) -> Result<Response, ApiError> {
    let started = Instant::now();
    let account = s.account(&headers)?;
    let res = s.ask(|reply| Command::Modify { account, order_id, req, reply }).await?;
    Ok(ack_response(res, started))
}

/// `GET /api/estimate?symbol=EURUSD&side=BUY&qty=5000000[&price=1.1260]`
async fn estimate(State(s): State<AppState>, headers: HeaderMap, Query(req): Query<EstimateRequest>) -> Result<Response, ApiError> {
    let account = s.account(&headers)?;
    match s.ask(|reply| Command::Estimate { account, req, reply }).await? {
        Ok(v) => Ok(Json(v).into_response()),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e)),
    }
}

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

async fn ws(State(s): State<AppState>, Query(q): Query<WsQuery>, upgrade: WebSocketUpgrade) -> Response {
    let stale_token = q.token.is_some();
    let account = q.token.and_then(|t| s.tokens.read().unwrap().get(&t).copied());
    let stale_token = stale_token && account.is_none();
    upgrade.on_upgrade(move |mut socket| async move {
        if stale_token {
            // Sessions are in-memory; tell the UI to sign in again after a restart.
            let msg = json!({"type": "account", "error": "unknown session"}).to_string();
            let _ = socket.send(Message::Text(msg)).await;
        }
        client(s, socket, account).await
    })
}

/// Stream engine output to one browser. On lag, resend a full snapshot
/// rather than leaving the client with a stale book.
async fn client(s: AppState, mut socket: WebSocket, account: Option<AccountId>) {
    let mut rx = s.out.subscribe();
    let mut need_snapshot = true;
    loop {
        if need_snapshot {
            let Ok(msgs) = s.ask(|reply| Command::Snapshot { account, reply }).await else { return };
            for m in msgs {
                if socket.send(Message::Text(m.to_string())).await.is_err() {
                    return;
                }
            }
            need_snapshot = false;
        }
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(m) => {
                    if m.account.is_some() && m.account != account {
                        continue;
                    }
                    if socket.send(Message::Text(m.json.to_string())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("ws client lagged by {n}; resnapshot");
                    rx = rx.resubscribe();
                    need_snapshot = true;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {}
            },
        }
    }
}
