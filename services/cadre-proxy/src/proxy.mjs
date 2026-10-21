// cadre-proxy HTTP server: a local OpenAI-compatible endpoint. Requests from local
// clients are checked against the spend caps, signed with the proxy's wallet (or sent
// with its gateway API key) and forwarded to the Cadre inference gateway; responses,
// including SSE streams, pass through unchanged and the receipt is booked in the ledger.
import { createServer } from "node:http";
import { timingSafeEqual } from "node:crypto";
import { audienceOf, gatewayHeaders } from "./auth.mjs";

const PASS_HEADERS = ["content-type", "cache-control", "x-cadre-request", "x-cadre-ticket", "x-cadre-order", "x-cadre-rows", "x-cadre-charged-lamports", "x-cadre-receipt", "x-cadre-explorer", "x-payment-response"];
const AUTHED_FORWARD = [
  ["GET", /^\/v1\/balance$/],
  ["POST", /^\/v1\/balance\/close$/],
  ["GET", /^\/v1\/requests\/[^/]+$/],
  ["GET", /^\/v1\/receipts(\/[^/]+)?$/],
  ["GET", /^\/v1\/stats$/],
];

export function openaiError(status, message, type = "invalid_request_error", code = null) {
  return { status, body: { error: { message, type, code } } };
}

/** Rough upper estimate of prompt tokens: 1 token per 3 characters plus 8 per message. */
export function estimatePromptTokens(body, chat) {
  let chars = 0;
  let msgs = 0;
  const text = (c) => {
    if (typeof c === "string") return c.length;
    if (Array.isArray(c)) return c.reduce((n, p) => n + (typeof p?.text === "string" ? p.text.length : JSON.stringify(p ?? "").length), 0);
    return c ? JSON.stringify(c).length : 0;
  };
  if (chat) {
    for (const m of body.messages ?? []) {
      msgs++;
      chars += text(m.content) + (m.tool_calls ? JSON.stringify(m.tool_calls).length : 0);
    }
    if (body.tools) chars += JSON.stringify(body.tools).length;
  } else {
    const p = Array.isArray(body.prompt) ? body.prompt.join("") : String(body.prompt ?? "");
    chars += p.length;
    msgs = 1;
  }
  return Math.ceil(chars / 3) + 8 * msgs;
}

export class Proxy {
  /**
   * cfg: {gateway, wallet, apiKey?, ledger, token?, defaultMaxTokens, log?(obj), pricesTtlMs?, allowOrigins?: string[]}
   */
  constructor(cfg) {
    this.cfg = { defaultMaxTokens: 256, pricesTtlMs: 60_000, log: (o) => process.stderr.write(JSON.stringify(o) + "\n"), ...cfg };
    this.gateway = this.cfg.gateway.replace(/\/+$/, "");
    this.models = null;
    this.modelsAt = 0;
  }

  log(o) {
    this.cfg.log({ t: new Date().toISOString(), ...o });
  }

  async gw(method, path, body, { auth = true, signal } = {}) {
    const buf = body === undefined || body === null ? Buffer.alloc(0) : Buffer.isBuffer(body) ? body : Buffer.from(typeof body === "string" ? body : JSON.stringify(body));
    const headers = { accept: "application/json, text/event-stream" };
    if (buf.length) headers["content-type"] = "application/json";
    if (auth) {
      const audience = this.cfg.apiKey ? null : await this.audience();
      Object.assign(headers, gatewayHeaders({ wallet: this.cfg.wallet, apiKey: this.cfg.apiKey, audience }, method, path, buf));
    }
    return fetch(this.gateway + path, { method, headers, body: method === "GET" || method === "DELETE" ? undefined : buf, signal });
  }

  /** The gateway's server key (cadre-auth-v2 audience), from its /v1/models. */
  async audience() {
    if (!this.aud) {
      const r = await this.gw("GET", "/v1/models", null, { auth: false });
      if (!r.ok) throw new Error(`gateway /v1/models: HTTP ${r.status}`);
      this.aud = audienceOf(await r.json());
    }
    return this.aud;
  }

  async modelList() {
    if (!this.models || Date.now() - this.modelsAt > this.cfg.pricesTtlMs) {
      const r = await this.gw("GET", "/v1/models", null, { auth: false });
      if (!r.ok) throw new Error(`gateway /v1/models: HTTP ${r.status}`);
      this.models = (await r.json()).data ?? [];
      this.modelsAt = Date.now();
    }
    return this.models;
  }

