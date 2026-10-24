// cadre-desktops: the house desktops controller (services/swarm/desktop/README.md).
//   node src/main.ts run          run the controller (systemd cadre-desktops.service)
//   node src/main.ts stop         KILL SWITCH: write the STOP flag and pause every house desktop on the
//                                 E2B account now (works with the controller down; billing stops)
//   node src/main.ts destroy      stop, then delete every house desktop sandbox (wallet profiles are lost)
//   node src/main.ts list         the house desktop sandboxes E2B lists (state, age), no secrets
//   node src/main.ts resume       remove the STOP flag (the controller starts the desktops again)
// Admin flag: the file DESK_KILL_FILE (default /var/lib/lode/desktops/STOP) or DESK_ENABLED=0.
import { addCx } from "../../../../apps/explorer/src/cxjson.ts";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, resolve } from "node:path";
import { CadreApi, Gateway, latestBlockhash } from "../../src/clients.ts";
import { nodeFromEnv } from "../../src/townnode.ts";
import { configFromEnv } from "../../src/main.ts";
import { loadWallet } from "../../src/wallet.ts";
import { deskConfig, loadSecrets } from "./config.ts";
import { Controller } from "./controller.ts";
import { type DesktopHost, META_KEY, type Role, sdkApi } from "./desk.ts";
import { selfHostApi, selfHostOptsFromEnv } from "./selfhost.ts";
import { checkStreamToken } from "./token.ts";
import { openRouterChat, openRouterKeyInfo } from "./vision.ts";
import { Cadre } from "../../trading/src/cadre.ts";
import { MarketOps } from "./market.ts";
import { guardedFetch, htmlToText } from "../../agents/src/toolbelt/netguard.ts";
import { fileURLToPath } from "node:url";

const env = process.env;
const num = (k: string, d: number) => (env[k] === undefined || env[k] === "" ? d : Number(env[k]));
const cfg = deskConfig();
const sec = loadSecrets();
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function e2b() {
  if (!sec.e2bKey) throw new Error("no E2B_API_KEY (E2B_ENV_FILE in CADRE_AGENTS_ENV)");
  return sdkApi(sec.e2bKey);
}

const selfHandles = (env.DESK_SELFHOST_AGENTS ?? "").split(",").map((s) => s.trim()).filter(Boolean);
/** The self-hosted host, when any agent runs there (DESK_SELFHOST_AGENTS) or DESK_SELFHOST=1. */
const selfhost = (): DesktopHost | null => (selfHandles.length || env.DESK_SELFHOST === "1" ? selfHostApi(selfHostOptsFromEnv()) : null);

async function stopAll(destroy: boolean) {
  mkdirSync(dirname(cfg.killFile), { recursive: true });
  writeFileSync(cfg.killFile, `stopped ${new Date().toISOString()}\n`);
  let n = 0;
  for (const api of [await e2b(), selfhost()].filter(Boolean) as DesktopHost[]) {
    const boxes = await api.list({});
    for (const b of boxes) {
      if (!b.metadata?.[META_KEY]) continue;
      if (destroy) await api.kill(b.sandboxId);
      else if (b.state === "running") await api.pause(b.sandboxId);
      n++;
    }
  }
  console.log(JSON.stringify({ event: destroy ? "destroyed" : "stopped", desktops: n, flag: cfg.killFile }));
}

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");

/** The work loop's outside world: the market (signed with each agent's own key), cadre-api on the box,
 * outside pages through the toolbelt's network guard, Solana devnet balances, the practice runner's state. */
