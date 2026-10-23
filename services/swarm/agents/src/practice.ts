// Practice trading for user agents (owner 2026-10-22: "trade" for a user agent means practice).
// A hosted agent whose spec has a practice section, or a trade section without trade.real_swaps,
// runs a PracticeAgent of services/swarm/strategies (the house practice agents' module, reused as
// is): it authors a strategy version in a paid thought, commits it on the strategies contract,
// enters and exits on a pretend balance at real Solana mainnet prices (read only, through the
// allowlisted ReadOnlySolana client), and commits a post-mortem of every trade. No mainnet tx is
// ever built or signed, and no real funds back the balance.
//
// One PracticeHost per runtime: one shared mainnet market (one batched read per tick for every
// practice agent), one blob store. Contract calls go through the agent's runner (its nonce, its
// send chain, its daily CADRE cap); thoughts are the agent's own paid inference.
//
// Rate (measured 2026-10-22, docs/STRATEGIES.md 6.1): a market tick is ONE getMultipleAccounts for
// up to 12 watched pools; discovery is 1 getSignaturesForAddress + up to 40 getTransaction every 15
// minutes; an entry adds 3-4 reads. PRACTICE_TICK_MS defaults to 5000: 0.2 reads/s for the watch
// loop. Faster ticks are possible on the read budget but each entry snapshot carries the last hour
// of observations and a blob is capped at 256 KiB (store.ts), which a 2 s tick would approach.
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Cadre, type ContractBody, type Sent } from "../../trading/src/cadre.ts";
import type { Wallet } from "../../src/wallet.ts";
import { isSovereign, PRACTICE_DEFAULT_BALANCE, type AgentSpec, type PracticeStyle } from "./spec.ts";
import type { AgentRunner } from "./agent.ts";

export const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");

export interface PracticePlan {
  style: PracticeStyle;
  balance: bigint;
  maxTrade: bigint | null;
  stop_bps?: number;
  target_bps?: number;
  strategy: string;
  /** "practice": the spec's practice section; "trade": a trade section without real_swaps (practice from it). */
  source: "practice" | "trade";
}

const clamp = (x: number | undefined, lo: number, hi: number) => (x === undefined ? undefined : Math.min(hi, Math.max(lo, Math.round(x))));

/** True only when the spec turns real swaps on (trade.real_swaps); no creation flow sets it. */
export const realSwapsOn = (s: AgentSpec) => isSovereign(s) && !!s.trade && s.trade.real_swaps === true && !!s.actions.trade && s.skills.includes("trade");

/** What this agent practices, or null. A trade section without real_swaps practices too. */
export function practicePlan(s: AgentSpec): PracticePlan | null {
  if (!isSovereign(s)) return null;
  const p = s.practice;
  if (p && s.skills.includes("practice")) {
    return { style: p.style, balance: BigInt(p.balance_lamports), maxTrade: p.max_trade_lamports ? BigInt(p.max_trade_lamports) : null, stop_bps: p.stop_loss_bps, target_bps: p.take_profit_bps, strategy: p.strategy, source: "practice" };
  }
  const t = s.trade;
  if (t && s.actions.trade && s.skills.includes("trade") && t.real_swaps !== true) {
    // the strategies module's bounds: stop 100..5000, target 100..20000 bps
    return { style: "momentum", balance: BigInt(PRACTICE_DEFAULT_BALANCE), maxTrade: null, stop_bps: clamp(t.stop_loss_bps, 100, 5_000), target_bps: clamp(t.take_profit_bps, 100, 20_000), strategy: t.strategy, source: "trade" };
  }
  return null;
}

/** The PracticeAgent config for a plan (styles.ts conditions; size scaled to the pretend balance). */
export async function practiceCfg(s: AgentSpec, plan: PracticePlan) {
  const { styleById } = await import("../../strategies/src/styles.ts");
  const st = styleById(plan.style)!;
  const max = plan.maxTrade ?? plan.balance / 10n;
  const idea = plan.strategy.trim() ? ` Its creator's starting idea: ${plan.strategy.trim().slice(0, 200)}` : "";
  return {
    handle: s.handle, name: s.name, persona: `${s.name}, ${st.persona}.${idea}`.slice(0, 400),
    base: { ...st.base, practice_balance_lamports: Number(plan.balance), size: { ...st.base.size, max_lamports: Number(max) } },
    pin: plan.stop_bps !== undefined || plan.target_bps !== undefined ? { stop_bps: plan.stop_bps, target_bps: plan.target_bps } : undefined,
  };
}

/** A Cadre client whose contract calls go through the runner (one nonce sequence per key). */
class RunnerCadre extends Cadre {
  r: AgentRunner;
  constructor(r: AgentRunner) {
    super("http://unused.invalid");
    this.r = r;
    (this as any).rpc = r.d.node;
  }
  override async sendContract(_w: Wallet, b: ContractBody): Promise<Sent & { simulated?: any }> {
    if (b.kind !== "Call") throw new Error("practice sends only contract calls");
    const sentAt = Date.now();
    const s = await this.r.sendCall(b);
    return { txHash: s.txHash, nonce: s.nonce, fee: s.fee, sentAt, bytes: 0 };
  }
}

