// Minimal HTTP layer for cadre-api (Node built-ins only): a router with path params,
// CORS, per-client rate limiting, Cache-Control + ETag (304 on If-None-Match), a short
// in-memory response cache for public GETs, one error shape, request ids, and
// streaming pass-through of upstream responses (SSE from the gateway).
//
// Error shape (every non-2xx JSON reply):
//   {"error": {"code": "not_found", "message": "...", "details"?: ...}, "request_id": "..."}

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { createHash, randomBytes } from "node:crypto";
import { Readable } from "node:stream";

export class ApiError extends Error {
  status: number;
  code: string;
  details?: unknown;
  constructor(status: number, code: string, message: string, details?: unknown) {
    super(message);
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

export const badRequest = (message: string, details?: unknown) => new ApiError(400, "bad_request", message, details);
export const notFound = (message: string) => new ApiError(404, "not_found", message);

/** Cache policy: seconds of public max-age (0 = no-cache, still ETag'd), or "no-store". */
export type Cache = number | "no-store";

export interface Ctx {
  method: string;
  path: string;
  params: Record<string, string>;
  query: URLSearchParams;
  headers: IncomingMessage["headers"];
  ip: string;
  requestId: string;
  req: IncomingMessage;
  /** Parsed JSON body (POST); {} when empty. */
  json(): Promise<any>;
  /** Raw body bytes (POST), capped. */
  raw(): Promise<Buffer>;
}

export interface Reply {
  status?: number;
  body?: unknown;
  headers?: Record<string, string>;
  cache?: Cache;
  /** Stream this upstream fetch Response through as is (status, content-type, selected headers). */
  upstream?: Response;
  /** A non-JSON body (e.g. the YAML contract). */
  text?: { type: string; body: string };
}

export type Handler = (c: Ctx) => Promise<unknown> | unknown;

interface Route {
  method: string;
  re: RegExp;
  keys: string[];
  pattern: string;
  handler: Handler;
  cache: Cache;
  cost: number;
}

export interface ServerOptions {
  /** "*" or a list of exact origins. */
  allowedOrigins: "*" | string[];
  /** Requests per minute per client (token bucket refill); 0 disables. */
  rateLimitPerMin: number;
  /** Bucket size (burst). */
  rateBurst: number;
  /** Take the client address from the last X-Forwarded-For entry (behind caddy). */
  trustProxy: boolean;
  /** Max JSON body bytes. */
  maxBody: number;
  log?: (line: Record<string, unknown>) => void;
}

const json = (v: unknown) => JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x), 1);

export const ALLOW_HEADERS = ["content-type", "authorization", "x-payment", "x-cadre-wallet", "x-cadre-account", "x-cadre-signature", "x-cadre-timestamp", "x-cadre-nonce", "if-none-match"];
/** The gateway's receipt headers (SPEC 5.4), copied from upstream replies and readable by browsers. */
export const RECEIPT_HEADERS = ["x-cadre-request", "x-cadre-ticket", "x-cadre-order", "x-cadre-rows", "x-cadre-charged-lamports", "x-cadre-receipt", "x-cadre-explorer"];
export const EXPOSE_HEADERS = ["etag", "x-request-id", "x-payment-response", "ratelimit-limit", "ratelimit-remaining", "ratelimit-reset", "retry-after", "x-cadre-upstream", ...RECEIPT_HEADERS];

export class Api {
  routes: Route[] = [];
  o: ServerOptions;
  private buckets = new Map<string, { tokens: number; at: number }>();
  private cache = new Map<string, { at: number; ttl: number; status: number; body: string; etag: string; headers: Record<string, string> }>();

  constructor(o: ServerOptions) {
    this.o = o;
  }

  add(method: string, pattern: string, handler: Handler, opts: { cache?: Cache; cost?: number } = {}) {
    const keys: string[] = [];
    const re = new RegExp(
      "^" +
        pattern.replace(/\/:([a-zA-Z_]+)(\*)?/g, (_m, k, star) => {
          keys.push(k);
          return star ? "/(.*)" : "/([^/]+)";
        }) +
        "/?$",
    );
    this.routes.push({ method, re, keys, pattern, handler, cache: opts.cache ?? 0, cost: opts.cost ?? 1 });
  }
  get(p: string, h: Handler, o?: { cache?: Cache; cost?: number }) {
    this.add("GET", p, h, o);
  }
  post(p: string, h: Handler, o?: { cache?: Cache; cost?: number }) {
    this.add("POST", p, h, { cache: "no-store", cost: 5, ...o });
  }
  delete(p: string, h: Handler, o?: { cache?: Cache; cost?: number }) {
    this.add("DELETE", p, h, { cache: "no-store", cost: 5, ...o });
  }

  // ------------------------------------------------------------ helpers

