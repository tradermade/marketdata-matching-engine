import { useEffect, useState } from 'react';
import { api } from '../lib/api';
import type { Estimate, Instrument, OrderKind, Side } from '../lib/types';
import { fmtQty } from '../lib/useExchange';

/** Coverage = visible opposite-side size / order size. */
function coverageClass(c: number) {
  if (c >= 3) return 'text-buy border-buy/40 bg-buy/10';
  if (c >= 1.5) return 'text-amber-400 border-amber-400/40 bg-amber-400/10';
  return 'text-sell border-sell/40 bg-sell/10';
}

/**
 * Pre-trade cost: asks the engine to walk the live book for this size on both
 * sides, without trading. Refreshes as the size changes and once a second.
 */
export function CostPreview({
  inst,
  token,
  qty,
  price,
  kind,
}: {
  inst: Instrument;
  token: string;
  qty: number;
  price: number;
  kind: OrderKind;
}) {
  const [est, setEst] = useState<Partial<Record<Side, Estimate>>>({});

  useEffect(() => {
    if (!(qty > 0)) {
      setEst({});
      return;
    }
    let live = true;
    const limit = kind === 'LIMIT' && price > 0 ? price : undefined;
    const load = () =>
      Promise.all(
        (['BUY', 'SELL'] as const).map((side) =>
          api.estimate(token, inst.symbol, side, qty, limit).then((e) => [side, e] as const),
        ),
      )
        .then((r) => live && setEst(Object.fromEntries(r)))
        .catch(() => {});
    const debounce = setTimeout(load, 150);
    const timer = setInterval(load, 1000);
    return () => {
      live = false;
      clearTimeout(debounce);
      clearInterval(timer);
    };
  }, [token, inst.symbol, qty, price, kind]);

  const quoteCcy = inst.symbol.slice(3);
  const row = (side: Side) => {
    const e = est[side];
    if (!e) return null;
    const color = side === 'BUY' ? 'text-buy' : 'text-sell';
    if (!e.avg_px) {
      return (
        <div key={side} className="flex justify-between">
          <span className={color}>{side}</span>
          {kind === 'LIMIT' ? (
            <span className="text-mute">no immediate fill · would rest on the book at your price</span>
          ) : (
            <span className="text-sell">no liquidity on this side</span>
          )}
        </div>
      );
    }
    return (
      <div key={side} className="grid grid-cols-[2.2rem_1fr_auto] gap-2 items-center">
        <span className={color}>{side}</span>
        <span className="text-mute truncate" title={`best ${e.best_px} · worst ${e.worst_px} · ${e.levels} level(s)`}>
          avg <span className="text-white">{e.avg_px.toFixed(inst.price_decimals + 1)}</span>
          {' · '}
          <span className={(e.cost_bps ?? 0) > 1 ? 'text-amber-400' : 'text-white'}>{(e.cost_bps ?? 0).toFixed(2)} bps</span>
          {' · '}
          {(e.cost_quote ?? 0).toFixed(2)} {quoteCcy}
          {' · '}
          {e.levels} lvl
          {e.fill_ratio < 1 && (
            <span className={kind === 'LIMIT' ? 'text-amber-400' : 'text-sell'}>
              {' · '}
              {fmtQty(e.filled_qty, inst.qty_decimals)} fills now{kind === 'LIMIT' ? ', rest would rest' : ', rest cancelled'}
            </span>
          )}
        </span>
        <span className={`px-1.5 rounded-full border ${coverageClass(e.coverage)}`} title="Visible opposite-side size ÷ your size. ≥3× comfortable, <1.5× you'll walk deep into the book.">
          {e.coverage >= 100 ? '99+' : e.coverage.toFixed(1)}×
        </span>
      </div>
    );
  };

  if (!est.BUY && !est.SELL) return null;
  return (
    <div className="mt-3 pt-2 border-t border-line font-mono text-[12px] space-y-1">
      <div className="text-mute uppercase tracking-wider text-[11px]">Pre-trade cost vs mid (at live prices)</div>
      {row('BUY')}
      {row('SELL')}
    </div>
  );
}