export interface PracticeHostCfg { contract: string; analyzerHash: string; mainnetUrl: string; stateDir: string; apiUrl: string; tickMs: number }

/** The host config from the environment, or why practice is off on this runtime. */
export function practiceConfig(env: NodeJS.ProcessEnv, stateDir: string, apiUrl: string): { cfg: PracticeHostCfg | null; why: string } {
  if (env.AGENTS_PRACTICE === "off") return { cfg: null, why: "AGENTS_PRACTICE=off on this runtime" };
  const f = env.CADRE_STRATEGIES_CONFIG ?? join(REPO, "deploy/strategies-devnet.json");
  if (!existsSync(f)) return { cfg: null, why: "no strategies deploy file (deploy/strategies-devnet.json or CADRE_STRATEGIES_CONFIG)" };
  const j = JSON.parse(readFileSync(f, "utf8"));
  let url = "";
  try {
    url = mainnetUrlSync(env);
  } catch {
    return { cfg: null, why: "no read-only mainnet RPC on this runtime (STRAT_MAINNET_RPC or STRAT_MAINNET_RPC_FILE)" };
  }
  const tickMs = Number(env.PRACTICE_TICK_MS ?? 5_000);
  return { cfg: { contract: j.contract, analyzerHash: j.analyzer_hash ?? "", mainnetUrl: url, stateDir: join(stateDir, "practice"), apiUrl, tickMs }, why: "" };
}

