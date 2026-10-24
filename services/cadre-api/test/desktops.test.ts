// House desktops routes (routes/desktops.ts) against a fake desktops controller: the agent list
// with fresh view-only stream paths, one agent's detail, the HLS proxy accepting only a valid
// token, and a clear 503 when no controller runs. Ports 10687 (fake controller), 10689 (api).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { mintStreamToken } from "../src/routes/desktops.ts";
import { checkStreamToken, mintStreamToken as mintCtl } from "../../swarm/desktop/src/token.ts";

const PORTS = { ctl: 10687, api: 10689 };
const SECRET = Buffer.from("0123456789abcdef0123456789abcdef");
let ctlSrv: Server, apiSrv: Server, dir: string, base: string;
let ctlUp = true;

const agent = {
  handle: "cartographer", name: "Fen Aldous", archetype: "mapmaker", persona: "Draws maps.", appearance: "a man with chalk", account: "GDcEUhU4fXRziN3YQyrwyoZJhsGhSRytJooX7kr7hC4s",
  desktops: { social: { state: "running", current: "reading the town", busy: true, error: null }, work: { state: "paused", current: "paused", busy: false, error: null } },
  latest_thought: { t: "2026-10-21T15:00:00Z", purpose: "post", request_id: "req-1", rows: 120, text_sha256: "ab".repeat(32), charged_lamports: null },
  counters: { posts: 1 }, spend_today: null,
};

const solo = {
  ...agent, handle: "ferry_nan", layout: "one_desktop", mode: "work", mode_since: "2026-10-21T20:00:00Z",
  desktops: { social: { state: "running", current: "reading the wallet", busy: true, error: null, host: "e2b", task_kind: "market", activity: "trading", last_action_at: "2026-10-21T20:01:00Z", health: { checked_at: "2026-10-21T20:01:00Z", stream_alive: true, stream_age_s: 1, encoder: true, browser_host: true, mem_total_mb: 1024, mem_avail_mb: 310, load1: 0.4 } } },
};

function controller() {
  return createServer((req, res) => {
    const u = new URL(req.url ?? "/", "http://x");
    if (!ctlUp) { res.writeHead(500); return res.end("{}"); }
    if (u.pathname === "/status") {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ stopped: false, capped: false, cap_usd_per_day: 25, spend_today: { total_usd: 1 }, measured: {}, e2b: {}, vision_model: "m", desktops_running: 2, desktops_total: 3, roles: ["social", "work"], layout: { two_desktops: 1, one_desktop: 1, mode_slot_s: 1800 }, health: { desktops: 3, running: 2, streams_alive: 2, unhealthy: [{ agent: "cartographer", role: "work", host: "e2b", state: "paused", issues: ["desktop paused"] }] }, agents: [agent, solo], personas_waiting: ["ferry_nan"] }));
    }
    if (u.pathname === "/agents/cartographer/reports") {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ agent: "cartographer", reports: [{ id: "cartographer-20261024010000-1", kind: "monitor", title: "Bridge: all clear", created: "2026-10-24T01:00:00Z", summary: "", sha256: "ab" }] }));
    }
    if (u.pathname === "/agents/cartographer/reports/cartographer-20261024010000-1") {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ id: "cartographer-20261024010000-1", body_md: "# Bridge", sha256: "ab" }));
    }
    if (u.pathname === "/agents/cartographer") {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ ...agent, log: [{ t: "x", desk: "social", kind: "task", text: "read town" }], thoughts: [agent.latest_thought] }));
    }
    const m = /^\/stream\/([a-z_]+)\/(social|work)\/(live\.m3u8|s\d{6}\.ts|thumb\.jpg)$/.exec(u.pathname);
    if (m) {
      if (!checkStreamToken(SECRET, m[1]!, m[2]!, u.searchParams.get("t") ?? "")) { res.writeHead(403, { "content-type": "application/json" }); return res.end('{"error":"bad token"}'); }
      if (m[3] === "live.m3u8") { res.writeHead(200, { "content-type": "application/vnd.apple.mpegurl" }); return res.end("#EXTM3U\n#EXTINF:2.0,\ns000001.ts?t=x\n"); }
      res.writeHead(200, { "content-type": "video/mp2t" });
      return res.end(Buffer.alloc(188, 0x47));
    }
    res.writeHead(404, { "content-type": "application/json" });
    res.end('{"error":"no such desktop agent"}');
  });
}

const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));

