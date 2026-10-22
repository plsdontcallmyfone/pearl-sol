// JSON-RPC clients: the agent-chain node (docs/RPC.md, supports batches) and a Solana
// RPC. Read-only: no method that writes is ever called.

export class RpcError extends Error {}

let ids = 1;

/** scheme://host of an RPC URL: a keyed RPC (key in the path or query) never reaches logs or replies. */
export function redactUrl(u: string): string {
  try {
    const x = new URL(u);
    return `${x.protocol}//${x.host}`;
  } catch {
    return "(rpc url)";
  }
}

async function post(url: string, body: unknown, timeoutMs: number): Promise<any> {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (!res.ok) throw new RpcError(`${redactUrl(url)}: HTTP ${res.status}`);
  return res.json();
}

/**
 * Optional shared limit on requests in flight to one node, with per-method counts and latency, so a
 * busy reader (cadre-api) never floods the node's RPC (2026-10-22: node 0's event loop saturated by
 * about 8 concurrent local readers). One gate is shared by every JsonRpc that talks to the node.
 */
export class RpcGate {
  max: number;
  inFlight = 0;
  private waiting: (() => void)[] = [];
  since = Date.now();
  stats = new Map<string, { calls: number; requests: number; errors: number; ms: number; maxMs: number }>();
  waitMs = 0;
  peakQueue = 0;
  constructor(max: number) {
    this.max = Math.max(1, max);
  }
  async run<T>(methods: string[], f: () => Promise<T>): Promise<T> {
    const q0 = Date.now();
    if (this.inFlight >= this.max) {
      this.peakQueue = Math.max(this.peakQueue, this.waiting.length + 1);
      await new Promise<void>((ok) => this.waiting.push(ok));
    }
    this.inFlight++;
    this.waitMs += Date.now() - q0;
    const t0 = Date.now();
    let err = false;
    try {
      return await f();
    } catch (e) {
      err = true;
      throw e;
    } finally {
      const ms = Date.now() - t0;
      const key = methods.length === 1 ? methods[0] : `batch(${[...new Set(methods)].sort().join(",")})`;
      const s = this.stats.get(key) ?? { calls: 0, requests: 0, errors: 0, ms: 0, maxMs: 0 };
      s.requests++;
      s.calls += methods.length;
      if (err) s.errors++;
      s.ms += ms;
      s.maxMs = Math.max(s.maxMs, ms);
      this.stats.set(key, s);
      this.inFlight--;
      this.waiting.shift()?.();
    }
  }
  snapshot() {
    const secs = Math.max(1, (Date.now() - this.since) / 1000);
    let req = 0, calls = 0, ms = 0;
    const methods = [...this.stats.entries()].map(([k, v]) => {
      req += v.requests;
      calls += v.calls;
      ms += v.ms;
      return { method: k, requests: v.requests, calls: v.calls, errors: v.errors, avg_ms: Math.round(v.ms / v.requests), max_ms: v.maxMs, per_min: Math.round((v.requests * 600) / secs) / 10 };
    }).sort((a, b) => b.requests - a.requests);
    return {
      max_in_flight: this.max, in_flight: this.inFlight, queued: this.waiting.length, peak_queue: this.peakQueue, window_s: Math.round(secs),
      requests: req, calls, requests_per_s: Math.round((req * 100) / secs) / 100, avg_ms: req ? Math.round(ms / req) : null, avg_wait_ms: req ? Math.round(this.waitMs / req) : null, methods,
    };
  }
}

export class JsonRpc {
  url: string;
  timeoutMs: number;
  gate: RpcGate | null;
  constructor(url: string, timeoutMs = 30_000, gate: RpcGate | null = null) {
    this.url = url;
    this.timeoutMs = timeoutMs;
    this.gate = gate;
  }

  private send(methods: string[], body: unknown): Promise<any> {
    return this.gate ? this.gate.run(methods, () => post(this.url, body, this.timeoutMs)) : post(this.url, body, this.timeoutMs);
  }

