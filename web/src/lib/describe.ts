import type { ExecReport } from './types';
import { fmtQty } from './useExchange';

/** Engine reject and cancel codes, in words a trader would use. */
const REASONS: Record<string, string> = {
  FokNotFillable: 'Fill-or-kill: there wasn’t enough at your price to fill the whole order, so nothing traded',
  PostOnlyWouldCross: 'Post-only: at this price the order would trade straight away, so it wasn’t placed',
  InvalidQty: 'That size isn’t valid for this symbol',
  InvalidPrice: 'That price isn’t valid for this symbol',
  DuplicateOrderId: 'Duplicate order',
  UnknownOrder: 'That order is no longer open',
  NotOwner: 'That order belongs to another account',
  'unfilled remainder (IOC/FOK/market)': 'the rest couldn’t be filled at your price and was cancelled',
  'self-trade prevention': 'cancelled so you don’t trade with your own order',
};

export const plainReason = (text: string | null) => (text ? (REASONS[text] ?? text) : '');

export type Tone = 'fill' | 'info' | 'warn';

/** One execution report as a sentence, e.g. "Bought 100K EURUSD at 1.12122". */
export function describe(r: ExecReport, dp: number, qdp: number): { tone: Tone; text: string } {
  const verb = r.side === 'BUY' ? 'Bought' : 'Sold';
  const qty = (v: number) => fmtQty(v, qdp);
  const px = (v: number) => v.toFixed(dp);
  switch (r.exec_type) {
    case 'FILL':
    case 'PARTIAL_FILL': {
      const rest = r.leaves_qty > 0 ? ` · ${qty(r.leaves_qty)} still open` : '';
      return { tone: 'fill', text: `${verb} ${qty(r.last_qty)} ${r.symbol} at ${px(r.last_px)}${rest}` };
    }
    case 'NEW':
      return r.price > 0
        ? { tone: 'info', text: `${r.side === 'BUY' ? 'Buy' : 'Sell'} ${qty(r.leaves_qty)} ${r.symbol} at ${px(r.price)} placed` }
        : { tone: 'info', text: `Market ${r.side === 'BUY' ? 'buy' : 'sell'} ${qty(r.leaves_qty)} ${r.symbol} sent` };
    case 'REPLACED':
      return { tone: 'info', text: `Order #${r.order_id} changed to ${qty(r.leaves_qty)} at ${px(r.price)}` };
    case 'CANCELED': {
      const why = plainReason(r.text);
      if (!why) return { tone: 'info', text: `Order #${r.order_id} cancelled` };
      if (r.cum_qty > 0) return { tone: 'warn', text: `${verb} ${qty(r.cum_qty)} ${r.symbol}; ${why}` };
      return { tone: 'warn', text: `Nothing filled in ${r.symbol}: ${why}` };
    }
    case 'REJECTED':
      return { tone: 'warn', text: plainReason(r.text) || 'Order rejected' };
  }
}
