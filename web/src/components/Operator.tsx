import { Activity, Cpu, Radio, Gauge } from 'lucide-react';
import type { Account, FeedStatus, Instrument, Metrics, Quote } from '../lib/types';
import { fmtQty, fmtTime } from '../lib/useExchange';
import { Panel } from './Panel';
import { ExecReports } from './Tape';

const OPS = ['BUY', 'SELL', 'CANCEL', 'MODIFY', 'LP_SYNC'];

function heatColor(us: number) {
  if (us <= 0) return 'bg-[#1A2438]';
  if (us < 2) return 'bg-buy/25';
  if (us < 10) return 'bg-buy/60';
  if (us < 50) return 'bg-amber-400/80';
  return 'bg-sell';
}

const us = (v: number) => (v >= 1000 ? `${(v / 1000).toFixed(2)}ms` : `${v.toFixed(2)}µs`);

export function Operator({
  metrics,
  feed,
  quotes,
  instruments,
  logs,
  account,
}: {
  account: Account | null;
  metrics: Metrics | null;
  feed: FeedStatus | null;
  quotes: Record<string, Quote>;
  instruments: Instrument[];
  logs: string[];
}) {
  const live = feed?.mode === 'tradermade' && feed.connected;
  return (
    <section className="lg:col-span-12 flex flex-col gap-4 min-h-0 lg:overflow-y-auto lg:pr-1">
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 flex-shrink-0">
        <Panel title="TraderMade Feed" icon={<Radio className="w-4 h-4 text-amber-400" />}>
          <div className="space-y-2 text-[12px] font-mono">
            <Row k="Source" v={<span className={live ? 'text-buy' : 'text-amber-400'}>{feed?.mode === 'tradermade' ? 'WebSocket V2 /feedAdv' : feed?.mode ?? '—'}</span>} />
            <Row k="Connected" v={<span className={feed?.connected ? 'text-buy' : 'text-sell'}>{feed?.connected ? '● YES' : '○ NO'}</span>} />
            <Row k="Ticks / sec" v={metrics?.ticks_per_sec ?? 0} />
            <Row k="Subscribed" v={feed?.accepted.join(' ') || '—'} />
            {Object.entries(feed?.denied ?? {}).map(([sym, reason]) => (
              <Row key={sym} k={`Denied ${sym}`} v={<span className={reason === 'toggle_cooldown' ? 'text-amber-400' : 'text-sell'}>{reason === 'toggle_cooldown' ? 'cooldown · retrying' : reason}</span>} />
            ))}
            {!!feed?.invalid.length && (
              <Row k="Invalid symbol" v={<span className="text-sell" title="CFDs need a USD suffix (UK100USD, SPX500USD); stocks use .US (AAPL.US)">{feed.invalid.join(' ')}</span>} />
            )}
            {feed?.message && <div className="text-amber-400/80 pt-1 border-t border-line break-all">{feed.message}</div>}
          </div>
        </Panel>

        <Panel title="Engine" icon={<Cpu className="w-4 h-4 text-amber-400" />}>
          <div className="space-y-2 text-[12px] font-mono">
            <Row k="Model" v="single thread · lock-free hot path" />
            <Row k="Book" v="BTreeMap levels + slab FIFO" />
            <Row k="Prices" v="i64 fixed-point ticks" />
            <Row k="Commands / sec" v={metrics?.ops_per_sec ?? 0} />
            <Row k="Resting orders" v={metrics?.resting_orders ?? 0} />
            <Row k="Sequence" v={metrics?.seq ?? 0} />
          </div>
        </Panel>

        <Panel title="Status" icon={<Gauge className="w-4 h-4 text-amber-400" />}>
          <div className="space-y-2 text-[12px] font-mono">
            <Row k="Accounts" v={metrics?.accounts ?? 0} />
            <Row k="Uptime" v={`${Math.floor((metrics?.uptime_s ?? 0) / 60)}m ${(metrics?.uptime_s ?? 0) % 60}s`} />
            <div className="text-mute pt-1 border-t border-line leading-relaxed">
              All figures are measured with <span className="text-white">std::time::Instant</span> inside the engine thread — nothing simulated.
            </div>
          </div>
        </Panel>
      </div>

      <div className="grid grid-cols-1 xl:grid-cols-2 gap-4 shrink-0">
        <div className="flex flex-col gap-4 min-h-0">
          <Panel title="Matching Latency (measured)" icon={<Activity className="w-4 h-4 text-buy" />}>
            <table className="w-full text-[12px] font-mono">
              <thead className="text-mute uppercase">
                <tr className="border-b border-line">
                  <th className="text-left py-1">Op</th>
                  <th className="text-right">Count</th>
                  <th className="text-right">p50</th>
                  <th className="text-right">p99</th>
                  <th className="text-right">p99.9</th>
                  <th className="text-right">Max</th>
                </tr>
              </thead>
              <tbody>
                {OPS.map((op) => {
                  const s = metrics?.ops[op];
                  return (
                    <tr key={op} className="border-b border-line/40">
                      <td className="py-1 text-mute">{op}</td>
                      <td className="text-right text-white">{s?.count ?? 0}</td>
                      <td className="text-right text-buy">{s?.count ? us(s.p50_us) : '—'}</td>
                      <td className="text-right text-white">{s?.count ? us(s.p99_us) : '—'}</td>
                      <td className="text-right text-white">{s?.count ? us(s.p999_us) : '—'}</td>
                      <td className="text-right text-amber-400">{s?.count ? us(s.max_us) : '—'}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            <div className="mt-4 flex flex-col gap-2">
              {OPS.map((op) => {
                const heat = metrics?.ops[op]?.heat ?? [];
                const cells = [...Array(Math.max(0, 24 - heat.length)).fill(0), ...heat];
                return (
                  <div key={op} className="flex items-center gap-2">
                    <span className="text-[11px] text-mute uppercase w-14">{op}</span>
                    <div className="flex-1 flex gap-[2px] h-5">
                      {cells.map((v, i) => (
                        <div key={i} className={`flex-1 rounded-sm ${heatColor(v)}`} title={v > 0 ? us(v) : 'no data'} />
                      ))}
                    </div>
                  </div>
                );
              })}
              <div className="flex gap-3 text-[11px] text-mute justify-end">
                <span><i className="inline-block w-2 h-2 bg-buy/25 mr-1" />&lt;2µs</span>
                <span><i className="inline-block w-2 h-2 bg-buy/60 mr-1" />&lt;10µs</span>
                <span><i className="inline-block w-2 h-2 bg-amber-400/80 mr-1" />&lt;50µs</span>
                <span><i className="inline-block w-2 h-2 bg-sell mr-1" />≥50µs</span>
              </div>
            </div>
          </Panel>

          <Panel title="Live TraderMade quotes" className="flex-1">
            <div className="overflow-x-auto">
              <table className="w-full text-[12px] font-mono">
                <thead className="text-mute uppercase">
                  <tr className="border-b border-line">
                    <th className="text-left py-1">Symbol</th>
                    <th className="text-right">Bid size</th>
                    <th className="text-right">Bid</th>
                    <th className="text-right">Ask</th>
                    <th className="text-right">Ask size</th>
                    <th className="text-right">Market</th>
                    <th className="text-right">Provider ts</th>
                  </tr>
                </thead>
                <tbody>
                  {instruments.map((i) => {
                    const q = quotes[i.symbol];
                    return (
                      <tr key={i.symbol} className="border-b border-line/40">
                        <td className="py-1 text-white">{i.symbol}</td>
                        <td className="text-right text-mute">{q?.bid_vol != null ? fmtQty(q.bid_vol, i.qty_decimals) : '—'}</td>
                        <td className="text-right text-sell">{q ? q.bid.toFixed(i.price_decimals) : '—'}</td>
                        <td className="text-right text-buy">{q ? q.ask.toFixed(i.price_decimals) : '—'}</td>
                        <td className="text-right text-mute">{q?.ask_vol != null ? fmtQty(q.ask_vol, i.qty_decimals) : '—'}</td>
                        <td className="text-right">{marketState(i.symbol, feed, q)}</td>
                        <td className="text-right text-mute" title={q?.ts ? new Date(q.ts).toUTCString() : ''}>{q?.ts ? fmtTime(q.ts) : '—'}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </Panel>
        </div>

        <div className="flex flex-col gap-4 min-h-0">
          <Panel title="Sequencer & Matcher Logs" className="h-[420px]">
            <div className="flex-1 overflow-y-auto font-mono text-[13px] space-y-0.5 pr-1 min-h-0">
              {logs.map((l, i) => {
                let c = 'text-mute';
                if (l.includes('MATCH')) c = 'text-[#E3E9F2]';
                if (l.includes('INIT') || l.includes('FEED') || l.includes('SESSION')) c = 'text-amber-400';
                if (l.includes('CANCEL') || l.includes('REJECT')) c = 'text-sell';
                if (l.includes('MODIFY')) c = 'text-blue-400';
                return (
                  <div key={i} className={`py-0.5 hover:bg-panel2 ${c}`}>
                    {l}
                  </div>
                );
              })}
            </div>
          </Panel>
          <div className="h-[360px] flex flex-col">
            <ExecReports account={account} instruments={instruments} />
          </div>
        </div>
      </div>
    </section>
  );
}

/** Accepted is not the same as live: outside trading hours TraderMade sends
 * one LAST_QUOTE on subscribe and then nothing until the market reopens. */
function marketState(symbol: string, feed: FeedStatus | null, q?: Quote) {
  if (feed?.invalid.includes(symbol)) return <span className="text-sell">INVALID</span>;
  if (feed?.denied[symbol] && feed.denied[symbol] !== 'toggle_cooldown') return <span className="text-sell">DENIED</span>;
  if (!q?.ts) return <span className="text-mute">waiting</span>;
  return Date.now() - q.ts > 120_000 ? <span className="text-amber-400">CLOSED</span> : <span className="text-buy">LIVE</span>;
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-3">
      <span className="text-mute">{k}</span>
      <span className="text-white text-right truncate">{v}</span>
    </div>
  );
}
