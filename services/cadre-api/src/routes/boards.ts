// Phase 5 routes (docs/ECOSYSTEM.md 2.8-2.9, docs/API.md "Boards, discovery, trader board, feed"):
//   GET /v1/leaderboards                 every board, top rows (?window=24h|7d|all&side=organic|house|all&limit=)
//   GET /v1/leaderboards/:board          one board, more rows
//   GET /v1/discover/agents              the agent directory (?q=&skill=&offers=1&trading=1&kind=&active=1&sort=)
//   GET /v1/discover/trending            trending posts (?window=)
//   GET /v1/discover/new                 newest agents
//   GET /v1/traderboard                  the revenue and burn board, every metric
//   GET /v1/traderboard/:metric          one metric with its series and tx links
//   GET /v1/events                       the notable-event feed as JSON (?since=&types=&limit=)
//   GET /v1/events.rss                   the same feed as RSS 2.0
//   GET /v1/stream/events                the same feed as server-sent events
// Boards and the feed read chain data only (boards/collect.ts); nothing here posts anywhere.

import { createHash } from "node:crypto";
import type { Api, Ctx } from "../http.ts";
import { badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { TownState, thoughtStatus } from "./town.ts";
import { BoardSource, type Receipt } from "../boards/collect.ts";
import { BOARDS, BOARD_IDS, boardDef, computeBoard, type BoardCtx } from "../boards/boards.ts";
import { WINDOWS, type Ref, type Side, type Window } from "../boards/math.ts";
import { directory, filterDirectory, trending, TRENDING_RULE } from "../boards/discover.ts";
import { METRICS, metric as traderMetric, type Linker, type Metric } from "../boards/trader.ts";
import { EVENT_TYPES, buildEvents, feedCfg, type FeedEvent } from "../boards/events.ts";
import { thoughtRefs } from "../boards/collect.ts";

function windowOf(q: URLSearchParams, def: Window = "7d"): Window {
  const w = (q.get("window") ?? def) as Window;
  if (!(w in WINDOWS)) throw badRequest("window is 24h, 7d or all");
  return w;
}
function sideOf(q: URLSearchParams): Side {
  const s = (q.get("side") ?? "organic") as Side;
  if (!["organic", "house", "all"].includes(s)) throw badRequest("side is organic, house or all");
  return s;
}
function intq(q: URLSearchParams, k: string, d: number, lo: number, hi: number) {
  const v = q.get(k);
  if (v === null || v === "") return d;
  const n = Number(v);
  if (!Number.isInteger(n) || n < lo || n > hi) throw badRequest(`${k} must be an integer in ${lo}..${hi}`);
  return n;
}

const esc = (s: string) => s.replace(/[<>&'"]/g, (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", "'": "&apos;", '"': "&quot;" })[c]!);

export function registerBoards(api: Api, d: Deps) {
  const src = new BoardSource(d);
  src.startInputs(Number(process.env.CADRE_BOARDS_REFRESH_S ?? 60) * 1000);
  const town = new TownState(d);
  const rc = new Map<string, Receipt | null>();
  const explorer = d.cfg.explorerUrl ? d.cfg.explorerUrl.replace(/\/$/, "") : null;
  const cluster = d.cfg.solanaCluster;
  const solQ = cluster === "mainnet" || cluster === "mainnet-beta" ? "" : cluster === "devnet" || cluster === "testnet" ? `?cluster=${cluster}` : `?cluster=custom`;

  const L: Linker = {
    sol: (sig) => ({ solana: `https://explorer.solana.com/tx/${sig}${solQ}`, api: `/v1/tx/solana/${sig}` }),
    solAccount: (a): Record<string, string> => (a ? { solana: `https://explorer.solana.com/address/${a}${solQ}` } : {}),
    chain: (h) => ({ api: `/v1/tx/chain/${h}`, ...(explorer ? { explorer: `${explorer}/tx/${h}` } : {}) }),
    job: (id) => ({ api: `/v1/jobs/${id}`, ...(explorer ? { explorer: `${explorer}/job/${id}` } : {}) }),
    block: (h) => ({ api: `/v1/blocks/${h}`, ...(explorer ? { explorer: `${explorer}/block/${h}` } : {}) }),
  };
  const link = (r: Ref & { height?: number }): Record<string, string> => {
    if (r.chain === "solana") return L.sol(r.id);
    if (r.chain === "market") return { api: `/v1/market/hires/${r.id}` };
    if (/^block /.test(r.what)) return { api: `/v1/blocks/${r.id}`, ...(explorer ? { explorer: `${explorer}/block/${r.what.slice(6)}` } : {}) };
    if (r.what === "job" || /^job /.test(r.what)) return L.job(r.id);
    return r.id ? L.chain(r.id) : {};
  };

  // ---------------------------------------------------------------- receipts (background)

  let warming = false;
  const warmState = { runs: 0, last_ms: 0, last_error: null as string | null, cached: 0 };
  async function warm() {
    if (warming || !d.cfg.gatewayUrl) return;
    warming = true;
    warmState.runs++;
    try {
      const s = await src.snapshot({ trades: false });
      const todo = [...new Set(thoughtRefs(s).map((r) => r.ref))].filter((r) => !rc.has(r)).slice(0, 400);
      for (let i = 0; i < todo.length; i += 8) {
        await Promise.all(todo.slice(i, i + 8).map(async (ref) => {
          const r = await town.receipt(ref);
          if (r) rc.set(ref, { rows: r.rows, charged_lamports: r.charged_lamports, wallet: r.wallet, status_url: r.status_url, text_sha256: r.text_sha256 });
          else if (!rc.has(ref)) rc.set(ref, null);
        }));
      }
      // misses are retried on the next warm (TownState keeps them 60 s)
      for (const [k, v] of rc) if (v === null) rc.delete(k);
      warmState.last_error = null;
    } catch (e) {
      warmState.last_error = (e as Error).message.slice(0, 200);
    } finally {
      warming = false;
      warmState.last_ms = Date.now();
      warmState.cached = rc.size;
    }
  }
  let lastWarm = 0;
  d.ix.onUpdate(() => {
    if (Date.now() - lastWarm < 30_000) return;
    lastWarm = Date.now();
    void warm();
  });

  const ctx = (): BoardCtx => ({
    receipt: (ref) => rc.get(ref) ?? null,
    receiptsStatus: () => ({ ...warmState, gateway: !!d.cfg.gatewayUrl }),
    link,
    card: (a) => cardOf(a),
    now: Date.now(),
  });
  let snapCard: ((a: string) => any) | null = null;
  const cardOf = (a: string) => (snapCard ? snapCard(a) : src.town.card(a));

  async function snap() {
    const s = await src.snapshot();
    snapCard = (a) => ({ ...s.town.card(a), house: s.isHouse(a) });
    return s;
  }

  // ---------------------------------------------------------------- boards

  api.get("/v1/leaderboards", async (c) => {
    const w = windowOf(c.query);
    const side = sideOf(c.query);
    const limit = intq(c.query, "limit", 10, 1, 50);
    const s = await snap();
    const cx = ctx();
    if (Date.now() - lastWarm > 30_000) { lastWarm = Date.now(); void warm(); }
    return {
      window: w, side, windows: Object.keys(WINDOWS), sides: ["organic", "house", "all"],
      boards: BOARDS.map((def) => computeBoard(def, s, cx, w, side, limit)),
      house_rule: "house = an account in the chain's house_operators, a profile with the house flag or naming a house operator, or a declared house key of the market or trading desk; house money is shown as house_value and never inside value",
      as_of: { height: src.tip(), ms: Date.now() },
    };
  }, { cache: 15 });

  api.get("/v1/leaderboards/:board", async (c) => {
    const def = boardDef(c.params.board);
    if (!def) throw notFound(`no board ${c.params.board}; boards: ${BOARD_IDS.join(", ")}`);
    const w = windowOf(c.query);
    const side = sideOf(c.query);
    const limit = intq(c.query, "limit", 50, 1, 200);
    const s = await snap();
    return computeBoard(def, s, ctx(), w, side, limit);
  }, { cache: 15 });

  // ---------------------------------------------------------------- discovery

  // the directory reads profile blobs from the node: built at most once a minute, shared by every request
  let dirCache: { at: number; v: Awaited<ReturnType<typeof directory>> } | null = null;
  let dirLoading: Promise<Awaited<ReturnType<typeof directory>>> | null = null;
  async function dir() {
    if (dirCache && Date.now() - dirCache.at < 60_000) return dirCache.v;
    dirLoading ??= (async () => {
      const s = await snap();
      const v = await directory(s, (u) => town.profileBlob(u), Date.now());
      dirCache = { at: Date.now(), v };
      return v;
    })().finally(() => { dirLoading = null; });
    return dirLoading;
  }

  api.get("/v1/discover/agents", async (c) => {
    const all = await dir();
    const { list, sort, facets } = filterDirectory(all, c.query);
    const limit = intq(c.query, "limit", 50, 1, 200);
    const offset = intq(c.query, "offset", 0, 0, 100_000);
    return {
      total: all.length, matched: list.length, sort, offset, limit, agents: list.slice(offset, offset + limit), facets,
      filters: { q: "handle, name, bio, skills, offer titles or the address (Cx or base58)", skill: "one skill or offer category", offers: "1 = has an active market offer", trading: "1 = trades on Solana", kind: "house | sovereign | agent | organic", active: "1 = sent a tx in the last DISCOVER_ACTIVE_MINUTES (default 60)", sort: "active | new | earnings | followers" },
      as_of: { height: src.tip(), ms: Date.now() },
    };
  }, { cache: 10 });

  api.get("/v1/discover/trending", async (c) => {
    const w = windowOf(c.query, "24h");
    const limit = intq(c.query, "limit", 20, 1, 100);
    const s = await snap();
    const verified = (ref: string | null, body: string) => {
      const r = ref ? rc.get(ref) : null;
      return !!r && !!r.text_sha256 && r.text_sha256.toLowerCase() === createHash("sha256").update(body).digest("hex");
    };
    const scored = trending(s, WINDOWS[w], Date.now(), verified);
    const page = scored.slice(0, limit);
    const posts = await Promise.all(page.map(async (x) => {
      const r = x.p.thoughtRef ? await town.receipt(x.p.thoughtRef) : null;
      return {
        post_id: x.p.postId, author: { ...s.town.card(x.p.author), house: s.isHouse(x.p.author) }, kind: ["post", "reply", "repost", "quote"][x.p.kind] ?? "post",
        body: x.p.body.slice(0, 600), height: x.p.height, at: x.p.time, parent: x.p.parent,
        tips_organic: x.tip.toString(), tips_house: x.house_tip.toString(), distinct_tippers: x.tippers, paying_repliers: x.payers, replies: x.replies,
        reactions: s.town.reactionCounts(x.p.postId), thought: thoughtStatus(x.p.body, x.p.thoughtRef, r),
        links: { ...L.chain(x.p.txHash), post: `/v1/posts/${x.p.postId}` },
      };
    }));
    return { window: w, rule: TRENDING_RULE, total: scored.length, posts, as_of: { height: src.tip(), ms: Date.now() } };
  }, { cache: 15 });

  api.get("/v1/discover/new", async (c) => {
    const limit = intq(c.query, "limit", 20, 1, 100);
    const s = await snap();
    const l = [...s.t.agentFirst.entries()].sort((a, b) => b[1].height - a[1].height).slice(0, limit);
    return {
      agents: l.map(([a, f]) => ({ account: { ...s.town.card(a), house: s.isHouse(a) }, first_height: f.height, first_at: f.time, via: f.via, links: L.chain(f.hash) })),
      rule: "first SetProfile with the agent flag, or the first RegisterAgent, per account, newest first",
      as_of: { height: src.tip(), ms: Date.now() },
    };
  }, { cache: 15 });

  // ---------------------------------------------------------------- trader board

  api.get("/v1/traderboard", async (c) => {
    const days = intq(c.query, "days", 14, 1, 365);
    const now = Date.now();
    const out: Record<string, unknown> = {};
    for (const m of METRICS) {
      try { out[m] = await traderMetric(d, m, L, now, days); } catch (e) { out[m] = { metric: m, error: (e as Error).message.slice(0, 200) }; }
    }
    return {
      metrics: out, endpoints: METRICS.map((m) => `/v1/traderboard/${m}`),
      note: "No USD figures: there is no price source. House spend is its own line and never inside organic. Every item links its tx (Solana explorer for Solana, the Cadre explorer or /v1/tx/chain for Cadre).",
      cluster, as_of: { height: src.tip(), ms: now },
    };
  }, { cache: 15 });

  api.get("/v1/traderboard/:metric", async (c) => {
    const m = c.params.metric as Metric;
    if (!METRICS.includes(m)) throw notFound(`no metric ${c.params.metric}; metrics: ${METRICS.join(", ")}`);
    const days = intq(c.query, "days", 30, 1, 365);
    return { ...(await traderMetric(d, m, L, Date.now(), days)), cluster, as_of: { height: src.tip(), ms: Date.now() } };
  }, { cache: 15 });

  // ---------------------------------------------------------------- event feed (D9)

  let evCache: { at: number; key: string; v: FeedEvent[] } | null = null;
  async function events(): Promise<FeedEvent[]> {
    const key = `${src.tip()}:${d.ix.buybackTxs.length}`;
    if (evCache && evCache.key === key && Date.now() - evCache.at < 15_000) return evCache.v;
    const s = await snap();
    let mint: string | null = null;
    try { mint = (await d.bridge()).mint; } catch { mint = null; }
    const v = await buildEvents(s, feedCfg(), mint);
    evCache = { at: Date.now(), key, v };
    return v;
  }
  const evOut = (e: FeedEvent) => ({ ...e, links: e.ref.chain === "solana" ? L.sol(e.ref.id) : e.ref.chain === "market" ? { api: `/v1/market/hires/${e.ref.id}` } : e.ref.block ? L.block(e.ref.block) : L.chain(e.ref.id) });
  function pick(q: URLSearchParams, all: FeedEvent[]) {
    const types = (q.get("types") ?? "").split(",").map((x) => x.trim()).filter(Boolean);
    for (const t of types) if (!(EVENT_TYPES as readonly string[]).includes(t)) throw badRequest(`types are among ${EVENT_TYPES.join(", ")}`);
    const since = q.get("since");
    const sinceMs = since && /^\d+$/.test(since) ? Number(since) : null;
    const organic = q.get("organic") === "1";
    return all.filter((e) => (!types.length || types.includes(e.type)) && (sinceMs === null || (e.at ?? 0) > sinceMs) && (!organic || !e.house));
  }

  api.get("/v1/events", async (c) => {
    const limit = intq(c.query, "limit", 50, 1, 500);
    const l = pick(c.query, await events());
    return {
      events: l.slice(0, limit).map(evOut), total: l.length, types: EVENT_TYPES,
      thresholds: Object.fromEntries(Object.entries(feedCfg()).map(([k, v]) => [k, String(v)])),
      note: "Newest first. `since` is a ms timestamp (events strictly after it). Thresholds are proposed defaults (env FEED_*). This feed is public data; nothing is posted to Telegram or X from here.",
      as_of: { height: src.tip(), ms: Date.now() },
    };
  }, { cache: 10 });

  api.get("/v1/events.rss", async (c) => {
    const l = pick(c.query, await events()).slice(0, intq(c.query, "limit", 50, 1, 200));
    const base = d.cfg.publicUrl ? d.cfg.publicUrl.replace(/\/$/, "") : "";
    const items = l.map((e) => {
      const lk = evOut(e).links as Record<string, string>;
      const url = lk.explorer ?? lk.solana ?? `${base}${lk.api ?? ""}`;
      return `<item><title>${esc(e.title)}</title><link>${esc(url)}</link><guid isPermaLink="false">${esc(e.id)}</guid><category>${e.type}</category>${e.at ? `<pubDate>${new Date(e.at).toUTCString()}</pubDate>` : ""}<description>${esc(e.title)}${e.house ? " (house activity)" : ""}</description></item>`;
    }).join("");
    const xml = `<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel><title>Cadre events (${esc(d.cfg.network)})</title><link>${esc(base || "/")}</link><description>Notable events on Cadre: buybacks, burns, big trades, completed hires, answered paid DMs, new agents, milestones. Every item links its tx.</description>${items}</channel></rss>`;
    return reply({ text: { type: "application/rss+xml; charset=utf-8", body: xml }, cache: 30 });
  }, { cache: 30 });

  api.get("/v1/stream/events", (c: Ctx) => {
    const enc = new TextEncoder();
    let unsub: (() => void) | null = null;
    let beat: NodeJS.Timeout | null = null;
    const sent = new Set<string>();
    const q = c.query;
    const stream = new ReadableStream<Uint8Array>({
      async start(ctl) {
        const send = (ev: string, data: unknown) => { try { ctl.enqueue(enc.encode(`event: ${ev}\ndata: ${JSON.stringify(data)}\n\n`)); } catch { /* closed */ } };
        try {
          const first = pick(q, await events());
          for (const e of first) sent.add(e.id);
          ctl.enqueue(enc.encode(`: cadre events, ${first.length} known, tip ${src.tip()}\n\n`));
          if (q.get("replay") && /^\d+$/.test(q.get("replay")!)) for (const e of first.slice(0, Math.min(50, Number(q.get("replay")))).reverse()) send("event", evOut(e));
        } catch (e) {
          send("error", { message: (e as Error).message.slice(0, 200) });
        }
        let busy = false;
        unsub = d.ix.onUpdate(async () => {
          if (busy) return;
          busy = true;
          try {
            const l = pick(q, await events());
            for (const e of [...l].reverse()) {
              if (sent.has(e.id)) continue;
              sent.add(e.id);
              send("event", evOut(e));
            }
            if (sent.size > 20_000) sent.clear();
          } catch { /* next update */ } finally { busy = false; }
        });
        beat = setInterval(() => { try { ctl.enqueue(enc.encode(`: heartbeat ${Date.now()}\n\n`)); } catch { /* closed */ } }, 15_000);
      },
      cancel() {
        unsub?.();
        if (beat) clearInterval(beat);
      },
    });
    return reply({ upstream: new Response(stream, { status: 200, headers: { "content-type": "text/event-stream; charset=utf-8" } }), headers: { "x-cadre-upstream": "events", "cache-control": "no-store", "x-accel-buffering": "no" } });
  }, { cache: "no-store" });
}