  /** lamports_per_row of the requested model (BigInt) or null when the gateway publishes none. */
  async price(modelName) {
    const ms = await this.modelList();
    const m = ms.find((x) => x.id === modelName || x.cadre?.model_id === modelName) ?? (modelName ? null : ms[0]);
    if (!m) return { found: false, lamportsPerRow: null };
    const p = m.cadre?.pricing?.lamports_per_row;
    return { found: true, model: m.id, lamportsPerRow: p === undefined || p === null || p === "" ? null : BigInt(p) };
  }

  localAuthOk(req) {
    if (!this.cfg.token) return true;
    const h = req.headers.authorization ?? "";
    const got = Buffer.from(h.replace(/^bearer\s+/i, ""));
    const want = Buffer.from(this.cfg.token);
    return got.length === want.length && timingSafeEqual(got, want);
  }

  /**
   * Browser guard. The proxy spends from its wallet and, on loopback, needs no token, so a web
   * page the user visits must not be able to drive it: browsers attach Origin to every
   * cross-origin POST (also no-cors ones, which skip the CORS preflight), so a request with an
   * Origin not in cfg.allowOrigins is refused; and a Host that is not this loopback listener
   * (DNS rebinding: evil.example resolving to 127.0.0.1) is refused too.
   */
  browserGuard(req) {
    const origin = req.headers.origin;
    if (origin !== undefined && !(this.cfg.allowOrigins ?? []).includes(origin)) return `requests from a web page (Origin ${String(origin).slice(0, 80)}) are refused; allow it with CADRE_PROXY_ALLOW_ORIGINS`;
    if (this.loopback) {
      const host = String(req.headers.host ?? "").toLowerCase().replace(/:\d+$/, "");
      if (!["127.0.0.1", "localhost", "[::1]", "::1"].includes(host)) return `Host ${String(req.headers.host ?? "").slice(0, 80)} is not this loopback listener (DNS rebinding guard)`;
    }
    return null;
  }

  async handle(req, res) {
    const url = new URL(req.url, "http://local");
    const path = url.pathname;
    const send = ({ status, body }) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    const refused = this.browserGuard(req);
    if (refused) return send(openaiError(403, refused, "invalid_request_error", "forbidden_origin"));
    try {
      if (req.method === "GET" && path === "/health") {
        return send({ status: 200, body: { ok: true, service: "cadre-proxy", wallet: this.cfg.wallet.address, gateway: this.gateway, auth: this.cfg.apiKey ? "api-key" : "wallet", spend: this.cfg.ledger.summary() } });
      }
      if (!this.localAuthOk(req)) return send(openaiError(401, "missing or wrong local bearer token for cadre-proxy", "invalid_request_error", "invalid_api_key"));
      const body = await readBody(req);
      if (req.method === "GET" && path === "/cadre/spend") return send({ status: 200, body: this.cfg.ledger.summary() });
      if (req.method === "GET" && path === "/v1/models") {
        const r = await this.gw("GET", "/v1/models", null, { auth: false });
        return pipe(r, res);
      }
      if (req.method === "POST" && (path === "/v1/chat/completions" || path === "/v1/completions")) {
        return await this.infer(path, path === "/v1/chat/completions", body, res);
      }
      if (AUTHED_FORWARD.some(([m, re]) => m === req.method && re.test(path))) {
        const r = await this.gw(req.method, path + url.search, body.length ? body : null);
        return pipe(r, res);
      }
      return send(openaiError(404, `cadre-proxy has no route ${req.method} ${path}`, "invalid_request_error", "not_found"));
    } catch (e) {
      this.log({ event: "error", path, error: String(e?.message ?? e) });
      if (!res.headersSent) send(openaiError(502, `cadre-proxy: ${e?.message ?? e}`, "api_error", "gateway_unreachable"));
      else res.end();
    }
  }