  private cors(origin: string | undefined): Record<string, string> {
    const h: Record<string, string> = { "access-control-expose-headers": EXPOSE_HEADERS.join(", ") };
    if (this.o.allowedOrigins === "*") h["access-control-allow-origin"] = "*";
    else {
      h["vary"] = "Origin";
      if (origin && this.o.allowedOrigins.includes(origin)) h["access-control-allow-origin"] = origin;
    }
    return h;
  }

  clientIp(req: IncomingMessage): string {
    if (this.o.trustProxy) {
      const xff = req.headers["x-forwarded-for"];
      const v = Array.isArray(xff) ? xff.join(",") : xff;
      if (v) {
        const parts = v.split(",").map((s) => s.trim()).filter(Boolean);
        if (parts.length) return parts[parts.length - 1];
      }
    }
    return req.socket.remoteAddress ?? "unknown";
  }

  /** Token bucket. Returns remaining tokens, or -seconds-to-wait when empty. */
  take(ip: string, cost: number): { ok: boolean; remaining: number; resetS: number } {
    const rpm = this.o.rateLimitPerMin;
    if (rpm <= 0) return { ok: true, remaining: Infinity, resetS: 0 };
    const now = Date.now();
    const perMs = rpm / 60_000;
    const b = this.buckets.get(ip) ?? { tokens: this.o.rateBurst, at: now };
    b.tokens = Math.min(this.o.rateBurst, b.tokens + (now - b.at) * perMs);
    b.at = now;
    let ok = true;
    if (b.tokens >= cost) b.tokens -= cost;
    else ok = false;
    this.buckets.set(ip, b);
    if (this.buckets.size > 50_000) {
      // drop full buckets (idle clients)
      for (const [k, v] of this.buckets) if (v.tokens >= this.o.rateBurst) this.buckets.delete(k);
    }
    const resetS = Math.ceil(Math.max(0, cost - b.tokens) / perMs / 1000);
    return { ok, remaining: Math.floor(b.tokens), resetS };
  }

  match(method: string, path: string): { route: Route; params: Record<string, string> } | { allowed: string[] } | null {
    const allowed: string[] = [];
    for (const r of this.routes) {
      const m = r.re.exec(path);
      if (!m) continue;
      if (r.method !== method && !(method === "HEAD" && r.method === "GET")) {
        allowed.push(r.method);
        continue;
      }
      const params: Record<string, string> = {};
      r.keys.forEach((k, i) => {
        try {
          params[k] = decodeURIComponent(m[i + 1]);
        } catch {
          params[k] = m[i + 1];
        }
      });
      return { route: r, params };
    }
    return allowed.length ? { allowed } : null;
  }

  cacheControl(c: Cache): string {
    if (c === "no-store") return "no-store";
    if (c <= 0) return "public, no-cache";
    return `public, max-age=${c}, stale-while-revalidate=${Math.max(c * 4, 10)}`;
  }

  // ------------------------------------------------------------ request handling

