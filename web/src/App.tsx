import { useCallback, useRef, useState } from 'react';
import { CheckCircle2, LogOut, X } from 'lucide-react';
import { api, loadSession, saveSession } from './lib/api';
import type { ExecReport, Session } from './lib/types';
import { describe, type Tone } from './lib/describe';
import { fmtPx, useExchange } from './lib/useExchange';
import { Login } from './components/Login';
import { PriceChart } from './components/PriceChart';
import { OrderEntry } from './components/OrderEntry';
import { Blotter } from './components/Blotter';
import { Activity, Trades } from './components/Tape';
import { Operator } from './components/Operator';
import { SymbolSearch } from './components/SymbolSearch';

interface Toast {
  id: number;
  tone: Tone;
  text: string;
}

export default function App() {
  const [session, setSession] = useState<Session | null>(loadSession);
  const logout = useCallback(() => {
    saveSession(null);
    setSession(null);
  }, []);

  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextToast = useRef(1);
  const toast = useCallback((tone: Tone, text: string) => {
    const id = nextToast.current++;
    setToasts((t) => [...t.slice(-3), { id, tone, text }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 4500);
  }, []);

  // Instruments live in a ref so the fill pop-up can format prices without
  // re-subscribing the WebSocket.
  const instRef = useRef<{ symbol: string; price_decimals: number; qty_decimals: number }[]>([]);
  const onExec = useCallback(
    (r: ExecReport) => {
      if (r.exec_type !== 'FILL' && r.exec_type !== 'PARTIAL_FILL') return;
      const i = instRef.current.find((x) => x.symbol === r.symbol);
      toast('fill', describe(r, i?.price_decimals ?? 5, i?.qty_decimals ?? 0).text);
    },
    [toast],
  );
  const ex = useExchange(session?.token ?? null, logout, onExec);
  instRef.current = ex.instruments;

  const [symbol, setSymbol] = useState('EURUSD');
  const [tab, setTab] = useState<'TRADER' | 'OPERATOR'>('TRADER');

  const removeSymbol = async (s: string) => {
    if (!session) return;
    try {
      await api.removeSymbol(session.token, s);
      if (s === symbol) setSymbol('EURUSD');
    } catch (e: any) {
      toast('warn', e.message);
    }
  };

  const inst = ex.instruments.find((i) => i.symbol === symbol) ?? ex.instruments[0];
  const quote = inst ? ex.quotes[inst.symbol] : undefined;

  if (!session) {
    return (
      <Login
        feed={ex.feed}
        onSession={(s) => {
          saveSession(s);
          setSession(s);
        }}
      />
    );
  }

  if (!inst) {
    return (
      <div className="w-full min-h-screen bg-bg flex items-center justify-center text-mute text-sm">
        {ex.connected ? 'Loading instruments…' : 'Connecting to the engine…'}
      </div>
    );
  }

  const live = ex.feed?.mode === 'tradermade' && ex.feed.connected;
  const m = ex.metrics;
  const p99 = Math.max(m?.ops.BUY?.p99_us ?? 0, m?.ops.SELL?.p99_us ?? 0);
  const matched = (m?.ops.BUY?.count ?? 0) + (m?.ops.SELL?.count ?? 0) > 0;

  return (
    <div className="w-full min-h-screen lg:h-screen bg-bg text-[#E3E9F2] font-sans flex flex-col p-4 sm:p-5 lg:overflow-hidden">
      <header className="flex flex-col xl:flex-row items-start xl:items-center justify-between border-b border-line pb-3 mb-3 gap-3">
        <div className="flex items-center gap-3">
          <span className="grid place-items-center w-9 h-9 rounded-lg bg-amber-400 text-bg font-bold text-sm">TM</span>
          <div>
            <h1 className="text-lg font-bold tracking-tight text-white flex items-center gap-2">
              Match Core
              <span className={`text-[13px] font-medium px-2.5 py-0.5 rounded-full ${live ? 'text-buy bg-buy/15' : 'text-amber-400 bg-amber-400/15'}`}>
                {live ? 'TraderMade live' : ex.feed?.mode === 'synthetic' ? 'Simulated prices' : 'Connecting…'}
              </span>
              {!ex.connected && <span className="text-[13px] font-medium px-2.5 py-0.5 rounded-full text-sell bg-sell/15">Disconnected</span>}
            </h1>
          </div>
        </div>

        <div className="flex gap-1 p-1 rounded-full bg-panel ring-1 ring-white/5">
          {(['TRADER', 'OPERATOR'] as const).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`px-4 py-1.5 rounded-full text-[13px] font-medium cursor-pointer transition-colors ${
                tab === t ? `bg-panel2 shadow-sm ${t === 'TRADER' ? 'text-buy' : 'text-amber-400'}` : 'text-mute hover:text-white'
              }`}
            >
              {t === 'TRADER' ? 'Dashboard' : 'Monitoring'}
            </button>
          ))}
        </div>

        <div className="card flex gap-4 sm:gap-6 px-4 py-2 items-center">
          <Stat label="p99 match" value={matched ? `${p99.toFixed(2)}µs` : '—'} className="text-buy" />
          <Stat label="TM ticks/s" value={m ? String(m.ticks_per_sec) : '—'} />
          <div className="border-l border-line pl-4 flex items-center gap-3">
            <div className="text-[13px] text-right">
              <p className="text-white font-medium">{session.name}</p>
              <p className="text-mute text-[12px]">acct #{session.account_id}</p>
            </div>
            <button onClick={logout} className="text-mute hover:text-sell transition-colors cursor-pointer" title="End session">
              <LogOut size={16} />
            </button>
          </div>
        </div>
      </header>

      {/* symbol strip, with search outside the scrolling area so its list isn't clipped */}
      <div className="flex gap-2 mb-3 items-start">
        {/* padding keeps the selected chip's outline from being clipped by the scroll area */}
        <nav className="flex gap-2 overflow-x-auto px-1 pt-1 pb-1.5 flex-1 min-w-0">
          {ex.instruments.map((i) => {
            const q = ex.quotes[i.symbol];
            const active = i.symbol === inst.symbol;
            return (
              <div key={i.symbol} className="relative flex-shrink-0 group">
                <button
                  onClick={() => setSymbol(i.symbol)}
                  className={`text-left px-3.5 py-2 rounded-xl font-mono cursor-pointer transition-all ${i.added ? 'pr-7' : ''} ${
                    active ? 'bg-panel2 ring-1 ring-amber-400/70 shadow-md' : 'bg-panel ring-1 ring-white/5 hover:ring-white/15'
                  }`}
                >
                  <div className={`text-[13px] font-semibold flex items-center gap-1.5 ${active ? 'text-white' : 'text-mute'}`}>
                    {i.symbol}
                    {(i.awaiting_price || !q) && (
                      <span className="text-[10px] px-1.5 rounded-full bg-amber-400/15 text-amber-400" title="TraderMade accepted this symbol but hasn't sent a price yet; its market may be closed">
                        No price yet
                      </span>
                    )}
                    {isStale(q?.ts) && (
                      <span className="text-[10px] px-1.5 rounded-full bg-amber-400/15 text-amber-400" title="No TraderMade tick for over 2 minutes (market closed?)">
                        Stale
                      </span>
                    )}
                  </div>
                  <div className="text-[12px]">
                    <span className="text-sell">{fmtPx(q?.bid, i.price_decimals)}</span>
                    <span className="text-mute"> / </span>
                    <span className="text-buy">{fmtPx(q?.ask, i.price_decimals)}</span>
                  </div>
                </button>
                {i.added && (
                  <button
                    onClick={() => removeSymbol(i.symbol)}
                    title={`Remove ${i.symbol}`}
                    aria-label={`Remove ${i.symbol}`}
                    className="absolute top-1.5 right-1.5 grid place-items-center w-4 h-4 rounded-full text-mute hover:text-white hover:bg-white/10 cursor-pointer"
                  >
                    <X size={11} />
                  </button>
                )}
              </div>
            );
          })}
        </nav>
        <SymbolSearch
          token={session.token}
          available={ex.symbols}
          listed={ex.instruments.map((i) => i.symbol)}
          feed={ex.feed}
          onAdded={setSymbol}
        />
      </div>

      <main className="flex-1 grid grid-cols-1 lg:grid-cols-12 gap-4 min-h-0">
        {tab === 'TRADER' ? (
          <>
            {/* chart and your orders on the left, the ticket and what happened on the right */}
            <section className="lg:col-span-8 flex flex-col gap-4 min-h-0">
              <div className="flex-[3] min-h-[300px]">
                <PriceChart inst={inst} candles={ex.candles[inst.symbol] ?? []} quote={quote} />
              </div>
              <div className="flex-[2] min-h-[200px] flex flex-col">
                <Blotter account={ex.account} token={session.token} instruments={ex.instruments} />
              </div>
            </section>
            <section className="lg:col-span-4 flex flex-col gap-4 min-h-0">
              <OrderEntry inst={inst} quote={quote} token={session.token} />
              <div className="flex-1 min-h-[200px] flex flex-col">
                <Activity account={ex.account} instruments={ex.instruments} />
              </div>
              <div className="flex-1 min-h-[200px] flex flex-col">
                <Trades inst={inst} trades={ex.trades[inst.symbol] ?? []} account={ex.account} />
              </div>
            </section>
          </>
        ) : (
          <Operator
            metrics={ex.metrics}
            feed={ex.feed}
            quotes={ex.quotes}
            instruments={ex.instruments}
            logs={ex.logs}
            account={ex.account}
          />
        )}
      </main>

      <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 w-[min(22rem,calc(100vw-2rem))]" aria-live="polite">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`card flex items-start gap-2.5 px-3.5 py-2.5 text-[13px] ${t.tone === 'warn' ? 'text-sell' : 'text-white'}`}
          >
            {t.tone === 'fill' && <CheckCircle2 size={15} className="text-buy shrink-0 mt-px" />}
            <span className="flex-1">{t.text}</span>
            <button
              onClick={() => setToasts((all) => all.filter((x) => x.id !== t.id))}
              className="text-mute hover:text-white cursor-pointer"
              aria-label="Dismiss"
            >
              <X size={13} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

/** No provider tick for 2 minutes: FX weekends, holidays, or a feed problem. */
export function isStale(ts?: number) {
  return !!ts && Date.now() - ts > 120_000;
}

function Stat({ label, value, className = 'text-white' }: { label: string; value: string; className?: string }) {
  return (
    <div className="text-right">
      <p className="text-[12px] text-mute uppercase">{label}</p>
      <p className={`text-lg font-mono ${className}`}>{value}</p>
    </div>
  );
}
