// Monitor jobs: each samples real devnet state every few minutes (cadre-api on the box, the Cadre
// node, Solana devnet, this controller's own ledger), keeps a 24 h series, and opens a finding when
// a rule's threshold is crossed (closed when it clears). Thresholds are house choices written here,
// each with its reason; every number in a report is a value read from the source it names.
import type { MonitorTarget } from "./roster.ts";

export type Sample = Record<string, number | null>;
export interface Point { t: number; s: Sample }
export interface Rule { id: string; what: string; check: (pts: Point[], now: number) => string | null }
export interface MonitorDef {
  title: string;
  /** what it reads (shown in the report) */
  sources: string[];
  /** the series charted in the report, and its unit */
  chart: { key: string; label: string; unit: string; threshold?: number };
  /** site page the desktop shows when a finding opens */
  page: (site: string) => string;
  rules: Rule[];
}

export interface MonitorCtx {
  api: (path: string) => Promise<any>;
  node: (method: string, params?: unknown) => Promise<any>;
  solBalances?: (addrs: string[]) => Promise<(number | null)[]>;
  houseAccounts?: () => { handle: string; account: string; needs: { cadre: number; sol: number } }[];
  self?: () => { spent: number; cap: number; e2b: number; vision: number; unhealthy: number; desktops: number; running: number; vision_paused: boolean; seconds_left: number; running_usd_per_s: number };
  now: () => number;
}

const num = (x: unknown): number | null => (x === null || x === undefined || x === "" || !Number.isFinite(Number(x)) ? null : Number(x));
const last = (pts: Point[]) => pts[pts.length - 1];
/** points whose time is within `ms` of the newest */
const within = (pts: Point[], ms: number) => { const l = last(pts); return l ? pts.filter((p) => l.t - p.t <= ms) : []; };
/** rate of a counter over the last `ms`, per minute (null without two points) */
function ratePerMin(pts: Point[], key: string, ms: number): number | null {
  const w = within(pts, ms).filter((p) => p.s[key] !== null && p.s[key] !== undefined);
  if (w.length < 2) return null;
  const a = w[0]!, b = w[w.length - 1]!;
  const dt = (b.t - a.t) / 60_000;
  return dt > 0 ? ((b.s[key] as number) - (a.s[key] as number)) / dt : null;
}
/** true when `key` held the condition at every point of the last `ms` (and the window is covered) */
function held(pts: Point[], key: string, ms: number, cond: (v: number) => boolean): boolean {
  const l = last(pts);
  if (!l) return false;
  const w = pts.filter((p) => l.t - p.t <= ms);
  if (!w.length || l.t - w[0]!.t < ms * 0.8) return false;
  return w.every((p) => p.s[key] !== null && p.s[key] !== undefined && cond(p.s[key] as number));
}

