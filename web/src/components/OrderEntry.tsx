import { useState } from 'react';
import type { ExecReport, Instrument, OrderKind, Quote, Side, Tif } from '../lib/types';
import { api } from '../lib/api';
import { describe, type Tone } from '../lib/describe';
import { fmtPx, fmtQty } from '../lib/useExchange';
import { Panel, Toggle } from './Panel';
import { CostPreview } from './CostPreview';

const KINDS = [
  { value: 'MARKET' as const, label: 'Market', title: 'Fills now at the best price available' },
  { value: 'LIMIT' as const, label: 'Limit', title: 'Fills only at your price or better' },
];

const TIFS = [
  { value: 'GTC' as const, label: 'GTC', title: 'Good till cancelled: whatever doesn’t fill stays open until it fills or you cancel it' },
  { value: 'IOC' as const, label: 'IOC', title: 'Immediate or cancel: fills what it can straight away and cancels the rest' },
  { value: 'FOK' as const, label: 'FOK', title: 'Fill or kill: fills the whole size straight away, or nothing at all' },
];

/** What happened to an order just sent, from its execution reports. */
function summarise(reports: ExecReport[], inst: Instrument): { tone: Tone; text: string } {
  const last = reports[reports.length - 1];
  if (!last) return { tone: 'info', text: 'Order sent' };
  const fills = reports.filter((r) => r.last_qty > 0).length;
  const avg = last.avg_px.toFixed(inst.price_decimals + (fills > 1 ? 1 : 0));
  const verb = last.side === 'BUY' ? 'Bought' : 'Sold';
  const qty = (v: number) => fmtQty(v, inst.qty_decimals);
  if (last.exec_type === 'FILL') {
    return { tone: 'fill', text: `${verb} ${qty(last.cum_qty)} ${last.symbol} at ${fills > 1 ? 'an average of ' : ''}${avg}` };
  }
  if (last.exec_type === 'PARTIAL_FILL') {
    return {
      tone: 'fill',
      text: `${verb} ${qty(last.cum_qty)} at ${avg}; the other ${qty(last.leaves_qty)} is open at ${last.price.toFixed(inst.price_decimals)}`,
    };
  }
  if (last.exec_type === 'NEW') {
    return { tone: 'info', text: `${describe(last, inst.price_decimals, inst.qty_decimals).text}. It fills when the market reaches your price.` };
  }
  return describe(last, inst.price_decimals, inst.qty_decimals);
}

