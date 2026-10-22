// Read-only Solana MAINNET client for practice mode (docs/STRATEGIES.md 3). Practice trades use
// real mainnet prices and state, and nothing else: this client can only call the read methods in
// READ_METHODS. Any other method (sendTransaction, simulateTransaction, requestAirdrop, ...) is
// refused before a byte leaves the process, so no code path here can put a transaction on mainnet.
//
// The URL carries the provider's key: it is read from the environment or a 0600 file and never
// printed, logged or put in an error message.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

export const READ_METHODS = new Set([
  "getAccountInfo",
  "getMultipleAccounts",
  "getTokenLargestAccounts",
  "getTokenSupply",
  "getSignaturesForAddress",
  "getTransaction",
  "getSlot",
  "getBlockTime",
  "getEpochInfo",
  "getGenesisHash",
]);

/** Mainnet-beta genesis hash: the client checks it once, so a devnet URL cannot pass for mainnet. */
export const MAINNET_GENESIS = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";

/**
 * Where the mainnet read URL comes from, in order: STRAT_MAINNET_RPC; the file named by
 * STRAT_MAINNET_RPC_FILE; keys/mainnet/solana-rpc-url; or the dedicated devnet URL
 * (keys/devnet/solana-rpc-url, /etc/lode/solana-rpc-url) when it is a Helius devnet URL, whose
 * key also serves mainnet reads (the host is swapped to mainnet.helius-rpc.com). Never logged.
 */
export function mainnetUrl(env: Record<string, string | undefined> = process.env, root = process.cwd()): string {
  if (env.STRAT_MAINNET_RPC) return env.STRAT_MAINNET_RPC.trim();
  const first = (f: string) => (existsSync(f) ? readFileSync(f, "utf8").split("\n")[0].trim() : "");
  if (env.STRAT_MAINNET_RPC_FILE) {
    const u = first(env.STRAT_MAINNET_RPC_FILE);
    if (u) return u;
  }
  const m = first(join(root, "keys/mainnet/solana-rpc-url"));
  if (m) return m;
  for (const f of [join(root, "keys/devnet/solana-rpc-url"), "/etc/lode/solana-rpc-url"]) {
    const u = first(f);
    if (/^https:\/\/devnet\.helius-rpc\.com\//.test(u)) return u.replace("https://devnet.helius-rpc.com/", "https://mainnet.helius-rpc.com/");
  }
  throw new Error("no mainnet read RPC: set STRAT_MAINNET_RPC or STRAT_MAINNET_RPC_FILE (the URL is never logged)");
}

export interface Acc { data: Buffer; owner: string; lamports: number }

export class ReadOnlySolana {
  #url: string;
  calls = 0;
  checked = false;
  fetcher: typeof fetch;
  constructor(url: string, fetcher: typeof fetch = fetch) {
    this.#url = url;
    this.fetcher = fetcher;
  }

  async call<T = any>(method: string, params: unknown[] = []): Promise<T> {
    if (!READ_METHODS.has(method)) throw new Error(`refused: ${method} is not a read method (practice mode never writes to mainnet)`);
    this.calls++;
    let res: Response;
    try {
      res = await this.fetcher(this.#url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: this.calls, method, params }), signal: AbortSignal.timeout(20_000) });
    } catch (e) {
      throw new Error(`mainnet ${method}: request failed (${(e as Error).name})`);
    }
    const j: any = await res.json().catch(() => null);
    if (!j) throw new Error(`mainnet ${method}: HTTP ${res.status}`);
    if (j.error) throw new Error(`mainnet ${method}: ${String(j.error.message ?? j.error.code).slice(0, 160)}`);
    return j.result as T;
  }

  /** Once per process: the endpoint must be mainnet-beta. */
  async ensureMainnet(): Promise<void> {
    if (this.checked) return;
    const g = await this.call<string>("getGenesisHash");
    if (g !== MAINNET_GENESIS) throw new Error("the practice read RPC is not Solana mainnet-beta (genesis hash differs)");
    this.checked = true;
  }

  /** getMultipleAccounts in pages of 100; returns the context slot of the LAST page (all are read at or after it). */
  async accounts(keys: string[]): Promise<{ slot: number; values: (Acc | null)[] }> {
    const values: (Acc | null)[] = [];
    let slot = 0;
    for (let i = 0; i < keys.length; i += 100) {
      const r = await this.call<any>("getMultipleAccounts", [keys.slice(i, i + 100), { encoding: "base64", commitment: "confirmed" }]);
      slot = Math.max(slot, r.context.slot);
      for (const v of r.value) values.push(v ? { data: Buffer.from(v.data[0], "base64"), owner: v.owner, lamports: v.lamports } : null);
    }
    return { slot, values };
  }

  async largest(mint: string): Promise<{ slot: number; list: { address: string; amount: bigint }[] }> {
    const r = await this.call<any>("getTokenLargestAccounts", [mint, { commitment: "confirmed" }]);
    return { slot: r.context.slot, list: r.value.map((x: any) => ({ address: x.address, amount: BigInt(x.amount) })) };
  }

  async signatures(address: string, limit: number): Promise<{ signature: string; slot: number; err: unknown }[]> {
    return this.call("getSignaturesForAddress", [address, { limit, commitment: "confirmed" }]);
  }

  async tx(sig: string): Promise<any> {
    return this.call("getTransaction", [sig, { encoding: "json", maxSupportedTransactionVersion: 0, commitment: "confirmed" }]);
  }

  async blockTime(slot: number): Promise<number | null> {
    return this.call<number | null>("getBlockTime", [slot]).catch(() => null);
  }
}