export const MONITORS: Record<MonitorTarget, MonitorDef> = {
  chain: {
    title: "Chain health",
    sources: ["cadre-api /v1/summary (chain height, finalized height, block counts)"],
    chart: { key: "block_time_s", label: "Average block time over the last 30 min", unit: "s", threshold: 45 },
    page: (site) => `${site}/explorer`,
    rules: [
      { id: "stall", what: "no new block for 5 minutes", check: (pts) => (held(pts, "height", 300_000, (v) => v === last(pts)!.s.height) ? `height stuck at ${last(pts)!.s.height} for 5 min or more` : null) },
      { id: "slow", what: "average block time above 45 s over 30 min (target 30 s)", check: (pts) => { const v = last(pts)?.s.block_time_s; return v !== null && v !== undefined && v > 45 ? `average block time ${v.toFixed(1)} s over the last 30 min` : null; } },
      { id: "finality", what: "finality lag above 90 blocks (finality depth is 60)", check: (pts) => { const v = last(pts)?.s.finality_lag; return v !== null && v !== undefined && v > 90 ? `finalized height is ${v} blocks behind the tip` : null; } },
    ],
  },
  bridge: {
    title: "Bridge and light client",
    sources: ["cadre-api /v1/summary (light_client tip and final, chain height)", "cadre-api /v1/bridge (escrow invariant)"],
    chart: { key: "lc_lag", label: "Light client tip behind the chain", unit: "blocks", threshold: 200 },
    page: (site) => `${site}/buy`,
    rules: [
      { id: "lc_lag", what: "light client more than 200 blocks behind the chain", check: (pts) => { const v = last(pts)?.s.lc_lag; return v !== null && v !== undefined && v > 200 ? `light client tip is ${v} blocks behind the chain (header-relay or light client stalled)` : null; } },
      { id: "lc_stuck", what: "light client tip not moving for 30 minutes", check: (pts) => (held(pts, "lc_tip", 1_800_000, (v) => v === last(pts)!.s.lc_tip) ? `light client tip stuck at ${last(pts)!.s.lc_tip} for 30 min or more` : null) },
      { id: "invariant", what: "bridge vault and chain escrow differ for 30 minutes", check: (pts) => (held(pts, "delta_spl", 1_800_000, (v) => v !== 0) ? `vault minus outside-escrow delta ${last(pts)!.s.delta_spl} base units for 30 min` : null) },
    ],
  },
  storefront: {
    title: "Storefront orders",
    sources: ["cadre-api /v1/summary (storefront orders by status and stage, SOL pending)"],
    chart: { key: "pending", label: "Pending storefront orders", unit: "orders" },
    page: (site) => `${site}/ai`,
    rules: [
      { id: "await_lc", what: "orders waiting on the light client for an hour", check: (pts) => (held(pts, "awaiting_light_client", 3_600_000, (v) => v > 0) ? `${last(pts)!.s.awaiting_light_client} paid orders have waited on the light client for 1 h or more` : null) },
      { id: "unknown", what: "orders whose job the index cannot find", check: (pts) => { const v = last(pts)?.s.job_unknown; return v ? `${v} pending orders with an unknown job` : null; } },
      { id: "growth", what: "pending orders grew by 10 or more in 6 hours with none finalized", check: (pts) => { const w = within(pts, 6 * 3_600_000); if (w.length < 2) return null; const a = w[0]!.s, b = last(pts)!.s; return a.pending !== null && b.pending !== null && a.finalized !== null && b.finalized !== null && b.pending! - a.pending! >= 10 && b.finalized === a.finalized ? `pending orders ${a.pending} -> ${b.pending} in 6 h, finalized unchanged at ${b.finalized}` : null; } },
    ],
  },
  mining: {
    title: "Mining",
    sources: ["cadre-api /v1/summary (paid-job and fallback block counts)", "cadre-api /v1/mining (pool balance, subsidy per block)"],
    chart: { key: "paid_share_1h", label: "Paid-job share of the blocks mined in the last hour", unit: "%", threshold: 30 },
    page: (site) => `${site}/mine`,
    rules: [
      { id: "paid_share", what: "paid-job share of the last hour's blocks under 30%", check: (pts) => { const v = last(pts)?.s.paid_share_1h; return v !== null && v !== undefined && v < 30 ? `only ${v.toFixed(0)}% of the last hour's blocks carried a paid job` : null; } },
      { id: "pool_drop", what: "mining pool balance down 5% or more in 24 h", check: (pts) => { const w = within(pts, 86_400_000); if (w.length < 2) return null; const a = w[0]!.s.pool_cadre, b = last(pts)!.s.pool_cadre; return a && b && (a - b) / a >= 0.05 ? `pool ${a.toFixed(0)} -> ${b.toFixed(0)} CADRE in the window` : null; } },
    ],
  },
  buyback: {
    title: "Buyback and reserve",
    sources: ["cadre-api /v1/crank (buyback vault, last buy, fund-mining check)"],
    chart: { key: "vault_sol", label: "Buyback vault waiting to be spent", unit: "SOL" },
    page: (site) => `${site}/crank`,
    rules: [
      { id: "crank_idle", what: "no crank for 2 hours while the vault holds more than the minimum buy", check: (pts) => { const s = last(pts)?.s; return s && s.crank_age_min !== null && s.crank_age_min! > 120 && (s.vault_sol ?? 0) > (s.min_buy_sol ?? 0) ? `last crank ${s.crank_age_min!.toFixed(0)} min ago, vault holds ${s.vault_sol!.toFixed(4)} SOL` : null; } },
      { id: "fund_blocked", what: "fund-mining refused by the program", check: (pts) => (last(pts)?.s.fund_ok === 0 ? "fund_mining would fail now (see /v1/crank fund_mining.reason)" : null) },
    ],
  },
  api: {
    title: "API and upstreams",
    sources: ["cadre-api /v1/ready (checks, upstreams, node RPC queue)", "the response time of that request from box 0"],
    chart: { key: "ready_ms", label: "cadre-api /v1/ready response time", unit: "ms", threshold: 2000 },
    page: (site) => `${site}/dashboard`,
    rules: [
      { id: "not_ready", what: "a readiness check or upstream failing", check: (pts) => { const s = last(pts)?.s; return s && s.failing ? `${s.failing} readiness checks or upstreams failing` : null; } },
      { id: "slow", what: "/v1/ready slower than 2 s three samples in a row", check: (pts) => { const w = pts.slice(-3); return w.length === 3 && w.every((p) => (p.s.ready_ms ?? 0) > 2000) ? `/v1/ready took ${w.map((p) => p.s.ready_ms).join(", ")} ms` : null; } },
      { id: "queue", what: "node RPC queue above 20", check: (pts) => { const v = last(pts)?.s.rpc_queued; return v !== null && v !== undefined && v > 20 ? `${v} node RPC calls queued in cadre-api` : null; } },
    ],
  },
  market: {
    title: "Services market",
    sources: ["cadre-api /v1/market/hires (status, deadline, votes)", "cadre-api /v1/market (counts)"],
    chart: { key: "open_or_delivered", label: "Hires open or waiting for a verdict", unit: "hires" },
    page: (site) => `${site}/market`,
    rules: [
      { id: "unjudged", what: "delivered hires within 20 blocks of their deadline with no vote", check: (pts) => { const v = last(pts)?.s.unjudged_near_deadline; return v ? `${v} delivered hires are within 20 blocks of expiring with no verdict` : null; } },
      { id: "expired_delivered", what: "hires that expired after delivery in the last 24 h", check: (pts) => { const w = within(pts, 86_400_000); if (w.length < 2) return null; const d = (last(pts)!.s.expired ?? 0) - (w[0]!.s.expired ?? 0); return d >= 3 ? `${d} hires expired in the last 24 h (refunded to the buyer)` : null; } },
    ],
  },
  wallets: {
    title: "House wallets",
    sources: ["Cadre node getAccount (CADRE) and Solana devnet getMultipleAccounts (SOL) for every house agent"],
    chart: { key: "low", label: "House agents under their job's minimum", unit: "agents" },
    page: (site) => `${site}/agents`,
    rules: [
      { id: "low", what: "house agents whose CADRE or SOL is under what their job needs", check: (pts) => { const v = last(pts)?.s.low; return v ? `${v} house agents are under their job's minimum (list in the report)` : null; } },
    ],
  },
  desktops: {
    title: "House desktops",
    sources: ["this controller's spend ledger (E2B seconds at list rate, vision usage.cost)", "its health checks"],
    chart: { key: "spent_usd", label: "Spend today", unit: "USD" },
    page: (site) => `${site}/live`,
    rules: [
      { id: "pace", what: "today's spend on pace to pass 95% of the cap", check: (pts) => { const s = last(pts)?.s; return s && s.projected_usd !== null && s.cap_usd ? (s.projected_usd! > 0.95 * s.cap_usd! ? `projected ${s.projected_usd!.toFixed(2)} USD against a ${s.cap_usd} USD cap` : null) : null; } },
      { id: "unhealthy", what: "more than 2 desktops unhealthy", check: (pts) => { const v = last(pts)?.s.unhealthy; return v !== null && v !== undefined && v > 2 ? `${v} desktops unhealthy` : null; } },
      { id: "vision", what: "vision model paused (out of credit)", check: (pts) => (last(pts)?.s.vision_paused ? "vision model paused: research and flow checks wait" : null) },
    ],
  },
};

