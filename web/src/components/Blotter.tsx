import { useState } from 'react';
import { Check, Edit2, X } from 'lucide-react';
import type { Account, Instrument } from '../lib/types';
import { api } from '../lib/api';
import { fmtQty } from '../lib/useExchange';
import { Empty, Panel, Toggle } from './Panel';
import { ExecutionQuality } from './ExecutionQuality';

/** Open orders and positions for the signed-in account. */
export function Blotter({ account, token, instruments }: { account: Account | null; token: string; instruments: Instrument[] }) {
  const [view, setView] = useState<'ORDERS' | 'POSITIONS' | 'TCA'>('ORDERS');
  const [editing, setEditing] = useState<number | null>(null);
  const [editPx, setEditPx] = useState('');
  const [editQty, setEditQty] = useState('');
  const spec = (s: string) => instruments.find((i) => i.symbol === s);

  const orders = [...(account?.open_orders ?? [])].sort((a, b) => b.order_id - a.order_id);
  const positions = (account?.positions ?? []).filter((p) => p.qty !== 0 || p.realized !== 0);
  const totalPnl = positions.reduce((s, p) => s + p.realized + p.unrealized, 0);

  return (
    <Panel
      className="flex-1"
      right={
        <span className={`text-[12px] font-mono ${totalPnl >= 0 ? 'text-buy' : 'text-sell'}`}>
          PnL {totalPnl >= 0 ? '+' : ''}
          {totalPnl.toFixed(2)}
        </span>
      }
      title={
        <Toggle
          value={view}
          onChange={setView}
          accent="text-buy"
          options={[
            { value: 'ORDERS', label: `Open orders (${orders.length})` },
            { value: 'POSITIONS', label: `Positions (${positions.length})` },
            { value: 'TCA', label: 'Execution quality' },
          ]}
        />
      }
    >
      <div className="flex-1 overflow-y-auto pr-1 space-y-1 font-mono text-[13px] min-h-0">
        {view === 'TCA' ? (
          <ExecutionQuality account={account} instruments={instruments} />
        ) : view === 'ORDERS' ? (
          orders.length === 0 ? (
            <Empty>No open orders</Empty>
          ) : (
            orders.map((o) => {
              const s = spec(o.symbol);
              const dp = s?.price_decimals ?? 5;
              const qdp = s?.qty_decimals ?? 0;
              return (
                <div key={o.order_id} className="rounded-lg px-3 py-2 bg-panel2/70 ring-1 ring-white/5">
                  {editing === o.order_id ? (
                    <div className="flex items-center gap-2">
                      <input type="number" className="field w-24 text-white px-2 py-1 text-[12px]" value={editPx} onChange={(e) => setEditPx(e.target.value)} />
                      <input type="number" className="field w-24 text-white px-2 py-1 text-[12px]" value={editQty} onChange={(e) => setEditQty(e.target.value)} />
                      <button
                        className="text-buy hover:text-white p-1 rounded hover:bg-line cursor-pointer"
                        onClick={async () => {
                          const p = parseFloat(editPx), q = parseFloat(editQty);
                          if (p > 0 && q > 0) await api.modify(token, o.order_id, p, q).catch(() => {});
                          setEditing(null);
                        }}
                      >
                        <Check className="w-4 h-4" />
                      </button>
                      <button className="text-mute hover:text-white p-1 rounded hover:bg-line cursor-pointer" onClick={() => setEditing(null)}>
                        <X className="w-4 h-4" />
                      </button>
                    </div>
                  ) : (
                    <div className="flex items-center justify-between">
                      <div className="flex gap-3 items-center">
                        <span className="text-mute w-14">{o.symbol}</span>
                        <span className={`w-9 ${o.side === 'BUY' ? 'text-buy' : 'text-sell'}`}>{o.side}</span>
                        <span className="text-white w-16 text-right">{fmtQty(o.leaves_qty, qdp)}</span>
                        <span className="text-mute">@ {o.price.toFixed(dp)}</span>
                        {o.cum_qty > 0 && <span className="text-[12px] text-amber-400">filled {fmtQty(o.cum_qty, qdp)}</span>}
                        {o.queue && (
                          <span
                            className="text-[12px] text-blue-400"
                            title={`${o.queue.orders_ahead} order(s), ${fmtQty(o.queue.qty_ahead, qdp)} ahead of you at this price; ${o.queue.level_orders} order(s), ${fmtQty(o.queue.level_qty, qdp)} resting in total`}
                          >
                            queue #{o.queue.orders_ahead + 1} · {fmtQty(o.queue.qty_ahead, qdp)} ahead
                          </span>
                        )}
                      </div>
                      <div className="flex gap-1">
                        <button
                          title="Modify"
                          className="text-mute hover:text-white p-1 rounded hover:bg-line cursor-pointer"
                          onClick={() => {
                            setEditing(o.order_id);
                            setEditPx(o.price.toFixed(dp));
                            setEditQty(String(o.leaves_qty));
                          }}
                        >
                          <Edit2 className="w-3 h-3" />
                        </button>
                        <button title="Cancel" className="text-mute hover:text-sell p-1 rounded hover:bg-line cursor-pointer" onClick={() => api.cancel(token, o.order_id).catch(() => {})}>
                          <X className="w-3 h-3" />
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })
          )
        ) : positions.length === 0 ? (
          <Empty>No positions — trade to open one</Empty>
        ) : (
          <>
            <div className="grid grid-cols-5 text-[12px] text-mute uppercase pb-1 border-b border-line">
              <span>Symbol</span>
              <span className="text-right">Qty</span>
              <span className="text-right">Avg</span>
              <span className="text-right">Mark</span>
              <span className="text-right">PnL</span>
            </div>
            {positions.map((p) => {
              const s = spec(p.symbol);
              const dp = s?.price_decimals ?? 5;
              const pnl = p.realized + p.unrealized;
              return (
                <div key={p.symbol} className="grid grid-cols-5 py-1 items-center">
                  <span className="text-white">{p.symbol}</span>
                  <span className={`text-right ${p.qty > 0 ? 'text-buy' : p.qty < 0 ? 'text-sell' : 'text-mute'}`}>{fmtQty(Math.abs(p.qty), s?.qty_decimals ?? 0)}{p.qty < 0 ? ' S' : p.qty > 0 ? ' L' : ''}</span>
                  <span className="text-right text-mute">{p.avg_px ? p.avg_px.toFixed(dp) : '—'}</span>
                  <span className="text-right text-mute">{p.mark.toFixed(dp)}</span>
                  <span className={`text-right ${pnl >= 0 ? 'text-buy' : 'text-sell'}`} title={`realized ${p.realized.toFixed(2)} · unrealized ${p.unrealized.toFixed(2)} (${p.symbol.slice(3)})`}>
                    {pnl >= 0 ? '+' : ''}
                    {pnl.toFixed(2)}
                  </span>
                </div>
              );
            })}
          </>
        )}
      </div>
    </Panel>
  );
}