  async infer(path, chat, raw, res) {
    let body;
    try {
      body = JSON.parse(raw.toString("utf8") || "{}");
    } catch {
      return sendJson(res, openaiError(400, "request body is not JSON"));
    }
    if (body.max_tokens === undefined && body.max_completion_tokens === undefined) body.max_tokens = this.cfg.defaultMaxTokens;
    const maxNew = Number(body.max_completion_tokens ?? body.max_tokens);
    const rows = estimatePromptTokens(body, chat) + maxNew;
    const pr = await this.price(body.model);
    if (!pr.found) return sendJson(res, openaiError(404, `the gateway serves no model ${JSON.stringify(body.model)}`, "invalid_request_error", "model_not_found"));
    const lamports = pr.lamportsPerRow === null ? null : pr.lamportsPerRow * BigInt(rows);
    const rsv = this.cfg.ledger.reserve(rows, lamports);
    if (!rsv.ok) {
      this.log({ event: "refused", path, est_rows: rows, est_lamports: lamports?.toString() ?? null, reason: rsv.message });
      return sendJson(res, openaiError(429, `cadre-proxy spend cap: ${rsv.message}`, "insufficient_quota", "cadre_spend_cap"));
    }
    const fwd = Buffer.from(JSON.stringify(body));
    let r;
    try {
      r = await this.gw("POST", path, fwd);
    } catch (e) {
      this.cfg.ledger.release(rsv.id);
      throw e;
    }
    const book = (receipt) => {
      if (!receipt) return this.cfg.ledger.release(rsv.id);
      let charged = receipt.charged_lamports;
      if ((charged === undefined || charged === null || charged === "") && pr.lamportsPerRow !== null && receipt.rows !== undefined) charged = (pr.lamportsPerRow * BigInt(receipt.rows)).toString();
      const rec = { ...receipt, charged_lamports: charged ?? null, model: pr.model };
      this.cfg.ledger.record(rsv.id, rec);
      this.log({ event: "served", path, request_id: rec.request_id ?? null, ticket_id: rec.ticket_id ?? null, rows: Number(rec.rows ?? 0), charged_lamports: rec.charged_lamports, est_rows: rows });
    };
    const ct = r.headers.get("content-type") ?? "";
    if (!r.ok) {
      this.cfg.ledger.release(rsv.id);
      return pipe(r, res);
    }
    if (ct.includes("text/event-stream")) {
      writeHead(r, res);
      let receipt = null;
      let carry = "";
      const dec = new TextDecoder();
      for await (const chunk of r.body) {
        res.write(chunk);
        carry += dec.decode(chunk, { stream: true });
        const lines = carry.split("\n");
        carry = lines.pop();
        for (const l of lines) {
          if (!l.startsWith("data:")) continue;
          const d = l.slice(5).trim();
          if (!d || d === "[DONE]" || !d.includes("\"cadre\"")) continue;
          try {
            const j = JSON.parse(d);
            if (j.cadre) receipt = j.cadre;
          } catch {}
        }
      }
      res.end();
      return book(receipt);
    }
    const text = Buffer.from(await r.arrayBuffer());
    writeHead(r, res);
    res.end(text);
    let receipt = null;
    try {
      receipt = JSON.parse(text.toString("utf8")).cadre ?? null;
    } catch {}
    const hRows = r.headers.get("x-cadre-rows");
    if (hRows !== null && hRows !== "") {
      receipt = {
        ...(receipt ?? {}),
        rows: Number(hRows),
        charged_lamports: r.headers.get("x-cadre-charged-lamports") || receipt?.charged_lamports || null,
        request_id: r.headers.get("x-cadre-request") ?? receipt?.request_id,
        ticket_id: r.headers.get("x-cadre-ticket") ?? receipt?.ticket_id,
      };
    }
    return book(receipt);
  }

  listen(port, host = "127.0.0.1") {
    const loopback = host === "127.0.0.1" || host === "::1" || host === "localhost";
    this.loopback = loopback;
    if (!loopback && !this.cfg.token) throw new Error(`refusing to listen on ${host} without --token (the proxy spends from its wallet)`);
    this.server = createServer((req, res) => this.handle(req, res));
    return new Promise((ok, fail) => {
      this.server.once("error", fail);
      this.server.listen(port, host, () => ok(this.server));
    });
  }

  close() {
    return new Promise((ok) => {
      if (!this.server) return ok();
      this.server.close(() => ok());
      this.server.closeAllConnections();
    });
  }
}

function readBody(req) {
  return new Promise((ok, fail) => {
    const parts = [];
    let n = 0;
    req.on("data", (c) => {
      n += c.length;
      if (n > 8 << 20) {
        fail(new Error("request body over 8 MB"));
        req.destroy();
      } else parts.push(c);
    });
    req.on("end", () => ok(Buffer.concat(parts)));
    req.on("error", fail);
  });
}

function writeHead(r, res) {
  const h = {};
  for (const k of PASS_HEADERS) {
    const v = r.headers.get(k);
    if (v !== null) h[k] = v;
  }
  res.writeHead(r.status, h);
}

async function pipe(r, res) {
  writeHead(r, res);
  if (r.body) for await (const c of r.body) res.write(c);
  res.end();
}

function sendJson(res, { status, body }) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}