export function OrderEntry({ inst, quote, token }: { inst: Instrument; quote?: Quote; token: string }) {
  const [kind, setKind] = useState<OrderKind>('MARKET');
  const [tif, setTif] = useState<Tif>('GTC');
  const [postOnly, setPostOnly] = useState(false);
  const [qty, setQty] = useState(String(inst.default_level_qty / 10));
  // The limit price follows the market's mid until the trader sets one.
  const [typed, setTyped] = useState<string | null>(null);
  const [symbolFor, setSymbolFor] = useState(inst.symbol);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ tone: Tone; text: string; us?: number } | null>(null);

  if (symbolFor !== inst.symbol) {
    setSymbolFor(inst.symbol);
    setQty(String(inst.default_level_qty / 10));
    setTyped(null);
    setResult(null);
  }

  const dp = inst.price_decimals;
  const price = typed ?? (quote ? quote.mid.toFixed(dp) : '');
  const base = inst.symbol.slice(0, 3);
  const quoteCcy = inst.symbol.slice(3);
  const presets = [0.1, 0.5, 1, 5].map((m) => inst.default_level_qty * m);
  const limit = kind === 'LIMIT';
  const priced = !!quote && quote.bid > 0 && !inst.awaiting_price;

  const send = async (side: Side) => {
    const q = parseFloat(qty);
    const p = parseFloat(price);
    if (!(q > 0)) return setResult({ tone: 'warn', text: 'Enter a size' });
    if (limit && !(p > 0)) return setResult({ tone: 'warn', text: 'Enter a limit price' });
    setBusy(true);
    try {
      const ack = await api.submit(token, {
        symbol: inst.symbol,
        side,
        type: kind,
        price: limit ? p : undefined,
        qty: q,
        tif: limit ? tif : 'IOC',
        post_only: limit && postOnly,
      });
      setResult({ ...summarise(ack.reports, inst), us: ack.engine_ns / 1000 });
    } catch (e: any) {
      setResult({ tone: 'warn', text: e.message });
    } finally {
      setBusy(false);
    }
  };

  const input = 'field w-full text-white px-3 py-2 text-sm font-mono';
  const sidePx = (side: Side) => (limit ? price || '—' : fmtPx(side === 'BUY' ? quote?.ask : quote?.bid, dp));

  return (
    <Panel
      title="Order Entry"
      className="flex-shrink-0"
      right={<Toggle value={kind} onChange={setKind} options={KINDS} accent="text-amber-400" />}
    >
      <div className="flex gap-3 mb-3">
        {limit && (
          <div className="flex-1">
            <label className="flex justify-between items-center text-[12px] text-mute uppercase mb-1">
              <span>Limit price ({quoteCcy})</span>
              {quote && (
                <span className="flex gap-2 normal-case">
                  <button className="hover:text-sell cursor-pointer" title="Use the bid" onClick={() => setTyped(quote.bid.toFixed(dp))}>
                    bid
                  </button>
                  <button className="hover:text-buy cursor-pointer" title="Use the offer" onClick={() => setTyped(quote.ask.toFixed(dp))}>
                    ask
                  </button>
                  {typed === null ? (
                    <span className="text-amber-400" title="Moves with the market until you type a price">
                      following mid
                    </span>
                  ) : (
                    <button className="hover:text-white cursor-pointer" title="Follow the market's mid again" onClick={() => setTyped(null)}>
                      ↺ mid
                    </button>
                  )}
                </span>
              )}
            </label>
            <input type="number" step={1 / 10 ** dp} value={price} onChange={(e) => setTyped(e.target.value)} className={input} />
          </div>
        )}
        <div className="flex-1">
          <label className="block text-[12px] text-mute uppercase mb-1">Size ({base})</label>
          <input type="number" value={qty} onChange={(e) => setQty(e.target.value)} className={input} />
        </div>
      </div>

      <div className="flex items-center justify-between gap-2 mb-3 flex-wrap">
        <div className="flex gap-1">
          {presets.map((p) => (
            <button
              key={p}
              onClick={() => setQty(String(p))}
              className="text-[12px] font-mono bg-bg/60 ring-1 ring-white/10 hover:ring-white/25 text-mute hover:text-white px-2.5 py-0.5 rounded-full cursor-pointer transition-all"
            >
              {fmtQty(p, inst.qty_decimals)}
            </button>
          ))}
        </div>
        {limit && (
          <div className="flex items-center gap-2">
            <Toggle value={tif} onChange={setTif} options={TIFS} />
            <label
              className="flex items-center gap-1 text-[12px] text-mute uppercase cursor-pointer select-none"
              title="Only adds liquidity: if the order would trade straight away, it’s rejected instead"
            >
              <input type="checkbox" checked={postOnly} onChange={(e) => setPostOnly(e.target.checked)} className="accent-[#389AC7]" />
              Post-only
            </label>
          </div>
        )}
      </div>

      <div className="flex gap-3">
        {(['SELL', 'BUY'] as const).map((side) => (
          <button
            key={side}
            disabled={busy || !priced}
            onClick={() => send(side)}
            title={!priced ? `No price for ${inst.symbol} yet; its market may be closed` : limit ? `${side === 'BUY' ? 'Buy' : 'Sell'} at ${price} or better` : side === 'BUY' ? 'Buy at the offer' : 'Sell at the bid'}
            className={`flex-1 flex flex-col items-center py-2 rounded-lg text-white hover:brightness-110 active:brightness-95 transition-all cursor-pointer disabled:opacity-50 ${
              side === 'BUY' ? 'bg-buy shadow-[0_6px_16px_-6px_rgba(56,154,199,0.6)]' : 'bg-sell shadow-[0_6px_16px_-6px_rgba(229,83,75,0.6)]'
            }`}
          >
            <span className="text-[13px] font-medium opacity-90">
              {side} {limit ? 'LIMIT' : base}
            </span>
            <span className="text-lg font-semibold font-mono leading-tight">{sidePx(side)}</span>
          </button>
        ))}
      </div>
      <CostPreview inst={inst} token={token} qty={parseFloat(qty)} price={parseFloat(price)} kind={kind} />
      {result && (
        <div
          className={`mt-2 text-[13px] flex justify-between gap-3 ${
            result.tone === 'warn' ? 'text-sell' : result.tone === 'fill' ? 'text-white' : 'text-mute'
          }`}
        >
          <span className="min-w-0">{result.text}</span>
          {result.us !== undefined && (
            <span className="text-mute font-mono shrink-0" title="Time inside the matching engine">
              {result.us.toFixed(1)}µs
            </span>
          )}
        </div>
      )}
    </Panel>
  );
}