async function workDeps() {
  const apiBase = cfg.apiUrl.replace(/\/$/, "");
  const api = async (path: string) => {
    const r = await fetch(apiBase + path, { signal: AbortSignal.timeout(20_000) });
    const j = await r.json().catch(() => null);
    if (!r.ok) throw new Error(`cadre-api ${path}: HTTP ${r.status}${j?.error?.message ? ` ${String(j.error.message).slice(0, 80)}` : ""}`);
    return j;
  };
  const marketFile = env.DESK_MARKET_FILE ?? resolve(REPO, "deploy/market-devnet.json");
  let market: MarketOps | null = null;
  if (existsSync(marketFile)) {
    const contract = String(JSON.parse(readFileSync(marketFile, "utf8")).market ?? "");
    if (contract) {
      const c = new Cadre(env.CHAIN_RPC ?? "http://127.0.0.1:10630");
      await c.init().catch(() => {});
      market = new MarketOps(c as any, contract, resolve(cfg.stateDir, "market-blobs"), (e) => console.log(JSON.stringify(addCx({ t: new Date().toISOString(), ...e }))));
    }
  }
  const UA = { "user-agent": "Mozilla/5.0 (X11; Linux x86_64) CadreHouseAgent/1.0 (+https://cadre.64-95-12-116.sslip.io)", accept: "text/html,application/xhtml+xml,application/rss+xml,application/xml;q=0.9,*/*;q=0.8" };
  const fetchRaw = async (url: string) => {
    const r = await guardedFetch(url, { headers: UA, maxBytes: 2_000_000, timeoutMs: 20_000 });
    if (r.status >= 400) throw new Error(`HTTP ${r.status}`);
    return r.body.toString("utf8");
  };
  const fetchPage = async (url: string) => {
    const r = await guardedFetch(url, { headers: UA, maxBytes: 2_000_000, timeoutMs: 20_000 });
    const body = r.body.toString("utf8");
    if (/html/i.test(r.contentType) || /^\s*</.test(body)) { const t = htmlToText(body, r.url); return { status: r.status, url: r.url, title: t.title, text: t.text }; }
    return { status: r.status, url: r.url, title: "", text: body };
  };
  const solRpc = env.SOLANA_RPC;
  const solBalances = solRpc ? async (addrs: string[]) => {
    const out: (number | null)[] = [];
    for (let i = 0; i < addrs.length; i += 100) {
      const r = await fetch(solRpc, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getMultipleAccounts", params: [addrs.slice(i, i + 100), { encoding: "base64", dataSlice: { offset: 0, length: 0 } }] }), signal: AbortSignal.timeout(20_000) });
      const j: any = await r.json();
      for (const v of j?.result?.value ?? addrs.slice(i, i + 100).map(() => undefined)) out.push(v === undefined ? null : v === null ? 0 : Number(v.lamports) / 1e9);
    }
    return out;
  } : undefined;
  const practiceDir = env.DESK_PRACTICE_DIR ?? "/var/lib/lode/practice";
  const practice = (handle: string) => { try { const f = resolve(practiceDir, `${handle}.json`); return existsSync(f) ? JSON.parse(readFileSync(f, "utf8")) : null; } catch { return null; } };
  const w = (k: string) => (env[k] ? Number(env[k]) : undefined);
  const wcfg = Object.fromEntries(Object.entries({ monitorS: w("WORK_MONITOR_S"), qaS: w("WORK_QA_S"), flowS: w("WORK_FLOW_S"), newsHireS: w("WORK_NEWS_HIRE_S"), opsHireS: w("WORK_OPS_HIRE_S"), digestS: w("WORK_DIGEST_S"), managerCapCadre: w("WORK_MANAGER_CAP_CADRE"), fundCapCadre: w("WORK_FUND_CAP_CADRE"), researchMaxUsd: w("WORK_RESEARCH_MAX_USD"), flowMaxUsd: w("WORK_FLOW_MAX_USD") }).filter(([, v]) => v !== undefined && Number.isFinite(v)));
  return { market, api, fetchPage, fetchRaw, solBalances, practice, publicApi: env.DESK_PUBLIC_API ?? "https://api.64-95-12-116.sslip.io", cfg: wcfg };
}

async function main() {
  const cmd = process.argv[2];
  if (cmd === "stop") return stopAll(false);
  if (cmd === "destroy") return stopAll(true);
  if (cmd === "resume") { rmSync(cfg.killFile, { force: true }); console.log(JSON.stringify({ event: "resumed", flag: cfg.killFile })); return; }
  if (cmd === "list") {
    const out: unknown[] = [];
    for (const api of [await e2b(), selfhost()].filter(Boolean) as DesktopHost[]) {
      const all = (await api.list({})).filter((b) => b.metadata?.[META_KEY]);
      out.push(...all.map((b) => ({ agent: b.metadata[META_KEY], role: b.metadata.cadre_role, host: api.kind, state: b.state, age_h: Number(((Date.now() - b.startedAt.getTime()) / 3.6e6).toFixed(2)) })));
    }
    console.log(JSON.stringify(out, null, 1));
    return;
  }
  if (cmd !== "run") throw new Error("usage: run | stop | destroy | resume | list");
  if (env.DESK_ENABLED === "0") { console.log(JSON.stringify({ event: "disabled", why: "DESK_ENABLED=0" })); return; }
  if (!sec.openrouterKey) throw new Error("no OPENROUTER_API_KEY (PROVIDERS_ENV_FILE in CADRE_AGENTS_ENV)");
  const streamSecret = existsSync(cfg.streamSecretFile) ? readFileSync(cfg.streamSecretFile) : null;
  const activityTokenFile = env.DESK_ACTIVITY_TOKEN_FILE ?? "/etc/lode/keys/desktops-activity.token";
  const activityToken = existsSync(activityTokenFile) ? readFileSync(activityTokenFile, "utf8").trim() : null;
  const roles = (env.DESK_ROLES ?? "social,work").split(",").map((s) => s.trim()) as Role[];
  const swarmCfg = { ...configFromEnv(), stateFile: resolve(cfg.stateDir, "swarm-state.json"), killFile: cfg.killFile };
  const operator = loadWallet(resolve(cfg.keysDir, "operator.json"));
  const deps = {
    // town + DMs on Solana when TOWN_PROGRAM is set (services/swarm/src/townnode.ts); the operator funds its agents' SOL
    node: nodeFromEnv(env.CHAIN_RPC ?? "http://127.0.0.1:10630", env, { funder: operator, api: cfg.apiUrl }),
    gateway: new Gateway(env.GATEWAY_URL ?? "http://127.0.0.1:10664", operator),
    api: new CadreApi(cfg.apiUrl),
    latestBlockhash,
    now: () => Date.now(),
    rand: Math.random,
    log: (e: Record<string, unknown>) => console.log(JSON.stringify(e)),
  };
  const work = env.DESK_WORK === "0" ? undefined : await workDeps();
  const ctl = new Controller(cfg, swarmCfg, deps, operator, await e2b(), openRouterChat(sec.openrouterKey), {
    work,
    town,
    roles,
    agentsN: num("DESK_AGENTS", 1),
    parallel: num("DESK_PARALLEL", 3),
    browseGapS: num("DESK_BROWSE_GAP_S", 1200),
    workGapS: num("DESK_WORK_GAP_S", 1200),
    tipShare: num("DESK_TIP_SHARE", 0.3),
    tipCadre: env.DESK_TIP_CADRE ?? "0.001",
    tipUnits: BigInt(env.DESK_TIP_UNITS ?? "1000000"),
    tipsPerDay: num("DESK_TIPS_PER_DAY", 3),
    walletTgz: env.DESK_WALLET_TGZ ?? resolve(cfg.stateDir, "wallet.tgz"),
    passwordFor: () => randomBytes(18).toString("base64url"),
    // Phase A (docs/DESKTOPS-AT-SCALE.md): the first DESK_TWO_DESK_AGENTS keep Social + Work, the rest one desktop
    twoDeskAgents: env.DESK_TWO_DESK_AGENTS ? num("DESK_TWO_DESK_AGENTS", 0) : undefined,
    soloGapS: num("DESK_SOLO_GAP_S", num("DESK_BROWSE_GAP_S", 1200)),
    modeSlotS: num("DESK_MODE_SLOT_S", 1800),
    selfhost: selfhost() ?? undefined,
    selfHostHandles: selfHandles,
    healthS: num("DESK_HEALTH_S", 300),
    memLowMb: num("DESK_MEM_LOW_MB", 80),
    // the vision model's credit gate (src/credits.ts): pause on a 402, probe, resume on credit
    credit: {
      probeS: num("DESK_CREDIT_PROBE_S", 300),
      maxProbeS: num("DESK_CREDIT_PROBE_MAX_S", 3600),
      flapS: num("DESK_CREDIT_FLAP_S", 600),
      holdDesksS: num("DESK_CREDIT_HOLD_DESKS_S", 1800),
      remindS: num("DESK_CREDIT_REMIND_S", 1800),
      minUsd: num("DESK_CREDIT_MIN_USD", 0.01),
      probeMaxTokens: num("DESK_CREDIT_PROBE_MAX_TOKENS", 1600),
    },
    keyInfo: openRouterKeyInfo(sec.openrouterKey),
  });
  // the old town behaviour (posts, replies, reactions typed on desktop 1) only with DESK_TOWN=1; DESK_SOCIAL
  // is no longer read (owner 2026-10-23: no Social desktops, no town activity)
  const town = env.DESK_TOWN === "1";

  // ---- HTTP (127.0.0.1 only; cadre-api is the public face and mints stream tokens)
  const cache = new Map<string, { at: number; body: Uint8Array }>();
  // cache lifetimes follow the encoder: a playlist at most 40% of a segment old, a still 90% of its interval
  const segS = Number(cfg.stream.DESK_SEG_S ?? 2), stillS = Number(cfg.stream.DESK_STILL_S ?? 2);
  const listMs = Math.min(1000, segS * 400), thumbMs = Math.min(2000, stillS * 900);
  const prefetching = new Set<string>();
  const send = (res: any, code: number, body: unknown, type = "application/json") => {
    res.writeHead(code, { "content-type": type, "cache-control": "no-store" });
    res.end(type === "application/json" ? JSON.stringify(addCx(body)) : body);
  };
  createServer(async (req, res) => {
    try {
      const u = new URL(req.url ?? "/", "http://x");
      const p = u.pathname.split("/").filter(Boolean);
      if (u.pathname === "/health") return send(res, 200, { ok: !ctl.killed(), capped: ctl.capped, vision_paused: ctl.credits.paused, desktops_held: ctl.credits.desksHeld, desktops_running: ctl.runningDesks().length, ...ctl.healthSummary() });
      if (u.pathname === "/status") return send(res, 200, ctl.status());
      if (u.pathname === "/activity" && req.method === "POST") {
        // runtime events for an agent's log and activity console (the trading executor's trades, its txs);
        // 127.0.0.1 only, and the DESK_ACTIVITY_TOKEN_FILE token when that file exists
        if (activityToken && req.headers["x-cadre-activity"] !== activityToken) return send(res, 403, { error: "activity token missing or wrong" });
        const chunks: Buffer[] = [];
        let size = 0;
        for await (const c of req) { size += c.length; if (size > 16_384) return send(res, 413, { error: "body over 16 KiB" }); chunks.push(c); }
        let b: any;
        try { b = JSON.parse(Buffer.concat(chunks).toString("utf8")); } catch { return send(res, 400, { error: "body is not JSON" }); }
        const items = Array.isArray(b) ? b : [b];
        let n = 0;
        for (const x of items.slice(0, 50)) {
          const kind = String(x?.kind ?? "");
          if (!/^(trade|tx|thought|note|order|quote)$/.test(kind) || typeof x?.agent !== "string" || typeof x?.text !== "string") continue;
          const tx = typeof x.tx === "string" && /^[0-9A-Za-z]{8,128}$/.test(x.tx) ? x.tx : undefined;
          if (ctl.activity(x.agent, { kind, text: x.text.replace(/[\u0000-\u001f]/g, " ").slice(0, 240), tx, ref: typeof x.ref === "string" ? x.ref.slice(0, 80) : undefined })) n++;
        }
        return send(res, 200, { accepted: n });
      }
      if (p[0] === "agents" && p[1] && p[2] === "reports" && p[3] && p.length === 4) {
        const r = ctl.store.get(p[1], p[3]);
        if (!r) return send(res, 404, { error: "no such report" });
        if (u.searchParams.get("format") === "md") return send(res, 200, r.body_md, "text/markdown; charset=utf-8");
        if (u.searchParams.get("format") === "svg" && r.chart_svg) return send(res, 200, r.chart_svg, "image/svg+xml");
        return send(res, 200, r);
      }
      if (p[0] === "agents" && p[1] && p[2] === "reports" && p.length === 3) {
        if (!ctl.byHandle.has(p[1])) return send(res, 404, { error: "no such desktop agent" });
        return send(res, 200, { agent: p[1], reports: ctl.store.list(p[1]).map((r) => ({ id: r.id, kind: r.kind, title: r.title, created: r.created, summary: r.summary, sha256: r.sha256 })) });
      }
      if (p[0] === "agents" && p[1] && p.length === 2) { const d = ctl.detail(p[1]); return d ? send(res, 200, d) : send(res, 404, { error: "no such desktop agent" }); }
      if (p[0] === "stream" && p.length === 4) {
        const [, h, role, file] = p as [string, string, Role, string];
        const tok = u.searchParams.get("t") ?? "";
        if (!streamSecret || !checkStreamToken(streamSecret, h, role, tok)) return send(res, 403, { error: "stream token missing, wrong or expired" });
        const a = ctl.byHandle.get(h);
        const d = a?.desks[role];
        if (!d || d.state !== "running") return send(res, 410, { error: "desktop not running" });
        const key = `${h}/${role}/${file}`;
        const isList = file === "live.m3u8";
        const isThumb = file === "thumb.jpg";
        const hit = cache.get(key);
        let body = hit && (isList ? Date.now() - hit.at < listMs : isThumb ? Date.now() - hit.at < thumbMs : true) ? hit.body : null;
        if (!body) {
          body = await d.readStream(file);
          if (!body) return send(res, 404, { error: "no such segment" });
          cache.set(key, { at: Date.now(), body });
          if (cache.size > 400) for (const k of [...cache.keys()].slice(0, 100)) cache.delete(k);
        }
        if (isList) {
          // someone is watching: copy the listed segments out now, so the player's next request is a cache hit
          for (const seg of Buffer.from(body).toString("utf8").split("\n").filter((l) => /^s\d{6}\.ts$/.test(l))) {
            const k = `${h}/${role}/${seg}`;
            if (cache.has(k) || prefetching.has(k)) continue;
            prefetching.add(k);
            void d.readStream(seg).then((b) => { if (b) cache.set(k, { at: Date.now(), body: b }); }).finally(() => prefetching.delete(k));
          }
          const text = Buffer.from(body).toString("utf8").split("\n").map((l) => (l && !l.startsWith("#") ? `${l}?t=${encodeURIComponent(tok)}` : l)).join("\n");
          res.writeHead(200, { "content-type": "application/vnd.apple.mpegurl", "cache-control": "no-store" });
          return res.end(text);
        }
        if (isThumb) {
          res.writeHead(200, { "content-type": "image/jpeg", "cache-control": "no-store" });
          return res.end(body);
        }
        res.writeHead(200, { "content-type": "video/mp2t", "cache-control": "max-age=60" });
        return res.end(body);
      }
      return send(res, 404, { error: "not found" });
    } catch (e: any) {
      return send(res, 500, { error: String(e?.message ?? e).slice(0, 200) });
    }
  }).listen(cfg.port, "127.0.0.1");

  let stop = false;
  const onSig = () => { stop = true; };
  process.on("SIGTERM", onSig);
  process.on("SIGINT", onSig);
  ctl.event({ event: "start", agents: ctl.agents.map((a) => a.persona.handle), roles, layout: ctl.agents.map((a) => `${a.persona.handle}:${a.roles.join("+")}${a.desks.social?.api.kind === "selfhost" ? "@selfhost" : ""}`), cap_usd_per_day: cfg.usdPerDay, vision_model: cfg.visionModel, e2b_usd_per_desktop_hour: Number((ctl.usdPerS * 3600).toFixed(4)), town, work: !!work, market: work?.market?.contract ?? null, port: cfg.port });

  const loops = [
    (async () => { // desktops, keepalive and the spend ledger: every minute
      while (!stop) {
        if (ctl.killed()) { await ctl.stopAll(false); break; }
        await ctl.spendTick();
        await ctl.creditTick().catch((e) => ctl.event({ event: "credit_error", error: String(e?.message ?? e).slice(0, 200) }));
        if (!ctl.holdDesks()) await ctl.ensureAll();
        await ctl.keepalive();
        for (let i = 0; i < 60 && !stop && !ctl.killed(); i++) await sleep(1000);
      }
    })(),
    (async () => { // profiles (always) and, only with DESK_TOWN=1, the swarm's town actions on desktop 1
      await sleep(30_000);
      while (!stop && !ctl.killed()) {
        if (town) { if (!ctl.holdDesks() && ctl.runningDesks().length) await ctl.swarm.tick(); await sleep(num("SWARM_TICK_MS", 20_000)); continue; }
        // a profile whose job text changed is published again (operator-written bio, no thought)
        await ctl.swarm.bootstrap().catch((e) => ctl.event({ event: "profile_error", error: String(e?.message ?? e).slice(0, 200) }));
        await sleep(ctl.agents.every((a) => (ctl.swarm.st.agents[a.persona.handle] as any)?.profileDone) ? 600_000 : 20_000);
      }
    })(),
    (async () => { // per-desktop health: stream, browser host, memory, last action
      await sleep(60_000);
      while (!stop && !ctl.killed()) {
        if (!ctl.capped) await ctl.healthTick().catch((e) => ctl.event({ event: "health_error", error: String(e?.message ?? e).slice(0, 200) }));
        for (let i = 0; i < num("DESK_HEALTH_S", 300) && !stop && !ctl.killed(); i++) await sleep(1000);
      }
    })(),
    (async () => { // the work loop (src/work.ts)
      await sleep(45_000);
      while (!stop && !ctl.killed()) {
        await ctl.background();
        await sleep(15_000);
      }
    })(),
  ];
  await Promise.race([Promise.all(loops), new Promise<void>((r) => { const t = setInterval(() => { if (stop || ctl.killed()) { clearInterval(t); r(); } }, 1000); })]);
  await ctl.spendTick();
  if (ctl.killed()) await ctl.stopAll(false);
  ctl.event({ event: "stop", reason: ctl.killed() ? "kill switch" : "signal", spend_today: ctl.ledger.spent });
  process.exit(0);
}

main().catch((e) => {
  console.error(JSON.stringify({ event: "fatal", error: String(e?.message ?? e).slice(0, 300) }));
  process.exit(1);
});
