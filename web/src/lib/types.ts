export type Side = 'BUY' | 'SELL';
export type OrderKind = 'LIMIT' | 'MARKET';
export type Tif = 'GTC' | 'IOC' | 'FOK';

export interface Instrument {
  symbol: string;
  price_decimals: number;
  qty_decimals: number;
  default_level_qty: number;
  max_order_qty: number;
  /** Added from the symbol search (and removable), rather than built in. */
  added: boolean;
  /** TraderMade accepted it but hasn't sent a price yet (e.g. market closed). */
  awaiting_price: boolean;
}

/** [price, qty, orderCount, userQty, estimated (TraderMade sent no size)] */
export type Level = [number, number, number, number, boolean];

export interface Book {
  bids: Level[];
  asks: Level[];
  last: number | null;
  orders: number;
}

export interface Quote {
  bid: number;
  ask: number;
  mid: number;
  bid_vol: number | null;
  ask_vol: number | null;
  ts: number;
  source: 'tradermade' | 'synthetic' | null;
}

export interface Trade {
  id: number;
  price: number;
  qty: number;
  side: Side;
  ts: number;
  taker_is_lp: boolean;
  maker_is_lp: boolean;
}

export interface Candle {
  t: number;
  o: number;
  h: number;
  l: number;
  c: number;
  ticks: number;
  volume: number;
}

export interface ExecReport {
  order_id: number;
  symbol: string;
  side: Side;
  exec_type: 'NEW' | 'PARTIAL_FILL' | 'FILL' | 'CANCELED' | 'REPLACED' | 'REJECTED';
  /** the order's limit price; 0 for a market order */
  price: number;
  last_px: number;
  last_qty: number;
  cum_qty: number;
  leaves_qty: number;
  avg_px: number;
  text: string | null;
  ts: number;
}

export interface QueuePosition {
  orders_ahead: number;
  qty_ahead: number;
  level_qty: number;
  level_orders: number;
}

export interface OpenOrder {
  order_id: number;
  symbol: string;
  side: Side;
  price: number;
  leaves_qty: number;
  cum_qty: number;
  queue: QueuePosition | null;
}

export interface FillRecord {
  fill_id: number;
  order_id: number;
  symbol: string;
  side: Side;
  liquidity: 'TAKER' | 'MAKER';
  px: number;
  qty: number;
  ts: number;
  arrival_mid: number;
  mid_at_fill: number;
  slippage_bps: number;
  /** +1s, +5s, +60s; positive = price moved in your favour */
  markouts_bps: (number | null)[];
}

export interface TcaSummary {
  fills: number;
  taker_fills: number;
  slippage_bps: number | null;
  markout_1s_bps: number | null;
  markout_5s_bps: number | null;
  markout_60s_bps: number | null;
}

export interface Estimate {
  filled_qty: number;
  fill_ratio: number;
  avg_px: number | null;
  best_px: number | null;
  worst_px: number | null;
  levels: number;
  mid: number;
  cost_bps: number | null;
  worst_bps: number | null;
  cost_quote: number | null;
  coverage: number;
}

export interface Position {
  symbol: string;
  qty: number;
  avg_px: number;
  realized: number;
  unrealized: number;
  mark: number;
}

export interface Account {
  account_id: number;
  name: string;
  open_orders: OpenOrder[];
  positions: Position[];
  execs: ExecReport[];
  fills: FillRecord[];
  tca: TcaSummary;
}

export interface OpStats {
  count: number;
  p50_us: number;
  p99_us: number;
  p999_us: number;
  max_us: number;
  heat: number[];
}

export interface Metrics {
  ops: Record<string, OpStats>;
  ops_per_sec: number;
  ticks_per_sec: number;
  tick_to_book_p50_us: number;
  tick_to_book_p99_us: number;
  resting_orders: number;
  accounts: number;
  uptime_s: number;
  seq: number;
}

export interface FeedStatus {
  mode: string;
  connected: boolean;
  accepted: string[];
  /** symbol -> reason, e.g. cfds_not_allowed, limit_reached, toggle_cooldown */
  denied: Record<string, string>;
  /** symbols TraderMade didn't recognise in this form */
  invalid: string[];
  message: string | null;
}

export interface Session {
  account_id: number;
  name: string;
  token: string;
}
