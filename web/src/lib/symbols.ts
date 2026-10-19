/**
 * What a TraderMade symbol is, from its form:
 *   AAPL.US   US stock        SAPG.DE  German stock      SPY.ETF  ETF
 *   AAPLUSD   CFD (stocks, indices and commodities quoted in USD: NAS100USD, UKOILUSD)
 *   GBPJPY    FX              XAUUSD   metal             BTCUSD   crypto
 * AAPL.US and AAPLUSD stream the same Apple price; the stock form also carries sizes.
 */

/** CFD names, from TraderMade's /cfd_list (plus the few the stream adds). */
const NAMES: Record<string, string> = {
  AAPL: 'Apple', AMZN: 'Amazon', AZN: 'AstraZeneca', BA: 'Boeing', BABA: 'Alibaba', BAC: 'Bank of America',
  BIDU: 'Baidu', FB: 'Facebook', GE: 'General Electric', GLW: 'Corning', GOOGL: 'Alphabet', GS: 'Goldman Sachs',
  JNJ: 'Johnson & Johnson', LMT: 'Lockheed Martin', MA: 'Mastercard', META: 'Meta Platforms', MRK: 'Merck',
  MSFT: 'Microsoft', NFLX: 'Netflix', PFE: 'Pfizer', TSLA: 'Tesla', TWTR: 'Twitter', V: 'Visa',
  AUS200: 'ASX 200', CHN50: 'China A50', ESP35: 'IBEX 35', EU50: 'Euro Stoxx 50', FRA40: 'CAC 40', GER30: 'DAX',
  HKG33: 'Hang Seng', JPN225: 'Nikkei 225', NAS100: 'Nasdaq 100', SPX500: 'S&P 500', UK100: 'FTSE 100',
  USA30: 'Dow Jones 30', COPPER: 'Copper', NATGAS: 'Natural gas', OIL: 'WTI crude oil', UKOIL: 'Brent crude oil',
};

/** ISO 4217 codes (plus CNH and a few legacy codes): TraderMade lists many exotic FX pairs. */
const CURRENCIES = new Set(
  (
    'AED AFN ALL AMD ANG AOA ARS AUD AWG AZN BAM BBD BDT BGN BHD BIF BMD BND BOB BRL BSD BTN BWP BYN BYR BZD ' +
    'CAD CDF CHF CLP CNH CNY COP CRC CUC CUP CVE CZK DJF DKK DOP DZD EGP ERN ETB EUR FJD FKP GBP GEL GGP GHS GIP ' +
    'GMD GNF GTQ GYD HKD HNL HRK HTG HUF IDR ILS IMP INR IQD IRR ISK JEP JMD JOD JPY KES KGS KHR KMF KPW KRW KWD ' +
    'KYD KZT LAK LBP LKR LRD LSL LYD MAD MDL MGA MKD MMK MNT MOP MRO MRU MUR MVR MWK MXN MYR MZN NAD NGN NIO NOK ' +
    'NPR NZD OMR PAB PEN PGK PHP PKR PLN PYG QAR RON RSD RUB RWF SAR SBD SCR SDG SEK SGD SHP SLE SLL SOS SRD SSP ' +
    'STD STN SVC SYP SZL THB TJS TMT TND TOP TRY TTD TWD TZS UAH UGX USD UYU UZS VEF VES VND VUV WST XAF XCD XOF ' +
    'XPF YER ZAR ZMK ZMW ZWL'
  ).split(' '),
);
const METALS = new Set(['XAU', 'XAG', 'XPT', 'XPD']);
const MAJORS = new Set(['USD', 'EUR', 'GBP', 'JPY', 'CHF', 'AUD', 'NZD', 'CAD']);

/** The code without its postfix: AAPL.US → AAPL, NAS100USD → NAS100. */
function base(symbol: string) {
  const dot = symbol.indexOf('.');
  if (dot > 0) return symbol.slice(0, dot);
  if (symbol.endsWith('USD') && symbol.length > 6) return symbol.slice(0, -3);
  return symbol;
}

export function symbolInfo(symbol: string): { kind: string; name?: string } {
  const b = base(symbol);
  const name = NAMES[b];
  if (symbol.endsWith('.US')) return { kind: 'US stock', name };
  if (symbol.endsWith('.DE')) return { kind: 'German stock' };
  if (symbol.endsWith('.ETF') || symbol === 'GLD' || symbol === 'SLV') return { kind: 'ETF' };
  if (b !== symbol && name) return { kind: 'CFD', name };
  if (symbol.length === 6) {
    const [x, y] = [symbol.slice(0, 3), symbol.slice(3)];
    if (METALS.has(x)) return { kind: 'Metal' };
    if (MAJORS.has(x) && MAJORS.has(y)) return { kind: 'FX major' };
    if (CURRENCIES.has(x) && CURRENCIES.has(y)) return { kind: 'FX' };
  }
  return { kind: 'Crypto' };
}

/**
 * Best matches for a search term, by symbol or name: "AAPL" finds AAPL.US and
 * AAPLUSD, "nasdaq" finds NAS100USD.
 */
export function searchSymbols(all: string[], term: string, exclude: string[], limit = 10): string[] {
  const t = term.trim().toUpperCase();
  if (!t) return [];
  const score = (s: string) => {
    const name = (symbolInfo(s).name ?? '').toUpperCase();
    if (s === t) return 0;
    if (base(s) === t) return 1;
    // a word of the name: "oil" finds Brent crude oil, "dax" finds GER30USD
    if (name.split(/[\s&]+/).some((w) => w.startsWith(t))) return 2;
    if (s.startsWith(t)) return 3;
    if (s.includes(t)) return 4;
    if (name.includes(t)) return 5;
    return -1;
  };
  // Equally good matches: majors and the bigger markets before exotic and
  // crypto crosses; the stock form before the CFD, since it also carries sizes.
  const rank: Record<string, number> = {
    'FX major': 0, Metal: 1, 'US stock': 2, CFD: 3, FX: 4, ETF: 5, 'German stock': 6, Crypto: 7,
  };
  return all
    .filter((s) => !exclude.includes(s))
    .map((s) => [s, score(s) * 10 + (rank[symbolInfo(s).kind] ?? 7)] as const)
    .filter(([, n]) => n >= 0)
    .sort((a, b) => a[1] - b[1])
    .slice(0, limit)
    .map(([s]) => s);
}