  async handle(req: IncomingMessage, res: ServerResponse) {
    const t0 = Date.now();
    const requestId = randomBytes(8).toString("hex");
    const method = (req.method ?? "GET").toUpperCase();
    const url = new URL(req.url ?? "/", "http://x");
    const path = url.pathname.replace(/\/{2,}/g, "/");
    const base: Record<string, string> = { ...this.cors(req.headers.origin), "x-request-id": requestId, "x-content-type-options": "nosniff" };
    const ip = this.clientIp(req);
    let status = 500;

    const send = (st: number, body: string, headers: Record<string, string>) => {
      status = st;
      res.writeHead(st, { ...base, ...headers });
      res.end(method === "HEAD" || st === 304 ? undefined : body);
    };
    const fail = (e: ApiError, extra: Record<string, string> = {}) =>
      send(e.status, json({ error: { code: e.code, message: e.message, ...(e.details !== undefined ? { details: e.details } : {}) }, request_id: requestId }), {
        "content-type": "application/json; charset=utf-8",
        "cache-control": "no-store",
        ...extra,
      });

    try {
      if (method === "OPTIONS") {
        status = 204;
        res.writeHead(204, {
          ...base,
          "access-control-allow-methods": "GET, HEAD, POST, DELETE, OPTIONS",
          "access-control-allow-headers": ALLOW_HEADERS.join(", "),
          "access-control-max-age": "600",
        });
        res.end();
        return;
      }
      const m = this.match(method, path);
      if (!m) return fail(new ApiError(404, "not_found", `no endpoint ${path}; see /v1`));
      if ("allowed" in m) return fail(new ApiError(405, "method_not_allowed", `${method} not allowed on ${path}`), { allow: [...new Set(m.allowed)].join(", ") });
      const { route, params } = m;

      const rl = this.take(ip, route.cost);
      if (this.o.rateLimitPerMin > 0) {
        base["ratelimit-limit"] = String(this.o.rateLimitPerMin);
        base["ratelimit-remaining"] = String(Math.max(0, rl.remaining));
        base["ratelimit-reset"] = String(rl.resetS);
      }
      if (!rl.ok) return fail(new ApiError(429, "rate_limited", `rate limit: ${this.o.rateLimitPerMin} requests per minute per client`), { "retry-after": String(Math.max(1, rl.resetS)) });

      // short in-memory cache for public GETs (same URL)
      const cacheable = (method === "GET" || method === "HEAD") && typeof route.cache === "number" && route.cache > 0;
      const key = path + url.search;
      if (cacheable) {
        const hit = this.cache.get(key);
        if (hit && Date.now() - hit.at < hit.ttl) {
          const inm = req.headers["if-none-match"];
          if (inm && inm === hit.etag) return send(304, "", { etag: hit.etag, "cache-control": this.cacheControl(route.cache) });
          return send(hit.status, hit.body, { ...hit.headers, etag: hit.etag, "x-cache": "hit" });
        }
      }

      let rawBody: Buffer | null = null;
      const raw = async () => {
        if (rawBody) return rawBody;
        const chunks: Buffer[] = [];
        let size = 0;
        for await (const c of req) {
          size += (c as Buffer).length;
          if (size > this.o.maxBody) throw new ApiError(413, "body_too_large", `body above ${this.o.maxBody} bytes`);
          chunks.push(c as Buffer);
        }
        rawBody = Buffer.concat(chunks);
        return rawBody;
      };
      const ctx: Ctx = {
        method,
        path,
        params,
        query: url.searchParams,
        headers: req.headers,
        ip,
        requestId,
        req,
        raw,
        async json() {
          const b = await raw();
          if (b.length === 0) return {};
          try {
            return JSON.parse(b.toString("utf8"));
          } catch {
            throw badRequest("body is not JSON");
          }
        },
      };

      const out = await route.handler(ctx);
      const r: Reply = out instanceof ReplyBox ? out.r : { body: out };

      if (r.upstream) {
        const u = r.upstream;
        const h: Record<string, string> = { "cache-control": "no-store" };
        for (const k of ["content-type", "x-payment-response", "x-request-id", ...RECEIPT_HEADERS]) {
          const v = u.headers.get(k);
          if (v) h[k === "x-request-id" ? "x-upstream-request-id" : k] = v;
        }
        h["x-cadre-upstream"] = "gateway";
        status = u.status;
        res.writeHead(u.status, { ...base, ...h, ...(r.headers ?? {}) });
        if (method === "HEAD" || !u.body) {
          res.end();
          return;
        }
        await new Promise<void>((resolve) => {
          const s = Readable.fromWeb(u.body as any);
          s.on("error", () => {
            res.destroy();
            resolve();
          });
          res.on("close", () => {
            s.destroy();
            resolve();
          });
          s.pipe(res);
        });
        return;
      }

      const st = r.status ?? 200;
      const body = r.text ? r.text.body : json(r.body ?? null);
      const cachePolicy = r.cache ?? route.cache;
      const etag = `W/"${createHash("sha1").update(body).digest("base64url").slice(0, 27)}"`;
      const headers: Record<string, string> = {
        "content-type": r.text ? r.text.type : "application/json; charset=utf-8",
        "cache-control": st >= 400 ? "no-store" : this.cacheControl(cachePolicy),
        ...(st < 300 && cachePolicy !== "no-store" ? { etag } : {}),
        ...(r.headers ?? {}),
      };
      if (cacheable && st === 200 && typeof cachePolicy === "number" && cachePolicy > 0) {
        this.cache.set(key, { at: Date.now(), ttl: cachePolicy * 1000, status: st, body, etag, headers });
        if (this.cache.size > 1000) this.cache.delete(this.cache.keys().next().value!);
      }
      const inm = req.headers["if-none-match"];
      if (st === 200 && cachePolicy !== "no-store" && inm && inm === etag) return send(304, "", { etag, "cache-control": headers["cache-control"] });
      return send(st, body, headers);
    } catch (e) {
      if (e instanceof ApiError) return fail(e);
      const msg = (e as Error).message ?? String(e);
      if (/fetch failed|ECONNREFUSED|ETIMEDOUT|aborted|timeout/i.test(msg)) return fail(new ApiError(503, "upstream_unavailable", `upstream unreachable: ${msg.slice(0, 200)}`));
      return fail(new ApiError(500, "internal", msg.slice(0, 300)));
    } finally {
      this.o.log?.({ t: new Date().toISOString(), method, path, status, ms: Date.now() - t0, ip, id: requestId });
    }
  }

  listen(port: number, host: string): Server {
    const srv = createServer((req, res) => {
      this.handle(req, res).catch(() => {
        try {
          res.destroy();
        } catch {
          /* gone */
        }
      });
    });
    srv.keepAliveTimeout = 65_000;
    srv.headersTimeout = 70_000;
    srv.requestTimeout = 300_000;
    srv.listen(port, host);
    return srv;
  }
}

class ReplyBox {
  r: Reply;
  constructor(r: Reply) {
    this.r = r;
  }
}
/** Wrap a handler result that needs a status, headers, a cache override or an upstream stream. */
export const reply = (r: Reply) => new ReplyBox(r);
