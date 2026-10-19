use tm_engine::SymbolSpec;

/// Per-instrument settings on top of the engine's fixed-point spec.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Instrument {
    #[serde(flatten)]
    pub spec: SymbolSpec,
    /// Size quoted when the feed sends none (e.g. the last price of a closed market).
    pub default_level_qty: f64,
    /// Largest single order a user may send.
    pub max_order_qty: f64,
    /// Starting price for the synthetic feed.
    pub seed_price: f64,
    /// Relative spread (fraction of price) for the synthetic feed.
    pub seed_spread: f64,
    /// Added from the UI at runtime (and removable), rather than built in.
    pub added: bool,
    /// TraderMade accepted the symbol but hasn't sent a price (e.g. its market
    /// is closed). Settings are provisional until the first quote.
    pub awaiting_price: bool,
}

impl Instrument {
    fn new(symbol: &str, px_dp: u32, qty_dp: u32, level_qty: f64, max_qty: f64, seed: f64, spread: f64) -> Self {
        Instrument {
            spec: SymbolSpec::new(symbol, px_dp, qty_dp),
            default_level_qty: level_qty,
            max_order_qty: max_qty,
            seed_price: seed,
            seed_spread: spread,
            added: false,
            awaiting_price: false,
        }
    }
}

/// Symbols added from the UI are kept here so they come back after a restart.
pub const ADDED_SYMBOLS_FILE: &str = "data/added-symbols.json";

pub fn load_added_symbols() -> Vec<String> {
    std::fs::read_to_string(ADDED_SYMBOLS_FILE).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_added_symbols(symbols: &[String]) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all("data")?;
        std::fs::write(ADDED_SYMBOLS_FILE, serde_json::to_string_pretty(symbols)?)
    };
    if let Err(e) = write() {
        tracing::warn!("could not save added symbols: {e}");
    }
}

/// Instruments the exchange lists. FX sizes are base-currency units (the same
/// unit TraderMade uses for `bv`/`av`).
pub fn instruments() -> Vec<Instrument> {
    vec![
        Instrument::new("EURUSD", 5, 0, 1_000_000.0, 50_000_000.0, 1.1257, 0.00006),
        Instrument::new("GBPUSD", 5, 0, 1_000_000.0, 50_000_000.0, 1.3241, 0.00008),
        Instrument::new("USDJPY", 3, 0, 1_000_000.0, 50_000_000.0, 157.83, 0.00007),
        Instrument::new("XAUUSD", 2, 2, 100.0, 5_000.0, 4143.5, 0.00017),
        Instrument::new("BTCUSD", 2, 4, 2.0, 100.0, 85_000.0, 0.00006),
    ]
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Fx,
    Metal,
    Crypto,
    Other,
}

const CURRENCIES: &[&str] = &[
    "USD", "EUR", "GBP", "JPY", "CHF", "AUD", "NZD", "CAD", "SEK", "NOK", "DKK", "PLN", "HUF", "CZK", "TRY", "ZAR",
    "MXN", "SGD", "HKD", "CNH", "CNY", "INR", "IDR", "KRW", "THB", "PHP", "MYR", "TWD", "BRL", "CLP", "COP", "PEN",
    "ILS", "AED", "SAR", "KWD", "QAR", "RUB", "RON", "BGN", "ISK",
];
const METALS: &[&str] = &["XAU", "XAG", "XPT", "XPD"];
const CRYPTO: &[&str] = &[
    "BTC", "ETH", "LTC", "XRP", "BCH", "ADA", "SOL", "DOT", "DOGE", "LINK", "BNB", "AVAX", "TRX", "XLM", "UNI", "ATOM",
    "ETC", "FIL", "SHIB", "MATIC",
];

