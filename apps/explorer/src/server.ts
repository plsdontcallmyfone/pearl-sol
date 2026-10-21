// HTTP server: HTML pages, their JSON twins under /api, static assets. GET only.

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { readFileSync, existsSync } from "node:fs";
import { dirname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import type { Indexer } from "./indexer.ts";
import * as M from "./models.ts";
import { EVALUATOR_STATUSES, evaluatorJobsModel, type EvaluatorStatus } from "./evaluator.ts";
import * as V from "./views.ts";
import { readBuyback } from "./buyback.ts";
import { solTxPage } from "./soltx.ts";
import { revenueModel } from "./revenue.ts";
import { plainTokens, supplyModel } from "./supply.ts";
import { revenuePage } from "./revenue-view.ts";
import { agentRoute } from "./profile-route.ts";
import { contractRoute } from "./contract-route.ts";
import { addCx, cxInput } from "./cxjson.ts";

const PUBLIC = join(dirname(fileURLToPath(import.meta.url)), "..", "public");
const TYPES: Record<string, string> = { ".css": "text/css; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".woff2": "font/woff2" };

type Res = { status: number; type: string; body: string | Buffer; headers?: Record<string, string> };
// JSON twins carry `k_cx` next to every Cadre address field (base58 kept, docs/EXPLORER.md).
const json = (v: unknown, status = 200): Res => ({ status, type: "application/json; charset=utf-8", body: JSON.stringify(addCx(v), null, 1) });
const html = (body: string, status = 200): Res => ({ status, type: "text/html; charset=utf-8", body });
const text = (body: string, status = 200): Res => ({ status, type: "text/plain; charset=utf-8", body, headers: { "access-control-allow-origin": "*" } });

export async function route(ix: Indexer, method: string, rawUrl: string): Promise<Res> {
  if (method !== "GET" && method !== "HEAD") return json({ error: "read-only: GET only" }, 405);
  const u = new URL(rawUrl, "http://x");
  const p = u.pathname.replace(/\/+$/, "") || "/";
  const q = u.searchParams;
  const sm = M.summaryModel(ix);
  const parts = p.split("/").filter(Boolean).map(decodeURIComponent);
  // Cx addresses (SPEC 2.1) in the path or query read as their base58 key; a bad checksum is a 400.
  for (let i = 0; i < parts.length; i++) {
    const r = cxInput(parts[i]);
    if ("error" in r) return parts[0] === "api" ? json({ error: r.error, code: "bad_address", cx_code: r.code }, 400) : html(V.badAddress(M.summaryModel(ix), r.error), 400);
    parts[i] = r.value;
  }
  for (const [k, v] of [...q.entries()]) {
    const r = cxInput(v);
    if ("error" in r) return json({ error: `${k}: ${r.error}`, code: "bad_address", cx_code: r.code }, 400);
    q.set(k, r.value);
  }

  if (p === "/health") return json({ ok: ix.status.nodeOk, polls: ix.status.polls });
  if (parts[0] === "assets" || parts[0] === "fonts") {
    const f = normalize(join(PUBLIC, ...parts));
    if (!f.startsWith(PUBLIC) || !existsSync(f)) return json({ error: "not found" }, 404);
    const ext = f.slice(f.lastIndexOf("."));
    return { status: 200, type: TYPES[ext] ?? "application/octet-stream", body: readFileSync(f), headers: { "cache-control": ext === ".woff2" ? "public, max-age=31536000, immutable" : "no-cache" } };
  }

  const api = parts[0] === "api";
  const rest = api ? parts.slice(1) : parts;
  const ag = await agentRoute(ix, api, rest, q, sm);
  if (ag) return ag;
  const ct = await contractRoute(ix, api, rest, q, sm);
  if (ct) return ct;
  const filter = (["all", "paid", "fallback"].includes(q.get("filter") ?? "") ? q.get("filter") : "all") as "all" | "paid" | "fallback";
  const before = q.get("before") ? Number(q.get("before")) : undefined;
  const limit = q.get("limit") ? Number(q.get("limit")) : undefined;

  // ---- api
  if (api) {
    switch (rest[0]) {
      case undefined:
        return json({ endpoints: ["/api/summary", "/api/feed", "/api/block/{height|hash}", "/api/block/{height}/proof", "/api/jobs", "/api/jobs?evaluator={address}&status=", "/api/job/{id}", "/api/orders", "/api/order/{nonce}", "/api/sol/tx/{sig}", "/api/payers", "/api/buyback", "/api/revenue", "/api/supply", "/api/supply/circulating", "/api/supply/total", "/api/agents", "/api/agent/{address}", "/api/contracts?offset&codes_offset", "/api/contract/{address}?offset&topic", "/api/code/{hash}", "/api/tx/{hash}"] });
      case "summary":
        return json(sm);
      case "feed":
        return json(await M.feedModel(ix, { filter, before, limit }));
      case "block": {
        if (!rest[1]) break;
        if (rest[2] === "proof") return proof(ix, rest[1], q.get("format") === "bin");
        const b = await M.blockModel(ix, rest[1]);
        return b ? json(b) : json({ error: "block not found" }, 404);
      }
      case "jobs": {
        const ev = q.get("evaluator");
        if (ev) {
          const st = q.get("status") ?? "all";
          if (!(EVALUATOR_STATUSES as readonly string[]).includes(st)) return json({ error: `status is one of ${EVALUATOR_STATUSES.join(", ")}` }, 400);
          return json(evaluatorJobsModel(ix, ev, st as EvaluatorStatus));
        }
        return json(M.jobsModel(ix));
      }
      case "job": {
        const j = rest[1] ? await M.jobModel(ix, rest[1]) : null;
        return j ? json(j) : json({ error: "job not found" }, 404);
      }
      case "orders":
        return json(M.ordersModel(ix));
      case "order": {
        const o = rest[1] ? M.orderPageModel(ix, rest[1]) : null;
        return o ? json(o) : json({ error: "order not found" }, 404);
      }
      case "sol":
        if (rest[1] === "tx" && rest[2]) {
          const t = await ix.sol.getTransaction(rest[2]).catch((e) => ({ error: (e as Error).message }));
          return t ? json(t) : json({ error: "not in the Solana RPC's ledger (pruned or unknown)" }, 404);
        }
        break;
      case "payers":
        return json(M.payersModel(ix));
      case "buyback":
        return json(await readBuyback(ix));
      case "revenue":
        return json(revenueModel(ix));
      case "supply": {
        const sp = await supplyModel(ix);
        if (rest[1] === "circulating" || rest[1] === "total") {
          if (!sp.available) return text(sp.reason, 503);
          return text(plainTokens(rest[1] === "total" ? sp.total_supply : sp.circulating_supply));
        }
        return json(sp, sp.available ? 200 : 503);
      }
    }
    return json({ error: "unknown endpoint", see: "/api" }, 404);
  }

  // ---- fragments for live refresh
  const frag = q.get("fragment");
  if (frag === "stats") return html(V.heroAndStats(sm));
  if (frag === "status") return html(V.statusPill(sm));
  if (frag === "feed") return html(V.feedList(await M.feedModel(ix, { filter, before, limit })));
  if (frag === "rail") return html(V.rail(sm, M.jobsModel(ix), M.ordersModel(ix), await readBuyback(ix)));

  // ---- pages
  switch (rest[0]) {
    case undefined:
      return html(V.homePage(sm, await M.feedModel(ix, { filter: "all", before, limit }), M.jobsModel(ix), M.ordersModel(ix), await readBuyback(ix), M.payersModel(ix)));
    case "blocks":
      return html(V.blocksPage(sm, await M.feedModel(ix, { filter, before, limit })));
    case "block": {
      const b = rest[1] ? await M.blockModel(ix, rest[1]) : null;
      return b ? html(V.blockPage(sm, b)) : html(V.notFound(sm, "Block"), 404);
    }
    case "jobs":
      return html(V.jobsPage(sm, M.jobsModel(ix)));
    case "job": {
      const j = rest[1] ? await M.jobModel(ix, rest[1]) : null;
      return j ? html(V.jobPage(sm, j)) : html(V.notFound(sm, "Job"), 404);
    }
    case "orders":
      return html(V.ordersPage(sm, M.ordersModel(ix)));
    case "order": {
      const o = rest[1] ? M.orderPageModel(ix, rest[1]) : null;
      return o ? html(V.orderPage(sm, o)) : html(V.notFound(sm, "Order"), 404);
    }
    case "sol":
      if (rest[1] === "tx" && rest[2]) {
        const t = await ix.sol.getTransaction(rest[2]).catch(() => null);
        return t ? html(solTxPage(sm, rest[2], t)) : html(V.notFound(sm, "Solana tx (pruned from the RPC's ledger, or unknown)"), 404);
      }
      break;
    case "api":
      return html(V.apiPage(sm));
    case "revenue":
      return html(revenuePage(sm, revenueModel(ix), await supplyModel(ix).catch((e) => ({ available: false, reason: (e as Error).message }))));
  }
  if (p === "/docs") return html(V.apiPage(sm));
  return html(V.notFound(sm, "Page"), 404);
}

async function proof(ix: Indexer, id: string, bin: boolean): Promise<Res> {
  const h = await ix.node.call<any>("getHeader", /^\d+$/.test(id) ? { height: Number(id) } : { hash: id }).catch(() => null);
  if (!h) return json({ error: "block not found" }, 404);
  if (bin) return { status: 200, type: "application/octet-stream", body: Buffer.from(h.proof, "base64"), headers: { "content-disposition": `attachment; filename="proof-${h.header_fields.height}.bin"` } };
  return json({ height: h.header_fields.height, hash: h.hash, proof_hash: h.header_fields.proof_hash, proof_hash_rule: "proof_hash = blake3(proof bytes), committed in the header", bytes: Buffer.from(h.proof, "base64").length, proof: h.proof });
}

export function serve(ix: Indexer, port: number, host = "127.0.0.1") {
  const srv = createServer(async (req: IncomingMessage, res: ServerResponse) => {
    try {
      const r = await route(ix, req.method ?? "GET", req.url ?? "/");
      res.writeHead(r.status, { "content-type": r.type, "x-content-type-options": "nosniff", ...(r.type.startsWith("application/json") ? { "access-control-allow-origin": "*" } : {}), ...r.headers });
      res.end(req.method === "HEAD" ? undefined : r.body);
    } catch (e) {
      res.writeHead(500, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: (e as Error).message }));
    }
  });
  srv.listen(port, host);
  return srv;
}
