// cadre-api composition: meta routes (index, OpenAPI contract, health, readiness) and the
// route groups. `build()` takes Deps so tests run it against fakes.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Api, ApiError, reply } from "./http.ts";
import type { Deps } from "./deps.ts";
import { registerStats } from "./routes/stats.ts";
import { registerWallet } from "./routes/wallet.ts";
import { registerMining } from "./routes/mining.ts";
import { registerAgents } from "./routes/agents.ts";
import { registerInference } from "./routes/inference.ts";
import { registerMarket } from "./routes/market.ts";
import { registerTown } from "./routes/town.ts";
import { registerDms } from "./routes/dms.ts";
import { registerDesktops } from "./routes/desktops.ts";
import { registerUserAgents } from "./routes/useragents.ts";
import { registerTemplates } from "./routes/templates.ts";
import { registerContracts } from "./routes/contracts.ts";
import { registerTxStatus } from "./routes/txstatus.ts";
import { registerTrading } from "./routes/trading.ts";
import { registerServices } from "./routes/services.ts";
import { registerLaunches } from "./routes/launches.ts";
import { registerBoards } from "./routes/boards.ts";
import { registerStrategies } from "./routes/strategies.ts";

export const VERSION = "1.0.0";
const here = dirname(fileURLToPath(import.meta.url));
export const OPENAPI_FILE = join(here, "..", "openapi.yaml");

export function build(d: Deps): Api {
  const api = new Api({
    allowedOrigins: d.cfg.allowedOrigins,
    rateLimitPerMin: d.cfg.rateLimitPerMin,
    rateBurst: d.cfg.rateBurst,
    trustProxy: d.cfg.trustProxy,
    maxBody: 1 << 20,
    log: process.env.CADRE_LOG === "0" ? undefined : (l) => (l.status as number) >= 500 || process.env.CADRE_LOG === "1" ? console.log(JSON.stringify(l)) : undefined,
  });

  api.get("/", () => reply({ status: 200, body: { name: "cadre-api", version: VERSION, see: "/v1" }, cache: 60 }));
  api.get("/v1", () => ({
    name: "cadre-api",
    version: VERSION,
    network: d.cfg.network,
    openapi: "/v1/openapi.yaml",
    docs: "docs/API.md in the pearl-sol repo",
    endpoints: api.routes.map((r) => `${r.method} ${r.pattern}`),
  }), { cache: 60 });

  api.get("/v1/openapi.yaml", () => {
    let text: string;
    try {
      text = readFileSync(OPENAPI_FILE, "utf8");
    } catch {
      throw new ApiError(500, "internal", "openapi.yaml missing from the build");
    }
    return reply({ text: { type: "application/yaml; charset=utf-8", body: text }, cache: 300 });
  }, { cache: 300 });

  api.get("/v1/health", () => ({ ok: true, version: VERSION, uptime_s: Math.round((Date.now() - d.startedAt) / 1000) }), { cache: "no-store" });

  api.get("/v1/ready", async () => {
    const ix = d.ix;
    const tip = ix.chainInfo?.height ?? 0;
    const indexed = ix.blocks.size;
    const checks: Record<string, { ok: boolean; detail?: unknown }> = {
      indexer_polled: { ok: ix.status.polls > 0, detail: { polls: ix.status.polls, last_poll_ms: ix.status.lastPoll || null } },
      node: { ok: ix.status.nodeOk, detail: ix.status.nodeError || undefined },
      solana: { ok: ix.status.solOk, detail: ix.status.solError || undefined },
      ...(d.town ? { town_solana: { ok: d.town.status.polls > 0 && d.town.status.ok, detail: { program: d.town.program, txs: d.town.recs.length, slot: d.town.status.slot, error: d.town.status.error || undefined } } } : {}),
      index_complete: { ok: tip > 0 && indexed >= tip, detail: { blocks_indexed: indexed, chain_height: tip, backfill_next: ix.backfillCursor } },
    };
    const optional: Record<string, { ok: boolean; detail?: unknown }> = {};
    for (const [name, url] of [["gateway", d.cfg.gatewayUrl], ["storefront", d.cfg.storefrontUrl], ["pool", d.cfg.poolUrl]] as const) {
      if (!url) continue;
      const r = await d.upstream(name, "/health", { timeoutMs: 2000 }).then((x) => ({ ok: x.ok, detail: x.status }), (e) => ({ ok: false, detail: (e as Error).message }));
      optional[name] = r;
    }
    const ready = Object.values(checks).every((c) => c.ok);
    return reply({ status: ready ? 200 : 503, body: { ready, checks, upstreams: optional, prepaid: d.cfg.prepaid, node_rpc: d.nodeGate.snapshot() }, cache: "no-store" });
  }, { cache: "no-store" });

  registerStats(api, d);
  registerWallet(api, d);
  registerMining(api, d);
  registerAgents(api, d);
  registerInference(api, d);
  registerMarket(api, d);
  registerTown(api, d);
  registerDms(api, d);
  registerDesktops(api, d);
  registerUserAgents(api, d);
  registerContracts(api, d);
  registerTxStatus(api, d);
  registerTrading(api, d);
  registerServices(api, d);
  registerLaunches(api, d);
  registerBoards(api, d);
  registerStrategies(api, d);
  registerTemplates(api, d);
  return api;
}
