import { useState } from 'react';
import { ArrowRight, BarChart3, Layers, Radio } from 'lucide-react';
import { api } from '../lib/api';
import type { FeedStatus, Session } from '../lib/types';

const POINTS = [
  { icon: Radio, title: 'Live TraderMade prices', text: 'FX, metals and crypto, streamed over WebSocket' },
  { icon: Layers, title: 'Real order types', text: 'Market, limit, IOC, fill-or-kill and post-only' },
  { icon: BarChart3, title: 'Execution quality', text: 'Slippage and markouts on every fill' },
];

export function Login({ onSession, feed }: { onSession: (s: Session) => void; feed: FeedStatus | null }) {
  const [name, setName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onSession(await api.createSession(name));
    } catch (err: any) {
      setError(err.message ?? 'Could not reach the exchange');
    } finally {
      setBusy(false);
    }
  };

  const status = !feed
    ? { dot: 'bg-mute', text: 'Connecting to the engine…' }
    : feed.mode === 'tradermade'
      ? feed.connected
        ? { dot: 'bg-buy', text: 'Live TraderMade prices' }
        : { dot: 'bg-amber-400', text: 'Reconnecting to TraderMade…' }
      : { dot: 'bg-amber-400', text: 'Simulated prices (no TraderMade key set)' };

  return (
    <div className="w-full min-h-screen bg-bg text-[#E3E9F2] flex items-center justify-center p-4 sm:p-8">
      <div className="w-full max-w-4xl grid md:grid-cols-[1.15fr_1fr] gap-10 md:gap-14 items-center">
        <div>
          <div className="flex items-center gap-2.5 text-sm font-semibold text-white">
            <span className="grid place-items-center w-8 h-8 rounded-lg bg-amber-400 text-bg font-bold text-[15px]">TM</span>
            Match Core
          </div>
          <h1 className="mt-7 text-3xl sm:text-4xl font-bold leading-[1.15] text-white">
            Paper-trade on a real matching engine.
          </h1>
          <p className="mt-4 text-sm text-mute leading-relaxed max-w-md">
            Orders are matched by price and time in a Rust engine, against a market maker that quotes TraderMade’s live bid and offer.
          </p>
          <ul className="mt-8 space-y-4">
            {POINTS.map(({ icon: Icon, title, text }) => (
              <li key={title} className="flex gap-3">
                <span className="grid place-items-center w-9 h-9 shrink-0 rounded-full bg-buy/10 text-buy">
                  <Icon size={16} />
                </span>
                <span>
                  <span className="block text-[15px] font-medium text-white">{title}</span>
                  <span className="block text-[13px] text-mute">{text}</span>
                </span>
              </li>
            ))}
          </ul>
        </div>

        <form onSubmit={submit} className="card p-6 sm:p-7">
          <h2 className="text-lg font-semibold text-white">Open a demo account</h2>
          <p className="mt-1 text-[13px] text-mute">No password and no real money. Your session stays in this browser.</p>

          <label className="block mt-6 mb-1.5 text-[13px] font-medium text-mute" htmlFor="trader-name">
            Your name
          </label>
          <input
            id="trader-name"
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. alex"
            maxLength={24}
            className="field w-full px-3.5 py-2.5 text-white text-sm"
          />

          {error && <div className="mt-3 text-[13px] text-sell bg-sell/10 px-3 py-2 rounded-lg">{error}</div>}

          <button
            type="submit"
            disabled={busy}
            className="mt-5 w-full flex items-center justify-center gap-2 bg-amber-400 text-bg hover:brightness-110 active:brightness-95 transition-all py-2.5 rounded-lg font-semibold text-sm cursor-pointer disabled:opacity-50"
          >
            {busy ? 'Opening…' : 'Start trading'}
            {!busy && <ArrowRight size={16} />}
          </button>

          <div className="mt-6 pt-4 border-t border-white/5 flex items-center gap-2 text-[13px] text-mute">
            <span className={`w-2 h-2 rounded-full ${status.dot}`} />
            {status.text}
          </div>
        </form>
      </div>
    </div>
  );
}
