import { useEffect, useRef, useState } from 'react';
import type { Account, Book, Candle, ExecReport, FeedStatus, Instrument, Metrics, Quote, Trade } from './types';

export interface ExchangeState {
  connected: boolean;
  instruments: Instrument[];
  /** Every symbol the streaming key can subscribe to, for the symbol search. */
  symbols: string[];
  feed: FeedStatus | null;
  books: Record<string, Book>;
  quotes: Record<string, Quote>;
  trades: Record<string, Trade[]>;
  candles: Record<string, Candle[]>;
  metrics: Metrics | null;
  account: Account | null;
  logs: string[];
}

const empty: ExchangeState = {
  connected: false,
  instruments: [],
  symbols: [],
  feed: null,
  books: {},
  quotes: {},
  trades: {},
  candles: {},
  metrics: null,
  account: null,
  logs: [],
};

const MAX_TRADES = 60;
const MAX_LOGS = 200;
const MAX_CANDLES = 240;

/**
 * Live exchange state from the server's WebSocket. Messages are folded into a
 * mutable draft and flushed to React at most once per animation frame, so a
 * burst of ticks costs one render.
 */
export function useExchange(
  token: string | null,
  onAuthLost: () => void,
  onExec?: (r: ExecReport) => void,
): ExchangeState {
  const [state, setState] = useState<ExchangeState>(empty);
  const draft = useRef<ExchangeState>(empty);
  const authLost = useRef(onAuthLost);
  authLost.current = onAuthLost;
  const execRef = useRef(onExec);
  execRef.current = onExec;

  useEffect(() => {
    let ws: WebSocket | null = null;
    let closed = false;
    let frame = 0;
    let fallback: ReturnType<typeof setTimeout> | undefined;
    let retry = 0;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;

    const flush = () => {
      if (frame) cancelAnimationFrame(frame);
      clearTimeout(fallback);
      frame = 0;
      fallback = undefined;
      setState({ ...draft.current });
    };
    // rAF batches a burst of messages into one render, but browsers pause it
    // in hidden tabs; the timeout keeps state flowing there.
    const schedule = () => {
      if (frame || fallback) return;
      frame = requestAnimationFrame(flush);
      fallback = setTimeout(flush, 250);
    };

    const apply = (m: any) => {
      const d = draft.current;
      switch (m.type) {
        case 'hello':
          d.instruments = m.instruments;
          d.symbols = m.symbols ?? [];
          d.feed = m.feed;
          d.logs = m.logs;
          break;
        case 'instruments':
          d.instruments = m.instruments;
          break;
        case 'symbols':
          d.symbols = m.symbols;
          break;
        case 'feed':
          d.feed = m.feed;
          break;
        case 'book':
          d.books = { ...d.books, [m.symbol]: { bids: m.bids, asks: m.asks, last: m.last, orders: m.orders } };
          break;
        case 'quote':
          d.quotes = { ...d.quotes, [m.symbol]: m };
          break;
        case 'trades':
          d.trades = { ...d.trades, [m.symbol]: m.trades };
          break;
        case 'trade':
          d.trades = { ...d.trades, [m.symbol]: [m.trade, ...(d.trades[m.symbol] ?? [])].slice(0, MAX_TRADES) };
          break;
        case 'candles':
          d.candles = { ...d.candles, [m.symbol]: m.candles };
          break;
        case 'candle': {
          const list = d.candles[m.symbol] ?? [];
          const last = list[list.length - 1];
          const next = last && last.t === m.candle.t ? [...list.slice(0, -1), m.candle] : [...list, m.candle];
          d.candles = { ...d.candles, [m.symbol]: next.slice(-MAX_CANDLES) };
          break;
        }
        case 'metrics':
          d.metrics = m;
          break;
        case 'account':
          if (m.error) authLost.current();
          else d.account = m;
          break;
        case 'exec':
          // full account state follows within 100ms; this is for pop-ups
          execRef.current?.(m.report);
          break;
        case 'log':
          d.logs = [m.line, ...d.logs].slice(0, MAX_LOGS);
          break;
      }
    };

    const connect = () => {
      const proto = location.protocol === 'https:' ? 'wss' : 'ws';
      const q = token ? `?token=${encodeURIComponent(token)}` : '';
      ws = new WebSocket(`${proto}://${location.host}/ws${q}`);
      ws.onopen = () => {
        retry = 0;
        draft.current = { ...draft.current, connected: true };
        schedule();
      };
      ws.onmessage = (ev) => {
        try {
          apply(JSON.parse(ev.data));
          schedule();
        } catch {
          /* ignore malformed frame */
        }
      };
      ws.onclose = () => {
        draft.current = { ...draft.current, connected: false };
        schedule();
        if (!closed) {
          retry = Math.min(retry + 1, 6);
          retryTimer = setTimeout(connect, 250 * 2 ** retry);
        }
      };
    };

    draft.current = { ...empty };
    connect();
    return () => {
      closed = true;
      clearTimeout(retryTimer);
      clearTimeout(fallback);
      if (frame) cancelAnimationFrame(frame);
      ws?.close();
    };
  }, [token]);

  return state;
}

export function fmtPx(v: number | null | undefined, dp: number) {
  return v == null || !isFinite(v) ? '—' : v.toFixed(dp);
}

export function fmtQty(v: number, dp: number) {
  if (dp === 0) {
    if (v >= 1e6) return `${(v / 1e6).toFixed(v % 1e6 === 0 ? 0 : 2)}M`;
    if (v >= 1e3) return `${(v / 1e3).toFixed(v % 1e3 === 0 ? 0 : 1)}K`;
    return v.toFixed(0);
  }
  return v.toFixed(dp);
}

export function fmtTime(ts: number) {
  return new Date(ts).toISOString().split('T')[1].slice(0, -1);
}
