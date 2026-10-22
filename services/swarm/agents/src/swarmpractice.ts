// Practice mode for trading swarms (docs/SWARMS.md 5): each member of a trading swarm runs a
// different style (swarmabi.ts STYLES) as a PracticeAgent on the strategies contract
// (services/swarm/strategies, docs/STRATEGIES.md), with the member's own key: its strategy
// versions, entries, exits and post-mortems are committed by that member, and the swarm view
// compares their knowledge records. Pretend balance, real mainnet prices read through the
// read-only client; no mainnet tx is ever built or signed.
//
// It runs only when this runtime has both a read-only mainnet RPC (STRAT_MAINNET_RPC or
// STRAT_MAINNET_RPC_FILE, never logged) and the strategies deploy file; otherwise every trading
// swarm says why practice is off. Contract calls go through the member's runner (its nonce
// sequence, its send chain and its daily CADRE cap); thoughts are the member's own paid inference.
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { Cadre, type ContractBody, type Sent } from "../../trading/src/cadre.ts";
import type { Wallet } from "../../src/wallet.ts";
import type { AgentRunner } from "./agent.ts";
import { REPO, STYLES, type StyleId } from "./swarmabi.ts";

/** A Cadre client whose contract sends go through a runner (one nonce sequence per key). */
class RunnerCadre extends Cadre {
  r: AgentRunner;
  send: (r: AgentRunner, b: ContractBody & { kind: "Call" }) => Promise<string>;
  constructor(r: AgentRunner, send: (r: AgentRunner, b: ContractBody & { kind: "Call" }) => Promise<string>) {
    super("http://unused.invalid");
    this.r = r;
    this.send = send;
    (this as any).rpc = r.d.node;
  }
  override async sendContract(_w: Wallet, b: ContractBody): Promise<Sent & { simulated?: any }> {
    if (b.kind !== "Call") throw new Error("practice sends only contract calls");
    const sentAt = Date.now();
    const txHash = await this.send(this.r, b);
    return { txHash, nonce: 0n, fee: 0n, sentAt, bytes: 0 };
  }
}

export interface PracticeCfg { contract: string; analyzerHash: string; mainnetUrl: string; stateDir: string; apiUrl: string; tickMs: number }

export function practiceConfig(env: NodeJS.ProcessEnv, stateDir: string, apiUrl: string, tickMs: number): { cfg: PracticeCfg | null; why: string } {
  const f = env.CADRE_STRATEGIES_CONFIG ?? join(REPO, "deploy/strategies-devnet.json");
  if (!existsSync(f)) return { cfg: null, why: "no strategies deploy file (deploy/strategies-devnet.json or CADRE_STRATEGIES_CONFIG)" };
  const j = JSON.parse(readFileSync(f, "utf8"));
  let url = env.STRAT_MAINNET_RPC?.trim() ?? "";
  if (!url && env.STRAT_MAINNET_RPC_FILE && existsSync(env.STRAT_MAINNET_RPC_FILE)) url = readFileSync(env.STRAT_MAINNET_RPC_FILE, "utf8").split("\n")[0].trim();
  if (!url) return { cfg: null, why: "no read-only mainnet RPC on this runtime (STRAT_MAINNET_RPC or STRAT_MAINNET_RPC_FILE)" };
  if (env.SWARM_PRACTICE === "off") return { cfg: null, why: "SWARM_PRACTICE=off on this runtime" };
  return { cfg: { contract: j.contract, analyzerHash: j.analyzer_hash ?? "", mainnetUrl: url, stateDir: join(stateDir, "practice"), apiUrl, tickMs }, why: "" };
}

export class SwarmPractice {
  cfg: PracticeCfg;
  agents = new Map<string, { style: StyleId; agent: any }>();
  market: any = null;
  sol: any = null;
  store: any = null;
  lastDiscover = 0;
  lastTick = 0;
  why = "starting";
  send: (r: AgentRunner, b: ContractBody & { kind: "Call" }) => Promise<string>;

  constructor(cfg: PracticeCfg, send: (r: AgentRunner, b: ContractBody & { kind: "Call" }) => Promise<string>) {
    this.cfg = cfg;
    this.send = send;
    mkdirSync(cfg.stateDir, { recursive: true, mode: 0o700 });
  }

  async init() {
    if (this.market) return;
    const { ReadOnlySolana } = await import("../../strategies/src/mainnet.ts");
    const { Market } = await import("../../strategies/src/market.ts");
    const { BlobStore } = await import("../../strategies/src/store.ts");
    this.sol = new ReadOnlySolana(this.cfg.mainnetUrl);
    await this.sol.ensureMainnet();
    this.market = new Market(this.sol);
    this.store = new BlobStore({ dir: join(this.cfg.stateDir, "blobs"), api: this.cfg.apiUrl, log: () => {} });
    this.why = "";
  }

  /** Start (once) the practice agent of a trading-swarm member running `style`. */
  async ensure(r: AgentRunner, style: StyleId) {
    if (this.agents.has(r.address)) return;
    await this.init();
    const s = STYLES.find((x) => x.id === style)!;
    const { PracticeAgent } = await import("../../strategies/src/agent.ts");
    const cadre = new RunnerCadre(r, this.send);
    await cadre.init();
    const think = async (messages: { role: string; content: string }[], maxTokens: number) => {
      await r.ensureBalance();
      const t = await r.think(r.me, messages, maxTokens, "practice");
      if (!t) throw new Error("the member's inference budget refused a practice thought today");
      if (!t.requestId) throw new Error("the gateway returned no request id: a thought without a receipt is not committed");
      return { text: t.text, requestId: t.requestId };
    };
    const agent = new PracticeAgent(
      { handle: r.spec.handle, name: r.spec.name, persona: `${r.spec.name}, ${s.persona}`, base: s.base as any },
      { cadre, contract: this.cfg.contract, market: this.market, store: this.store, think, wallet: r.me.wallet, stateFile: join(this.cfg.stateDir, `${r.address}.json`), tickMs: this.cfg.tickMs, analyzerHash: this.cfg.analyzerHash, log: (e, x) => r.log(`practice_${e}`, x) },
    );
    this.agents.set(r.address, { style, agent });
    r.note("practice", `practice mode started: style ${s.name} on the strategies contract (pretend balance, real mainnet prices, read-only)`, { ok: true });
  }

  status(agent: string) {
    const a = this.agents.get(agent);
    if (!a) return null;
    const st = a.agent.s;
    return { style: a.style, version: st.version || null, line: st.line || null, open: st.open ? { entry: st.open.entry, stage: st.open.stage } : null, explained: st.history.length };
  }

  async tick() {
    if (!this.market || this.agents.size === 0) return;
    if (Date.now() - this.lastTick < this.cfg.tickMs) return;
    this.lastTick = Date.now();
    const { discover } = await import("../../strategies/src/market.ts");
    if (Date.now() - this.lastDiscover > 15 * 60_000 || this.market.pools.size === 0) {
      const found = await discover(this.sol, { signatures: 100, txs: 40 });
      await this.market.refresh(found, { minPoolSol: 30, max: 12 });
      this.lastDiscover = Date.now();
    }
    await this.market.tick();
    for (const { agent } of this.agents.values()) agent.tick(); // each runs its own flow; a busy agent skips
  }
}