fn classify(symbol: &str) -> Class {
    if symbol.len() == 6 && symbol.chars().all(|c| c.is_ascii_uppercase()) {
        let (base, quote) = symbol.split_at(3);
        if METALS.contains(&base) {
            return Class::Metal;
        }
        if CRYPTO.contains(&base) {
            return Class::Crypto;
        }
        if CURRENCIES.contains(&base) && CURRENCIES.contains(&quote) {
            return Class::Fx;
        }
    }
    if CRYPTO.iter().any(|c| symbol.starts_with(c)) || symbol.ends_with("USDT") {
        return Class::Crypto;
    }
    Class::Other
}

/// Round to one significant figure, so default sizes look like 60 or 2,000.
fn round_size(x: f64) -> f64 {
    if !(x > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(x.log10().floor());
    ((x / p).round() * p).max(1.0)
}

/// Settings for a symbol added while the server is running, worked out from
/// its asset class and first quote. Price decimals are never coarser than the
/// spread, so the bid and ask can't round onto the same tick.
pub fn instrument_for(symbol: &str, bid: f64, ask: f64) -> Instrument {
    if let Some(i) = instruments().into_iter().find(|i| i.spec.symbol == symbol) {
        return i;
    }
    let mid = (bid + ask) / 2.0;
    let spread = (ask - bid).abs();
    let class = classify(symbol);
    let class_dp = match class {
        Class::Fx if mid < 50.0 => 5,
        Class::Fx if mid < 1000.0 => 3,
        Class::Fx => 2,
        _ if mid < 1.0 => 6,
        _ if mid < 10.0 => 4,
        _ if mid < 1000.0 => 3,
        _ => 2,
    };
    let spread_dp = if spread > 0.0 { (-spread.log10() - 1e-6).ceil().max(0.0) as u32 } else { 0 };
    let price_dp = class_dp.max(spread_dp).min(8);
    let (qty_dp, level) = match class {
        // FX sizes are base-currency units, like TraderMade's bv/av
        Class::Fx => (0, 1_000_000.0),
        Class::Crypto => (4, round_size(250_000.0 / mid)),
        Class::Metal | Class::Other => (2, round_size(250_000.0 / mid)),
    };
    let max = if class == Class::Fx { 50_000_000.0 } else { level * 50.0 };
    let mut inst = Instrument::new(symbol, price_dp, qty_dp, level, max, mid, if mid > 0.0 { spread / mid } else { 0.0 });
    inst.added = true;
    inst
}

/// Provisional settings for a symbol TraderMade has accepted but not priced
/// yet. `instrument_for` replaces them on the first quote; orders are refused
/// until then, so nothing is ever priced with these decimals.
pub fn instrument_unpriced(symbol: &str) -> Instrument {
    let (px_dp, qty_dp, level) = match classify(symbol) {
        Class::Fx if symbol.ends_with("JPY") => (3, 0, 1_000_000.0),
        Class::Fx => (5, 0, 1_000_000.0),
        Class::Crypto => (2, 4, 1.0),
        Class::Metal | Class::Other => (2, 2, 1.0),
    };
    let mut inst = Instrument::new(symbol, px_dp, qty_dp, level, level * 50.0, 0.0, 0.0);
    inst.added = true;
    inst.awaiting_price = true;
    inst
}

pub struct Config {
    /// Streaming key (WebSocket V2).
    pub api_key: Option<String>,
    /// REST key, used to backfill chart history. TraderMade issues these separately.
    pub rest_key: Option<String>,
    pub bind: String,
    pub static_dir: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        // Same names as TraderMade's CLI: separate WS/REST keys, with
        // TRADERMADE_API_KEY as the fallback for both.
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let shared = var("TRADERMADE_API_KEY");
        let api_key = var("TRADERMADE_WS_API_KEY").or_else(|| shared.clone());
        let rest_key = var("TRADERMADE_REST_API_KEY").or(shared);
        Config {
            api_key,
            rest_key,
            bind: std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8080".into()),
            static_dir: std::env::var("STATIC_DIR").ok().or_else(|| {
                std::path::Path::new("web/dist").exists().then(|| "web/dist".to_string())
            }),
        }
    }
}