// strategies/src/mainnet.ts mainnetUrl, inlined so the config read stays synchronous at startup
function mainnetUrlSync(env: NodeJS.ProcessEnv): string {
  if (env.STRAT_MAINNET_RPC) return env.STRAT_MAINNET_RPC.trim();
  const first = (f: string) => (existsSync(f) ? readFileSync(f, "utf8").split("\n")[0].trim() : "");
  if (env.STRAT_MAINNET_RPC_FILE) { const u = first(env.STRAT_MAINNET_RPC_FILE); if (u) return u; }
  const m = first(join(REPO, "keys/mainnet/solana-rpc-url"));
  if (m) return m;
  for (const f of [join(REPO, "keys/devnet/solana-rpc-url"), "/etc/lode/solana-rpc-url"]) {
    const u = first(f);
    if (/^https:\/\/devnet\.helius-rpc\.com\//.test(u)) return u.replace("https://devnet.helius-rpc.com/", "https://mainnet.helius-rpc.com/");
  }
  throw new Error("no mainnet read RPC");
}

/** Events of a PracticeAgent that the creator's live feed shows (agent log lines). */
const FEED: Record<string, (x: any) => string> = {
  committed: (x) => `practice: committed its ${x.what} on the strategies contract`,
  version: (x) => `practice: committed strategy version ${x.id}${x.change && x.change !== "first version" ? ` (${String(x.change).slice(0, 160)})` : " (its first)"}`,
  entered: (x) => `practice: entered pool ${String(x.pool).slice(0, 8)}... with ${Number(x.size) / 1e9} pretend SOL (entry #${x.entry}, committed before the fill)`,
  exit_due: (x) => `practice: exit due on entry #${x.entry} (${x.reason})`,
  postmortem: (x) => `practice: post-mortem of entry #${x.entry}: ${x.outcome}, ${Number(x.pnl) / 1e9} pretend SOL${x.buckets?.length ? `, ${x.buckets.join(", ")}` : ""}`,
  skip: () => "practice: looked at a pool that met its rules and skipped it",
  condition_gone: () => "practice: its rules stopped holding before the entry",
  thought: (x) => `practice thought: ${String(x.text ?? "").slice(0, 120)}`,
  error: (x) => `practice error: ${String(x.message ?? "").slice(0, 200)}`,
};

/** The PracticeAgent's log hook: every event to the runtime log; the feed events (FEED) into the
 * agent's own log with their tx, and one line per distinct error per 5 minutes, so a retried
 * failure (the gateway busy, a refused simulation) does not push every other line out of the feed. */
export function practiceLog(r: { log: (e: string, x: Record<string, unknown>) => void; note: (k: string, t: string, x?: any) => void }, now = () => Date.now()) {
  let lastErr = { text: "", at: 0 };
  return (e: string, x: Record<string, unknown>) => {
    r.log(`practice_${e}`, x);
    const f = FEED[e];
    if (!f) return;
    const text = f(x);
    if (e === "error") {
      const key = text.replace(/\d+/g, "#");
      if (key === lastErr.text && now() - lastErr.at < 300_000) return;
      lastErr = { text: key, at: now() };
    }
    r.note(e === "error" ? "practice_error" : "practice", text, { ok: e !== "error", tx: (x as any).tx, request_id: (x as any).request });
  };
}

export class PracticeHost {
  cfg: PracticeHostCfg;
  agents = new Map<string, { plan: PracticePlan; agent: any }>();
  market: any = null;
  sol: any = null;
  store: any = null;
  lastDiscover = 0;
  why = "starting";
  stats = { ticks: 0, last_tick_ms: 0, mainnet_calls: 0, started_ms: Date.now() };

  constructor(cfg: PracticeHostCfg) {
    this.cfg = cfg;
    mkdirSync(cfg.stateDir, { recursive: true, mode: 0o700 });
  }

  async init(nodeRpc?: (m: string, p: unknown) => Promise<any>) {
    if (this.market) return;
    const { ReadOnlySolana } = await import("../../strategies/src/mainnet.ts");
    const { Market } = await import("../../strategies/src/market.ts");
    const { BlobStore } = await import("../../strategies/src/store.ts");
    this.sol = new ReadOnlySolana(this.cfg.mainnetUrl);
    await this.sol.ensureMainnet();
    this.market = new Market(this.sol);
    this.store = new BlobStore({ dir: join(this.cfg.stateDir, "blobs"), api: this.cfg.apiUrl, nodeRpc: nodeRpc ?? null, log: () => {} });
    this.why = "";
  }

  /** Start (once) the practice agent of a running runner whose spec practices. */
  async ensure(r: AgentRunner): Promise<boolean> {
    if (this.agents.has(r.address)) return false;
    const plan = practicePlan(r.spec);
    if (!plan) return false;
    await this.init((m, p) => r.d.node.call(m, p));
    const { PracticeAgent } = await import("../../strategies/src/agent.ts");
    const cadre = new RunnerCadre(r);
    await cadre.init();
    const think = async (messages: { role: string; content: string }[], maxTokens: number) => {
      await r.ensureBalance();
      const t = await r.think(r.me, messages, maxTokens, "practice");
      if (!t) throw new Error("the agent's inference budget refused a practice thought today");
      if (!t.requestId) throw new Error("the gateway returned no request id: a thought without a receipt is not committed");
      return { text: t.text, requestId: t.requestId };
    };
    const log = practiceLog(r);
    const agent = new PracticeAgent(await practiceCfg(r.spec, plan), {
      cadre, contract: this.cfg.contract, market: this.market, store: this.store, think, wallet: r.me.wallet,
      stateFile: join(this.cfg.stateDir, `${r.address}.json`), tickMs: this.cfg.tickMs, analyzerHash: this.cfg.analyzerHash, log,
    });
    this.agents.set(r.address, { plan, agent });
    r.note("practice", `practice trading started: style ${plan.style}, ${Number(plan.balance) / 1e9} pretend SOL at real Solana mainnet prices (read only); every version, entry, exit and post-mortem goes on the strategies contract${plan.source === "trade" ? "; its spec's trade section runs as practice (real swaps are off: trade.real_swaps is not set)" : ""}`, { ok: true });
    // its first step at once (the authoring thought), not on the next market tick
    void agent.tick();
    return true;
  }

  status(agent: string) {
    const a = this.agents.get(agent);
    if (!a) return null;
    const st = a.agent.s;
    const pnl = st.history.reduce((x: bigint, h: any) => x + BigInt(h.pnl), 0n);
    return {
      mode: "practice", label: "practice trading: pretend money at real prices, every trade and lesson recorded on Cadre",
      contract: this.cfg.contract, style: a.plan.style, source: a.plan.source, pretend_balance_lamports: a.plan.balance.toString(),
      version: st.version || null, line: st.line || null, open: st.open ? { entry: st.open.entry, stage: st.open.stage, pool: st.open.pool } : null,
      explained: st.history.length, practice_pnl_lamports: pnl.toString(), tick_ms: this.cfg.tickMs,
    };
  }

  /** One market tick (shared by every practice agent), then each agent's own step. */
  async tick() {
    if (!this.market || this.agents.size === 0) return;
    const { discover } = await import("../../strategies/src/market.ts");
    if (Date.now() - this.lastDiscover > 15 * 60_000 || this.market.pools.size === 0) {
      const found = await discover(this.sol, { signatures: 100, txs: 40 });
      const keep = new Set([...this.agents.values()].map((a) => a.agent.s.open?.pool).filter((x): x is string => !!x));
      await this.market.refresh(found, { minPoolSol: 30, max: 12, keep });
      this.lastDiscover = Date.now();
    }
    const t0 = Date.now();
    await this.market.tick();
    this.stats.ticks++;
    this.stats.last_tick_ms = Date.now() - t0;
    this.stats.mainnet_calls = this.sol.calls;
    for (const { agent } of this.agents.values()) agent.tick(); // each runs its own flow; a busy agent skips
  }

  info() {
    const mins = Math.max(1, (Date.now() - this.stats.started_ms) / 60_000);
    return { enabled: true, contract: this.cfg.contract, tick_ms: this.cfg.tickMs, agents: this.agents.size, ticks: this.stats.ticks, mainnet_reads: this.stats.mainnet_calls, mainnet_reads_per_min: Number((this.stats.mainnet_calls / mins).toFixed(2)), mainnet: "read only" };
  }
}