/** Reads one sample for a target. Missing fields are null, never guessed. */
export async function sampleOf(t: MonitorTarget, c: MonitorCtx, prev: Point[]): Promise<{ s: Sample; detail?: unknown }> {
  if (t === "chain") {
    const d = await c.api("/v1/summary");
    const h = num(d?.chain?.height), f = num(d?.chain?.finalized_height);
    const p = [...prev, { t: c.now(), s: { height: h } }];
    const r = ratePerMin(p, "height", 1_800_000);
    return { s: { height: h, finalized: f, finality_lag: h !== null && f !== null ? h - f : null, block_time_s: r && r > 0 ? 60 / r : null, paid_share: num(String(d?.blocks?.paid_share ?? "").replace("%", "")) } };
  }
  if (t === "bridge") {
    const [d, b] = await Promise.all([c.api("/v1/summary"), c.api("/v1/bridge")]);
    const h = num(d?.chain?.height), tip = num(d?.light_client?.tip_height);
    return { s: { height: h, lc_tip: tip, lc_final: num(d?.light_client?.final_height), lc_lag: h !== null && tip !== null ? h - tip : null, delta_spl: num(b?.invariant?.delta_spl) } };
  }
  if (t === "storefront") {
    const d = (await c.api("/v1/summary"))?.storefront ?? {};
    return { s: { orders: num(d.orders), pending: num(d.by_status?.pending), finalized: num(d.by_status?.finalized), refunded: num(d.by_status?.refunded), awaiting_light_client: num(d.pending_by_stage?.awaiting_light_client), job_open: num(d.pending_by_stage?.job_open), job_unknown: num(d.pending_by_stage?.job_unknown), sol_pending: d.sol_pending !== undefined ? Number(d.sol_pending) / 1e9 : null } };
  }
  if (t === "mining") {
    const [d, m] = await Promise.all([c.api("/v1/summary"), c.api("/v1/mining")]);
    const total = num(d?.blocks?.total), paid = num(d?.blocks?.paid_job_blocks);
    const p = [...prev, { t: c.now(), s: { total, paid } }];
    const w = within(p, 3_600_000).filter((x) => x.s.total !== null && x.s.paid !== null);
    const dt = w.length >= 2 ? (w[w.length - 1]!.s.total! - w[0]!.s.total!) : 0;
    const dp = w.length >= 2 ? (w[w.length - 1]!.s.paid! - w[0]!.s.paid!) : 0;
    return { s: { total, paid, paid_share_1h: dt > 0 ? (100 * dp) / dt : null, blocks_1h: w.length >= 2 ? dt : null, pool_cadre: m?.mining_pool?.balance !== undefined ? Number(BigInt(m.mining_pool.balance) / 1_000_000n) / 1000 : null, subsidy_max_cadre: m?.subsidy_now?.max_per_block !== undefined ? Number(m.subsidy_now.max_per_block) / 1e9 : null } };
  }
  if (t === "buyback") {
    const d = await c.api("/v1/crank");
    const lastTs = Math.max(num(d?.buyback?.last_buy_ts) ?? 0, num(d?.reserve?.last_crank_ts) ?? 0);
    return { s: { vault_sol: d?.buyback?.vault_lamports !== undefined ? Number(d.buyback.vault_lamports) / 1e9 : null, buys: num(d?.buyback?.buys), crank_age_min: lastTs ? (c.now() / 1000 - lastTs) / 60 : null, min_buy_sol: d?.reserve?.limits?.min_buy_lamports !== undefined ? Number(d.reserve.limits.min_buy_lamports) / 1e9 : null, fund_ok: d?.fund_mining?.ok === undefined ? null : d.fund_mining.ok ? 1 : 0 } };
  }
  if (t === "api") {
    const t0 = c.now();
    const d = await c.api("/v1/ready");
    const ms = c.now() - t0;
    const checks = Object.values(d?.checks ?? {}) as any[];
    const ups = Object.values(d?.upstreams ?? {}) as any[];
    return { s: { ready_ms: ms, failing: [...checks, ...ups].filter((x) => x && x.ok === false).length + (d?.ready === false ? 1 : 0), rpc_queued: num(d?.node_rpc?.queued), rpc_avg_ms: num(d?.node_rpc?.avg_ms) }, detail: { failing: [...Object.entries(d?.checks ?? {}), ...Object.entries(d?.upstreams ?? {})].filter(([, v]: any) => v && v.ok === false).map(([k]) => k) } };
  }
  if (t === "market") {
    const d = await c.api("/v1/market/hires?limit=200");
    const hs: any[] = d?.hires ?? [];
    const h = num(d?.height) ?? 0;
    const st = (x: string) => hs.filter((y) => y.status === x).length;
    const near = hs.filter((y) => y.status === "delivered" && (y.voted ?? []).length === 0 && y.deadline - h <= 20 && y.deadline >= h);
    return { s: { open: st("open"), delivered: st("delivered"), completed: st("completed"), rejected: st("rejected"), expired: st("expired"), open_or_delivered: st("open") + st("delivered"), unjudged_near_deadline: near.length }, detail: { near_deadline: near.map((y) => ({ hire: y.id, deadline: y.deadline, offer: y.offer_title ?? y.offer })) } };
  }
  if (t === "wallets") {
    const list = c.houseAccounts?.() ?? [];
    const cadre = await Promise.all(list.map((a) => c.node("getAccount", { account: a.account }).then((r: any) => (r?.balance !== undefined ? Number(BigInt(r.balance)) / 1e9 : 0)).catch(() => null)));
    const sol = c.solBalances ? await c.solBalances(list.map((a) => a.account)).catch(() => list.map(() => null)) : list.map(() => null);
    const rows = list.map((a, i) => ({ handle: a.handle, cadre: cadre[i] ?? null, sol: sol[i] ?? null, needs: a.needs }));
    const low = rows.filter((r) => (r.cadre !== null && r.cadre < r.needs.cadre) || (r.sol !== null && r.sol < r.needs.sol));
    return { s: { agents: rows.length, low: low.length, cadre_total: rows.reduce((s, r) => s + (r.cadre ?? 0), 0), sol_total: rows.reduce((s, r) => s + (r.sol ?? 0), 0) }, detail: { rows, low: low.map((r) => r.handle) } };
  }
  // desktops: this controller's own numbers
  const s = c.self!();
  return { s: { spent_usd: s.spent, cap_usd: s.cap, e2b_usd: s.e2b, vision_usd: s.vision, projected_usd: s.spent + s.running_usd_per_s * s.seconds_left, unhealthy: s.unhealthy, running: s.running, desktops: s.desktops, vision_paused: s.vision_paused ? 1 : 0 } };
}

export interface Finding { rule: string; what: string; text: string; opened: number; last: number; closed?: number }

/** Applies a monitor's rules: opens new findings, refreshes open ones, closes cleared ones. */
export function applyRules(def: MonitorDef, pts: Point[], open: Finding[], now: number): { open: Finding[]; opened: Finding[]; closed: Finding[] } {
  const opened: Finding[] = [], closed: Finding[] = [];
  const next: Finding[] = [];
  for (const r of def.rules) {
    const text = r.check(pts, now);
    const cur = open.find((f) => f.rule === r.id);
    if (text && cur) next.push({ ...cur, text, last: now });
    else if (text) { const f = { rule: r.id, what: r.what, text, opened: now, last: now }; next.push(f); opened.push(f); }
    else if (cur) closed.push({ ...cur, closed: now });
  }
  return { open: next, opened, closed };
}
