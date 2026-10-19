import type { Estimate, ExecReport, OrderKind, Session, Side, Tif } from './types';

const TOKEN_KEY = 'tm_session';

export function loadSession(): Session | null {
  try {
    const raw = localStorage.getItem(TOKEN_KEY);
    return raw ? (JSON.parse(raw) as Session) : null;
  } catch {
    return null;
  }
}

export function saveSession(s: Session | null) {
  try {
    if (s) localStorage.setItem(TOKEN_KEY, JSON.stringify(s));
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* storage unavailable: session lives for this tab only */
  }
}

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

async function call<T>(method: string, path: string, token: string | null, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: {
      'content-type': 'application/json',
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new ApiError(res.status, data.error ?? res.statusText);
  return data as T;
}

export interface OrderAck {
  order_id: number;
  engine_ns: number;
  roundtrip_us: number;
  reports: ExecReport[];
}

export const api = {
  createSession: (name: string) => call<Session>('POST', '/api/session', null, { name }),
  me: (token: string) => call<unknown>('GET', '/api/me', token),
  submit: (
    token: string,
    o: { symbol: string; side: Side; type: OrderKind; price?: number; qty: number; tif: Tif; post_only: boolean },
  ) => call<OrderAck>('POST', '/api/orders', token, o),
  cancel: (token: string, id: number) => call<OrderAck>('DELETE', `/api/orders/${id}`, token),
  estimate: (token: string, symbol: string, side: Side, qty: number, price?: number) =>
    call<Estimate>(
      'GET',
      `/api/estimate?symbol=${encodeURIComponent(symbol)}&side=${side}&qty=${qty}${price ? `&price=${price}` : ''}`,
      token,
    ),
  modify: (token: string, id: number, price: number, qty: number) =>
    call<OrderAck>('PUT', `/api/orders/${id}`, token, { price, qty }),
  /** status: "listed" (already there), "requested" or "pending" (waiting for its first quote) */
  addSymbol: (token: string, symbol: string) =>
    call<{ symbol: string; status: string }>('POST', '/api/symbols', token, { symbol }),
  /** Only symbols added from the search; refused while you have open orders in it. */
  removeSymbol: (token: string, symbol: string) =>
    call<{ symbol: string; status: string }>('DELETE', `/api/symbols/${encodeURIComponent(symbol)}`, token),
};
