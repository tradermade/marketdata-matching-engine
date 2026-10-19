import { useEffect, useRef } from 'react';
import { createChart, type IChartApi, type ISeriesApi, type UTCTimestamp } from 'lightweight-charts';
import type { Candle, Instrument, Quote } from '../lib/types';
import { symbolInfo } from '../lib/symbols';
import { fmtPx } from '../lib/useExchange';
import { Panel } from './Panel';

/** Spread in pips for FX, in price otherwise. */
function spreadText(inst: Instrument, q?: Quote) {
  if (!q || !(q.ask > q.bid)) return '—';
  const s = q.ask - q.bid;
  if (symbolInfo(inst.symbol).kind === 'FX') return `${(s / 10 ** -(inst.price_decimals - 1)).toFixed(1)} pips`;
  return s.toFixed(inst.price_decimals);
}

/** Change over the last 24 hours of candles. */
function dayChange(candles: Candle[]) {
  const last = candles[candles.length - 1];
  const since = last ? candles.find((k) => k.t >= last.t - 86_400_000) : undefined;
  return last && since && since.o > 0 ? ((last.c - since.o) / since.o) * 100 : null;
}

/** 1-minute candles of TraderMade's mid price, with the live bid, ask and spread above. */
export function PriceChart({ inst, candles, quote }: { inst: Instrument; candles: Candle[]; quote?: Quote }) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<IChartApi | null>(null);
  const price = useRef<ISeriesApi<'Candlestick'> | null>(null);

  useEffect(() => {
    if (!host.current) return;
    const c = createChart(host.current, {
      autoSize: true,
      layout: { background: { color: 'transparent' }, textColor: '#8A97AD', fontFamily: 'Roboto', fontSize: 12 },
      grid: { vertLines: { color: '#111A2B' }, horzLines: { color: '#111A2B' } },
      rightPriceScale: { borderColor: '#22304A' },
      timeScale: { borderColor: '#22304A', timeVisible: true, secondsVisible: false },
      crosshair: { mode: 0 },
    });
    price.current = c.addCandlestickSeries({
      upColor: '#389AC7',
      downColor: '#E5534B',
      borderVisible: false,
      wickUpColor: '#389AC7',
      wickDownColor: '#E5534B',
    });
    chart.current = c;
    return () => {
      c.remove();
      chart.current = null;
    };
  }, []);

  useEffect(() => {
    price.current?.applyOptions({
      priceFormat: { type: 'price', precision: inst.price_decimals, minMove: 1 / 10 ** inst.price_decimals },
    });
    chart.current?.timeScale().fitContent();
  }, [inst.symbol, inst.price_decimals]);

  useEffect(() => {
    price.current?.setData(candles.map((k) => ({ time: (k.t / 1000) as UTCTimestamp, open: k.o, high: k.h, low: k.l, close: k.c })));
  }, [candles]);

  return (
    <Panel
      title={
        <span className="flex items-baseline gap-2">
          {inst.symbol}
          <span className="text-[12px] font-normal text-mute">{symbolInfo(inst.symbol).name ?? symbolInfo(inst.symbol).kind} · 1m</span>
        </span>
      }
      right={<MarketBar inst={inst} quote={quote} change={dayChange(candles)} />}
      className="h-full"
    >
      <div ref={host} className="flex-1 min-h-[180px]" />
    </Panel>
  );
}

function MarketBar({ inst, quote, change }: { inst: Instrument; quote?: Quote; change: number | null }) {
  const item = (label: string, value: string, cls = 'text-white') => (
    <span className="flex items-baseline gap-1.5">
      <span className="text-[12px] text-mute">{label}</span>
      <span className={`font-mono text-[14px] font-medium ${cls}`}>{value}</span>
    </span>
  );
  return (
    <div className="hidden sm:flex items-baseline gap-5">
      {item('Bid', fmtPx(quote?.bid, inst.price_decimals), 'text-sell')}
      {item('Ask', fmtPx(quote?.ask, inst.price_decimals), 'text-buy')}
      {item('Spread', spreadText(inst, quote))}
      {change !== null &&
        item('24h', `${change >= 0 ? '+' : ''}${change.toFixed(2)}%`, change >= 0 ? 'text-buy' : 'text-sell')}
    </div>
  );
}
