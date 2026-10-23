// A small, polite cadre-api / RPC client: bounded concurrency, honours RateLimit-* and Retry-After,
// retries 429 / 5xx / network errors with backoff. Never sends credentials anywhere.

export class HttpError extends Error {
  status: number;
  body: any;
  constructor(status: number, msg: string, body: any) {
    super(msg);
    this.status = status;
    this.body = body;
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export interface ApiOptions {
  base: string;
  rpc: string;
  concurrency?: number;
  retries?: number;
  timeoutMs?: number;
  log?: (s: string) => void;
}

export class Api {
  base: string;
  rpcUrl: string;
  conc: number;
  retries: number;
  timeoutMs: number;
  active = 0;
  waiters: (() => void)[] = [];
  pauseUntil = 0;
  calls = 0;
  log: (s: string) => void;
  constructor(o: ApiOptions) {
    this.base = o.base.replace(/\/$/, "");
    this.rpcUrl = o.rpc.replace(/\/$/, "");
    this.conc = o.concurrency ?? 3;
    this.retries = o.retries ?? 5;
    this.timeoutMs = o.timeoutMs ?? 30_000;
    this.log = o.log ?? (() => {});
  }

  async slot<T>(f: () => Promise<T>): Promise<T> {
    while (this.active >= this.conc) await new Promise<void>((r) => this.waiters.push(r));
    this.active++;
    try {
      return await f();
    } finally {
      this.active--;
      this.waiters.shift()?.();
    }
  }

  async request(url: string, init: RequestInit = {}): Promise<any> {
    let last: unknown = null;
    for (let i = 0; i <= this.retries; i++) {
      const wait = this.pauseUntil - Date.now();
      if (wait > 0) await sleep(wait);
      let r: Response;
      try {
        this.calls++;
        const t0 = Date.now();
        r = await this.slot(() => fetch(url, { ...init, headers: { accept: "application/json", "user-agent": "cadre-agent-dump/0.1", ...(init.headers ?? {}) }, signal: AbortSignal.timeout(this.timeoutMs) }));
        if (Date.now() - t0 > 5000) this.log(`slow ${Date.now() - t0} ms: ${url.replace(this.base, "").slice(0, 80)}`);
      } catch (e) {
        last = e;
        this.log(`network error on ${url.replace(this.base, "").slice(0, 80)}: ${(e as Error).message}`);
        await sleep(1000 * 2 ** i);
        continue;
      }
      const rem = Number(r.headers.get("ratelimit-remaining") ?? "NaN");
      const reset = Number(r.headers.get("ratelimit-reset") ?? "NaN");
      if (Number.isFinite(rem) && rem < 10 && Number.isFinite(reset)) this.pauseUntil = Math.max(this.pauseUntil, Date.now() + reset * 1000);
      const text = await r.text();
      let body: any = null;
      try { body = text ? JSON.parse(text) : null; } catch { body = text; }
      if (r.status === 429 || r.status >= 500) {
        const ra = Number(r.headers.get("retry-after") ?? "NaN");
        const ms = Number.isFinite(ra) ? ra * 1000 : 1000 * 2 ** i;
        this.log(`${r.status} on ${url.replace(this.base, "")}, retry in ${ms} ms`);
        last = new HttpError(r.status, `${r.status} ${url}`, body);
        if (r.status === 503 || r.status === 501) throw last; // a missing component, not a hiccup
        await sleep(ms);
        continue;
      }
      if (!r.ok) throw new HttpError(r.status, `${r.status} ${url}: ${typeof body === "object" ? body?.error?.message ?? "" : String(body).slice(0, 120)}`, body);
      return body;
    }
    throw last instanceof Error ? last : new Error(String(last));
  }

  get(path: string): Promise<any> {
    return this.request(this.base + path);
  }

  async rpc(method: string, params: Record<string, unknown>): Promise<any> {
    const b = await this.request(this.rpcUrl, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
    if (b?.error) throw new Error(`rpc ${method}: ${b.error.message}`);
    return b?.result;
  }
}
