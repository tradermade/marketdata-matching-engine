mod api;
mod backfill;
mod config;
mod engine;
mod feed;
mod messages;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tm_server=info".into()))
        .init();

    let cfg = config::Config::from_env();
    let instruments = config::instruments();

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    let (out_tx, _) = broadcast::channel(8192);

    // The matching engine gets a dedicated OS thread; nothing else touches its state.
    let engine = engine::Engine::new(instruments.clone(), out_tx.clone());
    std::thread::Builder::new().name("engine".into()).spawn(move || engine.run(cmd_rx))?;

    // Symbols added from the UI reach the feed task through this channel.
    let (subscribe_tx, subscribe_rx) = tokio::sync::mpsc::unbounded_channel();

    // Symbols added in an earlier run come back once TraderMade accepts them again.
    let mut symbols: Vec<String> = instruments.iter().map(|i| i.spec.symbol.clone()).collect();
    if cfg.api_key.is_some() {
        for symbol in config::load_added_symbols() {
            if symbols.contains(&symbol) {
                continue;
            }
            let (reply, _) = tokio::sync::oneshot::channel();
            let _ = cmd_tx.send(messages::Command::AddSymbol { symbol: symbol.clone(), reply });
            let _ = subscribe_tx.send(messages::FeedCtl::Subscribe(symbol.clone()));
            symbols.push(symbol);
        }
        if symbols.len() > instruments.len() {
            tracing::info!("restoring added symbols: {:?}", &symbols[instruments.len()..]);
        }
    }
    if let Some(key) = cfg.rest_key.clone() {
        tokio::spawn(backfill::run(key, symbols, cmd_tx.clone()));
    }
    tokio::spawn(feed::run(cfg.api_key.clone(), instruments, cmd_tx.clone(), subscribe_rx));

    let state = api::AppState {
        tx: cmd_tx,
        out: out_tx,
        tokens: Arc::new(RwLock::new(HashMap::new())),
        subscribe: subscribe_tx,
        live: cfg.api_key.is_some(),
        rest_key: cfg.rest_key.clone(),
    };
    let mut app = api::router(state).layer(CorsLayer::permissive());
    if let Some(dir) = &cfg.static_dir {
        let index = format!("{dir}/index.html");
        app = app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
        tracing::info!("serving UI from {dir}");
    }

    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(
        "listening on http://{} (feed: {})",
        cfg.bind,
        if cfg.api_key.is_some() { "TraderMade WebSocket V2" } else { "synthetic" }
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
