import { useState } from 'react';
import { Download, Terminal } from 'lucide-react';
import type { Account, Instrument, Trade } from '../lib/types';
import { describe, type Tone } from '../lib/describe';
import { fmtQty, fmtTime } from '../lib/useExchange';
import { Empty, Panel, Toggle } from './Panel';

const toneDot: Record<Tone, string> = { fill: 'bg-buy', info: 'bg-mute/60', warn: 'bg-sell' };

/** The trader's own orders and fills, one plain sentence each, newest first. */
export function Activity({ account, instruments }: { account: Account | null; instruments: Instrument[] }) {
  const specOf = (s: string) => instruments.find((i) => i.symbol === s);
  // a market order's "sent" line adds nothing next to its fill
  const reports = (account?.execs ?? []).filter((r) => !(r.exec_type === 'NEW' && r.price === 0));
  return (
    <Panel className="flex-1" title="Activity">
      <div className="flex-1 overflow-y-auto pr-1 min-h-0">
        {reports.length === 0 && <Empty>Your orders and fills will show here</Empty>}
        {reports.map((r, i) => {
          const spec = specOf(r.symbol);
          const { tone, text } = describe(r, spec?.price_decimals ?? 5, spec?.qty_decimals ?? 0);
          return (
            <div key={`${r.order_id}-${i}`} className="flex items-start gap-2.5 px-1 py-1.5 border-b border-white/[0.04] text-[13px]">
              <span className={`mt-1.5 w-1.5 h-1.5 rounded-full shrink-0 ${toneDot[tone]}`} />
              <span className={`flex-1 min-w-0 ${tone === 'warn' ? 'text-sell/90' : tone === 'fill' ? 'text-white' : 'text-mute'}`}>{text}</span>
              <span className="text-[12px] text-mute font-mono shrink-0">{fmtTime(r.ts).slice(0, 8)}</span>
            </div>
          );
        })}
      </div>
    </Panel>
  );
}

export function Trades({ inst, trades, account }: { inst: Instrument; trades: Trade[]; account: Account | null }) {
  const [view, setView] = useState<'PUBLIC' | 'MINE'>('PUBLIC');
  const dp = inst.price_decimals;
  const qdp = inst.qty_decimals;
  const mine = (account?.execs ?? []).filter((e) => e.last_qty > 0 && e.symbol === inst.symbol);

  const csv = () => {
    const rows = [['id', 'symbol', 'side', 'price', 'qty', 'time'].join(','), ...trades.map((t) => [t.id, inst.symbol, t.side, t.price, t.qty, new Date(t.ts).toISOString()].join(','))];
    const url = URL.createObjectURL(new Blob([rows.join('\n')], { type: 'text/csv' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `${inst.symbol}_trades_${Date.now()}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <Panel
      className="flex-1"
      title={
        <Toggle
          value={view}
          onChange={setView}
          accent="text-buy"
          options={[
            { value: 'PUBLIC', label: 'Exchange trades' },
            { value: 'MINE', label: `My fills (${mine.length})` },
          ]}
        />
      }
      right={
        <button onClick={csv} className="text-[12px] bg-bg/60 ring-1 ring-white/10 hover:ring-white/25 text-mute hover:text-white px-2.5 py-0.5 rounded-full flex items-center gap-1 cursor-pointer transition-all" title="Download CSV">
          <Download className="w-3 h-3" /> CSV
        </button>
      }
    >
      <div className="grid grid-cols-3 text-[12px] text-mute uppercase pb-1.5 mb-1 border-b border-line px-1">
        <span>Price</span>
        <span className="text-right">Size</span>
        <span className="text-right">Time</span>
      </div>
      <div className="flex-1 overflow-y-auto pr-1 space-y-0.5 font-mono text-[13px] min-h-0">
        {view === 'PUBLIC' ? (
          trades.length === 0 ? (
            <Empty>No trades yet — every fill on this exchange shows here</Empty>
          ) : (
            trades.map((t) => (
              <div key={t.id} className={`grid grid-cols-3 px-1 py-0.5 rounded ${t.side === 'BUY' ? 'text-buy bg-buy/5' : 'text-sell bg-sell/5'}`}>
                <span>{t.price.toFixed(dp)}</span>
                <span className="text-right text-white">{fmtQty(t.qty, qdp)}</span>
                <span className="text-right text-mute">{fmtTime(t.ts)}</span>
              </div>
            ))
          )
        ) : mine.length === 0 ? (
          <Empty>No fills in {inst.symbol}</Empty>
        ) : (
          mine.map((e, i) => (
            <div key={i} className={`grid grid-cols-3 px-1 py-0.5 rounded ${e.side === 'BUY' ? 'text-buy bg-buy/5' : 'text-sell bg-sell/5'}`}>
              <span>{e.last_px.toFixed(dp)}</span>
              <span className="text-right text-white">{fmtQty(e.last_qty, qdp)}</span>
              <span className="text-right text-mute">{fmtTime(e.ts)}</span>
            </div>
          ))
        )}
      </div>
    </Panel>
  );
}

const execColor: Record<string, string> = {
  NEW: 'text-blue-400',
  PARTIAL_FILL: 'text-amber-400',
  FILL: 'text-buy',
  CANCELED: 'text-sell',
  REJECTED: 'text-sell',
  REPLACED: 'text-blue-400',
};

export function ExecReports({ account, instruments }: { account: Account | null; instruments: Instrument[] }) {
  const reports = account?.execs ?? [];
  const dpOf = (s: string) => instruments.find((i) => i.symbol === s)?.price_decimals ?? 5;
  return (
    <Panel className="flex-1" title="Execution Reports (FIX-style)" icon={<Terminal className="w-4 h-4 text-buy" />}>
      <div className="flex-1 overflow-y-auto pr-1 space-y-2 font-mono text-[12px] min-h-0">
        {reports.length === 0 && <Empty>Waiting for orders…</Empty>}
        {reports.map((r, i) => (
          <div key={`${r.order_id}-${i}`} className="p-2.5 rounded-lg bg-panel2/70 ring-1 ring-white/5 text-mute space-y-1">
            <div className="flex justify-between text-white">
              <span className="font-bold text-blue-400">← 35=8 EXECUTION_REPORT</span>
              <span>{fmtTime(r.ts)}</span>
            </div>
            <div className="grid grid-cols-2 gap-x-4">
              <span>order_id: <span className="text-white">{r.order_id}</span></span>
              <span>exec_type: <span className={execColor[r.exec_type]}>{r.exec_type}</span></span>
              <span>symbol: <span className="text-white">{r.symbol} {r.side}</span></span>
              <span>last_px: <span className="text-white">{r.last_px > 0 ? r.last_px.toFixed(dpOf(r.symbol)) : '-'}</span></span>
              <span>cum_qty: <span className="text-white">{r.cum_qty}</span></span>
              <span>leaves_qty: <span className="text-white">{r.leaves_qty}</span></span>
              {r.avg_px > 0 && <span>avg_px: <span className="text-white">{r.avg_px.toFixed(dpOf(r.symbol) + 1)}</span></span>}
              {r.text && <span className="col-span-2">text: <span className="text-amber-400">{r.text}</span></span>}
            </div>
          </div>
        ))}
      </div>
    </Panel>
  );
}
