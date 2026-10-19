import type { Account, Instrument } from '../lib/types';
import { fmtQty, fmtTime } from '../lib/useExchange';
import { Empty } from './Panel';

const bps = (v: number | null | undefined) => (v == null ? '…' : `${v >= 0 ? '+' : ''}${v.toFixed(2)}`);
const tone = (v: number | null | undefined, goodIfPositive: boolean) =>
  v == null ? 'text-mute' : (v >= 0) === goodIfPositive ? 'text-buy' : 'text-sell';

/**
 * Transaction-cost analysis per fill. Slippage is measured against the
 * TraderMade mid when the order arrived (positive = cost). Markouts are the
 * mid 1s / 5s / 60s after the fill (positive = price moved your way).
 */
export function ExecutionQuality({ account, instruments }: { account: Account | null; instruments: Instrument[] }) {
  const fills = account?.fills ?? [];
  const t = account?.tca;
  const spec = (s: string) => instruments.find((i) => i.symbol === s);
  if (!t || fills.length === 0) return <Empty>No fills yet — trade to see slippage and markouts</Empty>;

  const card = (label: string, v: number | null, goodIfPositive: boolean, hint: string) => (
    <div className="rounded-lg px-2.5 py-1.5 bg-panel2/70 ring-1 ring-white/5" title={hint}>
      <div className="text-[11px] text-mute uppercase">{label}</div>
      <div className={`text-sm ${tone(v, goodIfPositive)}`}>{bps(v)} <span className="text-[11px] text-mute">bps</span></div>
    </div>
  );

  return (
    <div className="flex flex-col gap-2 min-h-0">
      <div className="grid grid-cols-4 gap-2 font-mono">
        {card('Slippage', t.slippage_bps, false, 'Notional-weighted cost vs arrival mid. Negative = you earned the spread (passive fills).')}
        {card('Markout 1s', t.markout_1s_bps, true, 'Mid move 1s after your fills. Persistently negative = adverse selection.')}
        {card('Markout 5s', t.markout_5s_bps, true, 'Mid move 5s after your fills.')}
        {card('Markout 60s', t.markout_60s_bps, true, 'Mid move 60s after your fills.')}
      </div>
      <div className="grid grid-cols-[3.6rem_2.4rem_1fr_1fr_3.2rem_repeat(3,2.8rem)] text-[11px] text-mute uppercase border-b border-line pb-1 gap-1">
        <span>Time</span>
        <span>Liq</span>
        <span>Fill</span>
        <span className="text-right">Arrival mid</span>
        <span className="text-right">Slip</span>
        <span className="text-right">1s</span>
        <span className="text-right">5s</span>
        <span className="text-right">60s</span>
      </div>
      <div className="overflow-y-auto space-y-0.5 min-h-0">
        {fills.map((f) => {
          const s = spec(f.symbol);
          const dp = s?.price_decimals ?? 5;
          return (
            <div key={f.fill_id} className="grid grid-cols-[3.6rem_2.4rem_1fr_1fr_3.2rem_repeat(3,2.8rem)] gap-1 text-[12px] items-center">
              <span className="text-mute">{fmtTime(f.ts).slice(0, 8)}</span>
              <span className={f.liquidity === 'MAKER' ? 'text-blue-400' : 'text-mute'} title={f.liquidity === 'MAKER' ? 'Your resting order was hit' : 'You crossed the spread'}>
                {f.liquidity === 'MAKER' ? 'MKR' : 'TKR'}
              </span>
              <span className={`truncate ${f.side === 'BUY' ? 'text-buy' : 'text-sell'}`}>
                {f.symbol} {f.side === 'BUY' ? 'B' : 'S'} {fmtQty(f.qty, s?.qty_decimals ?? 0)} @ {f.px.toFixed(dp)}
              </span>
              <span className="text-right text-mute">{f.arrival_mid.toFixed(dp + 1)}</span>
              <span className={`text-right ${tone(f.slippage_bps, false)}`}>{bps(f.slippage_bps)}</span>
              {f.markouts_bps.map((m, i) => (
                <span key={i} className={`text-right ${tone(m, true)}`}>
                  {bps(m)}
                </span>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
