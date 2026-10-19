// Local shim of DefiLlama/dimension-adapters `adapters/types.ts`: only what fees/cadre uses.
// In the DefiLlama repo the real module is used and this directory is not copied.

export interface Balances {
  add(token: string, amount: bigint | number | string): void;
  getBalances(): Record<string, string>;
}

export interface FetchOptions {
  /** window [startTimestamp, endTimestamp), unix seconds */
  startTimestamp: number;
  endTimestamp: number;
  chain: string;
  createBalances(): Balances;
}

export type FetchResult = Record<string, Balances>;

export interface SimpleAdapter {
  version: 2;
  adapter: Record<string, { fetch: (options: FetchOptions) => Promise<FetchResult>; start: string }>;
  methodology?: Record<string, string>;
  breakdownMethodology?: Record<string, Record<string, string>>;
}

export function createBalances(): Balances {
  const m = new Map<string, bigint>();
  return {
    add(token, amount) {
      m.set(token, (m.get(token) ?? 0n) + BigInt(amount));
    },
    getBalances() {
      return Object.fromEntries([...m].map(([k, v]) => [k, v.toString()]));
    },
  };
}