  async call<T = any>(method: string, params: unknown): Promise<T> {
    const r = await this.send([method], { jsonrpc: "2.0", id: ids++, method, params });
    if (r.error) throw new RpcError(`${method}: ${r.error.message ?? JSON.stringify(r.error)}`);
    return r.result as T;
  }

  /** One HTTP request for many calls; results in input order (an error entry becomes an Error). */
  async batch<T = any>(calls: [string, unknown][]): Promise<(T | Error)[]> {
    if (calls.length === 0) return [];
    const base = ids;
    ids += calls.length;
    const body = calls.map(([method, params], i) => ({ jsonrpc: "2.0", id: base + i, method, params }));
    const r = await this.send(calls.map(([m]) => m), body);
    const byId = new Map<number, any>((Array.isArray(r) ? r : [r]).map((x: any) => [x.id, x]));
    return calls.map((_, i) => {
      const x = byId.get(base + i);
      if (!x) return new RpcError("missing batch response");
      if (x.error) return new RpcError(x.error.message ?? JSON.stringify(x.error));
      return x.result as T;
    });
  }
}

/** Splits big batches so one request stays small. */
export async function chunkedBatch<T>(rpc: JsonRpc, calls: [string, unknown][], size = 50, parallel = 4): Promise<(T | Error)[]> {
  const chunks: [string, unknown][][] = [];
  for (let i = 0; i < calls.length; i += size) chunks.push(calls.slice(i, i + size));
  const out: (T | Error)[][] = new Array(chunks.length);
  let next = 0;
  await Promise.all(
    Array.from({ length: Math.min(parallel, chunks.length) }, async () => {
      while (next < chunks.length) {
        const i = next++;
        try {
          out[i] = await rpc.batch<T>(chunks[i]);
        } catch (e) {
          out[i] = chunks[i].map(() => e as Error);
        }
      }
    }),
  );
  return out.flat();
}

export interface SolAccount {
  address: string;
  data: Uint8Array;
  owner: string;
  lamports: number;
}

export class SolanaRpc {
  rpc: JsonRpc;
  constructor(url: string) {
    this.rpc = new JsonRpc(url);
  }

  async getMultipleAccounts(addresses: string[]): Promise<(SolAccount | null)[]> {
    const out: (SolAccount | null)[] = [];
    for (let i = 0; i < addresses.length; i += 100) {
      const part = addresses.slice(i, i + 100);
      const r = await this.rpc.call<any>("getMultipleAccounts", [part, { encoding: "base64", commitment: "confirmed" }]);
      r.value.forEach((v: any, j: number) =>
        out.push(v ? { address: part[j], data: Uint8Array.from(Buffer.from(v.data[0], "base64")), owner: v.owner, lamports: v.lamports } : null),
      );
    }
    return out;
  }

  async getProgramAccounts(program: string, discBase58: string): Promise<SolAccount[]> {
    const r = await this.rpc.call<any[]>("getProgramAccounts", [
      program,
      { encoding: "base64", commitment: "confirmed", filters: [{ memcmp: { offset: 0, bytes: discBase58 } }] },
    ]);
    return r.map((x) => ({ address: x.pubkey, data: Uint8Array.from(Buffer.from(x.account.data[0], "base64")), owner: x.account.owner, lamports: x.account.lamports }));
  }

  getSignatures(address: string, limit = 50, before?: string): Promise<{ signature: string; slot: number; err: unknown; blockTime: number | null }[]> {
    return this.rpc.call("getSignaturesForAddress", [address, { limit, commitment: "confirmed", ...(before ? { before } : {}) }]);
  }

  getTransaction(sig: string): Promise<any> {
    return this.rpc.call("getTransaction", [sig, { encoding: "json", commitment: "confirmed", maxSupportedTransactionVersion: 0 }]);
  }
}