before(async () => {
  dir = mkdtempSync(join(tmpdir(), "cadre-desk-test-"));
  writeFileSync(join(dir, "stream.key"), SECRET);
  process.env.DESKTOPS_URL = `http://127.0.0.1:${PORTS.ctl}`;
  process.env.DESKTOPS_STREAM_SECRET = join(dir, "stream.key");
  ctlSrv = controller();
  await listen(ctlSrv, PORTS.ctl);
  const cfg = configFromEnv({ NODE_RPC: "http://127.0.0.1:9", SOLANA_RPC: "http://127.0.0.1:9", GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  const api = build(new Deps(cfg));
  api.o.log = undefined;
  apiSrv = api.listen(PORTS.api, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${PORTS.api}`;
});
after(() => {
  apiSrv?.close();
  ctlSrv?.close();
  rmSync(dir, { recursive: true, force: true });
});

test("token: the API's minting matches the controller's (same vector)", () => {
  const now = Date.UTC(2026, 9, 21, 12, 0, 0);
  const a = mintStreamToken(SECRET, "cartographer", "social", 600, now);
  assert.equal(a, mintCtl(SECRET, "cartographer", "social", 600, now));
  assert.equal(a, "1792584600.3fsUJC2pPuQ_zksrNsCHjphzjPIavGmcsQgOixD1cms");
  assert.ok(checkStreamToken(SECRET, "cartographer", "social", a, now));
  assert.ok(!checkStreamToken(SECRET, "cartographer", "work", a, now), "a token names one desktop");
  assert.ok(!checkStreamToken(SECRET, "cartographer", "social", a, now + 601_000), "a token expires");
});

test("GET /v1/desktops lists agents with a view-only stream path per running desktop", async () => {
  const r = await fetch(base + "/v1/desktops");
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("cache-control"), "no-store");
  const d = await r.json();
  assert.equal(d.agents.length, 2);
  const a = d.agents[0];
  // Phase A: a one-desktop agent gets exactly one stream, its health and mode pass through
  const s = d.agents[1];
  assert.deepEqual(Object.keys(s.streams), ["social"]);
  assert.match(s.streams.social.path, /^\/v1\/desktops\/ferry_nan\/social\/hls\/live\.m3u8\?t=/);
  assert.equal(s.layout, "one_desktop");
  assert.equal(s.mode, "work");
  assert.equal(s.desktops.social.health.stream_alive, true);
  assert.equal(s.desktops.social.activity, "trading");
  assert.deepEqual(d.layout, { two_desktops: 1, one_desktop: 1, mode_slot_s: 1800 });
  assert.equal(d.health.unhealthy.length, 1);
  assert.equal(d.desktops_total, 3);
  assert.match(a.streams.social.path, /^\/v1\/desktops\/cartographer\/social\/hls\/live\.m3u8\?t=\d{10}\.[A-Za-z0-9_-]{43}$/);
  assert.equal(a.streams.work, null, "no stream for a paused desktop");
  assert.deepEqual(d.personas_waiting, ["ferry_nan"]);
  assert.ok(!JSON.stringify(d).includes("sandbox"), "no sandbox ids leave the API");
});

test("GET /v1/desktops/:handle serves the action log and thoughts; unknown agent is 404", async () => {
  const d = await (await fetch(base + "/v1/desktops/cartographer")).json();
  assert.equal(d.log.length, 1);
  assert.equal(d.thoughts[0].request_id, "req-1");
  assert.ok(d.streams.social.path);
  assert.equal((await fetch(base + "/v1/desktops/nobody_here")).status, 404);
  assert.equal((await fetch(base + "/v1/desktops/BAD!")).status, 400);
});

test("HLS proxy: valid token streams the playlist and segments, a bad token is 403", async () => {
  const d = await (await fetch(base + "/v1/desktops")).json();
  const r = await fetch(base + d.agents[0].streams.social.path);
  assert.equal(r.status, 200);
  assert.match(r.headers.get("content-type") ?? "", /mpegurl/);
  assert.match(await r.text(), /#EXTM3U/);
  const t = new URL(base + d.agents[0].streams.social.path).searchParams.get("t")!;
  const seg = await fetch(`${base}/v1/desktops/cartographer/social/hls/s000001.ts?t=${encodeURIComponent(t)}`);
  assert.equal(seg.status, 200);
  assert.equal((await seg.arrayBuffer()).byteLength, 188);
  const forged = `${Math.floor(Date.now() / 1000) + 600}.${"A".repeat(43)}`;
  assert.equal((await fetch(`${base}/v1/desktops/cartographer/social/hls/live.m3u8?t=${forged}`)).status, 403);
  assert.equal((await fetch(`${base}/v1/desktops/cartographer/social/hls/live.m3u8`)).status, 403);
  assert.equal((await fetch(`${base}/v1/desktops/cartographer/social/hls/..%2Fetc?t=${forged}`)).status, 400);
});

test("no controller: 503 with a reason", async () => {
  ctlSrv.close();
  await new Promise((r) => setTimeout(r, 100));
  const r = await fetch(base + "/v1/desktops");
  assert.equal(r.status, 503);
  assert.match((await r.json()).error.message, /not running/);
  ctlSrv = controller();
  await listen(ctlSrv, PORTS.ctl);
});

test("reports: the list and one report pass through; a bad id is 400, a missing one 404", async () => {
  const l = await (await fetch(base + "/v1/desktops/cartographer/reports")).json();
  assert.equal(l.reports[0].kind, "monitor");
  const r = await (await fetch(base + "/v1/desktops/cartographer/reports/cartographer-20261024010000-1")).json();
  assert.equal(r.body_md, "# Bridge");
  assert.equal((await fetch(base + "/v1/desktops/cartographer/reports/..%2Fstatus")).status, 400);
  assert.equal((await fetch(base + "/v1/desktops/cartographer/reports/cartographer-20261024010000-9")).status, 404);
});
