import { useEffect, useState } from 'react';
import { api } from '../lib/api';
import type { FeedStatus } from '../lib/types';
import { searchSymbols, symbolInfo } from '../lib/symbols';

/**
 * Search TraderMade's symbol list (by code or name), or type any symbol and
 * press Enter, to add it. TraderMade's `sub_ack` decides: accepted symbols are
 * listed straight away, even if their market is closed and no price comes;
 * invalid or denied ones show the reason.
 */
export function SymbolSearch({
  token,
  available,
  listed,
  feed,
  onAdded,
}: {
  token: string;
  available: string[];
  listed: string[];
  feed: FeedStatus | null;
  onAdded: (symbol: string) => void;
}) {
  const [q, setQ] = useState('');
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const term = q.trim().toUpperCase();
  const matches = searchSymbols(available, term, listed);

  // Select the symbol once it's listed; report TraderMade's reason if it isn't.
  useEffect(() => {
    if (!pending) return;
    if (listed.includes(pending)) {
      onAdded(pending);
      setPending(null);
      return;
    }
    const denied = feed?.denied[pending];
    if (feed?.invalid.includes(pending)) {
      setError(`${pending}: not a TraderMade symbol in this form`);
      setPending(null);
    } else if (denied && denied !== 'toggle_cooldown') {
      setError(`${pending}: ${denied.replace(/_/g, ' ')}`);
      setPending(null);
    }
  }, [pending, listed, feed, onAdded]);

  // No sub_ack after a while (e.g. the feed is reconnecting): say so.
  useEffect(() => {
    if (!pending) return;
    const t = setTimeout(() => {
      setError(`${pending}: no reply from TraderMade yet; try again shortly`);
      setPending(null);
    }, 15_000);
    return () => clearTimeout(t);
  }, [pending]);

  const add = async (symbol: string) => {
    const s = symbol.trim().toUpperCase();
    if (!s) return;
    setQ('');
    setOpen(false);
    setError(null);
    if (listed.includes(s)) {
      onAdded(s);
      return;
    }
    try {
      const r = await api.addSymbol(token, s);
      if (r.status === 'listed') onAdded(r.symbol);
      else setPending(r.symbol);
    } catch (e: any) {
      setError(e.message);
    }
  };

  return (
    <div className="relative flex-shrink-0 mt-1">
      <input
        value={q}
        onChange={(e) => {
          setQ(e.target.value);
          setOpen(true);
          setError(null);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && term) add(matches[0] ?? term);
          if (e.key === 'Escape') {
            setQ('');
            setOpen(false);
          }
        }}
        placeholder={pending ? `Adding ${pending}…` : '+ Add symbol'}
        title="Search by code or name. Stocks: AAPL.US · CFDs and indices: NAS100USD · FX: GBPJPY"
        className="field w-44 h-full px-3.5 py-2 text-[13px] font-mono text-white placeholder:text-mute"
      />
      {open && matches.length > 0 && (
        <div className="card absolute right-0 top-full mt-1.5 z-20 w-72 p-1 max-h-80 overflow-auto">
          {matches.map((s) => {
            const { kind, name } = symbolInfo(s);
            return (
              <button
                key={s}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => add(s)}
                className="flex w-full items-center justify-between gap-3 text-left px-3 py-1.5 rounded-lg text-[13px] text-mute hover:text-white hover:bg-panel2 cursor-pointer"
              >
                <span className="font-mono text-white">{s}</span>
                <span className="truncate text-[12px]">{name ? `${name} · ${kind}` : kind}</span>
              </button>
            );
          })}
        </div>
      )}
      {error && !open && (
        <div className="card absolute right-0 top-full mt-1.5 z-20 w-60 text-[12px] font-mono text-sell px-3 py-2">
          {error}
        </div>
      )}
    </div>
  );
}
