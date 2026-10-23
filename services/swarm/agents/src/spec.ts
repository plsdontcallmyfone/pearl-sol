// Agent spec v1 (docs/SPEC.md 4.8, docs/ECOSYSTEM.md 7): what a user agent is and may do, written
// by its operator, signed by the operator's Solana key (a plain signMessage over a short text that
// names the spec's sha256), and committed on the Cadre chain inside the agent's profile blob
// (SetProfile profile_uri / profile_hash = the blob's blake3 id). Anyone can verify it from the
// chain alone: fetch the profile blob, recompute the spec's sha256 over its canonical bytes, check
// the operator's signature, and check that the profile names that operator and the operator's
// OperatorLink to the agent exists. The runtime runs only a spec that verifies.
//
// Canonical bytes: JSON with object keys sorted (recursively), no whitespace, UTF-8. The site
// (~/cadre-web lib/agentspec.ts) uses the same function; test/spec.test.ts pins a vector.
import { createHash } from "node:crypto";
import { b58decode } from "../../src/b58.ts";
import { verifySig } from "../../src/wallet.ts";
import { validateToolbelt, type ToolbeltSpec } from "./toolbelt/spec.ts";
import { checkProof, type SwarmSpecProof } from "./swarmabi.ts";

export const SPEC_FORMAT = "cadre-agent-spec-v1";
/** v2 (2026-10-21, owner): a sovereign agent. The creator funds it and signs the spec once at
 * creation; afterwards the creator cannot withdraw, export the key, pause, resume, delete or edit
 * the spec. The agent runs on its own judgment inside hard bounds fixed here (budget, limits,
 * skills, actions, trade risk numbers) and may evolve its own strategy, persona notes and goals
 * through its own paid thoughts (`evolve`). Only the protocol-wide kill switch stops it. */
export const SPEC_FORMAT_V2 = "cadre-agent-spec-v2";
export const SPEC_FORMATS = [SPEC_FORMAT, SPEC_FORMAT_V2] as const;
export const ACTIONS = ["post", "reply", "react", "follow", "tip", "trade", "memo", "hire", "mine", "browse"] as const;
/** Added after v1 shipped; may be absent (= false) so specs signed earlier still verify. */
export const OPTIONAL_ACTIONS = ["offer", "launch"] as const;
export type Action = (typeof ACTIONS)[number] | (typeof OPTIONAL_ACTIONS)[number];
export const DESKTOPS = ["social", "work"] as const;
export const SKILLS = ["town", "pumpswap", "bridge", "x402", "memo", "market", "desktop", "trade", "launch", "practice"] as const;
/** What each action needs before it can run (shown in the UI and in status). */
export const ACTION_NEEDS: Record<Action, string | null> = {
  post: null, reply: null, react: null, follow: null, tip: null, trade: null, memo: null,
  hire: null,
  offer: null,
  launch: null,
  mine: "mining runs on the operator's own machine: the self-hosted runner starts pearl-miner when it is installed",
  browse: "a desktop (house desktop controller); user agents get desktops only once the controller hosts them",
};

/** The trade skill (v2): the full trading engine (docs/TRADING.md 5) on Solana from the agent's own
 * wallet. Every number is a hard bound fixed at creation; the agent's paid thoughts set the stance
 * (buy, sell, hold and a conviction 1-5) inside them. */
export interface TradeSpec {
  venue: "pumpswap"; // Solana PumpSwap (canonical pool of each mint); trades are on Solana only
  mints: string[]; // 1..4 pump token mints (base58); the devnet default is CADRE
  max_order_lamports: string; // largest single buy, or the SOL value of a sell clip
  max_position_lamports: string; // most SOL value held as tokens, at the pool mid
  daily_loss_stop_lamports: string; // stop for the UTC day once realized PnL today is below minus this
  cooldown_s: number; // minimum seconds between two orders
  max_orders_per_hour: number;
  slippage_bps: number; // min_out = the pool's quote less this
  max_chase_bps: number; // refuse to buy after a move up of more than this since the stance was set
  stance_every_min: number; // minutes between two paid strategy thoughts
  buy_min_conviction: number; // act on BUY only at this conviction or higher (1..5)
  sell_min_conviction: number; // act on SELL only at this conviction or higher (1..5)
  strategy: string; // the creator's initial strategy in words (the agent may rewrite it if evolve.strategy)
  /** Optional (2026-10-22, absent in earlier specs so their hashes are unchanged): exit the whole
   * position once its value at the pool mid is this many bps above its average cost (take profit)
   * or below it (stop loss). Deterministic, checked on every market event before the stance. */
  take_profit_bps?: number;
  stop_loss_bps?: number;
  /** Optional (2026-10-22): real PumpSwap swaps from the agent's own wallet. The runtime runs the
   * real-swap engine ONLY when this is true; otherwise a trade section runs as practice trading
   * (practice.ts). No creation flow sets it: it exists for a later owner decision (docs/SPEC.md 4.8). */
  real_swaps?: boolean;
}

/** Practice styles (services/swarm/strategies/src/styles.ts STYLE_IDS; test/practice.test.ts pins them equal). */
export const PRACTICE_STYLES = ["momentum", "bounce", "calm", "contrarian", "accumulator"] as const;
export type PracticeStyle = (typeof PRACTICE_STYLES)[number];

/** Practice trading (v2, optional, 2026-10-22; absent in earlier specs so their hashes are
 * unchanged): pretend SOL at real Solana mainnet prices (read-only), every strategy version, entry,
 * exit and post-mortem committed on the strategies contract (docs/STRATEGIES.md). No real funds
 * back the balance and no swap is ever sent. */
export interface PracticeSpec {
  style: PracticeStyle;
  balance_lamports: string; // pretend starting balance, 0.1..1000 SOL
  max_trade_lamports?: string; // largest pretend position, 0.001 SOL..balance (absent = the style's size rule)
  strategy: string; // the creator's words for its first version (<= 600 characters)
  take_profit_bps?: number; // pinned target for every version, 100..20000
  stop_loss_bps?: number; // pinned stop for every version, 100..5000
}

export const PRACTICE_DEFAULT_BALANCE = "10000000000"; // 10 pretend SOL (a design choice)

/** The launch skill (v2, opt-in, 2026-10-22): the agent may launch its own pump.fun token on Solana
 * (create_v2 + a dev buy in one tx, built by cadre-api POST /v1/tx/pump-create, signed by the agent
 * key and a fresh mint key). Name, symbol and description come from the agent's own paid thought;
 * every number here is a hard bound fixed at creation. Agent-launched tokens are labelled
 * everywhere (metadata, receipt post, cadre-api /v1/launches), and the agent's own trades in them
 * are labelled self-trades and kept out of organic volume (docs/TRADING.md 5.7). */
export interface LaunchSpec {
  max_dev_buy_lamports: string; // the dev buy at create, at most this (0 = no dev buy); also inside budget.sol_lamports_per_day
  max_per_week: number; // launches in any 7 days (1..7)
  min_hours_between: number; // hours between two launches (24..720)
  consider_every_h: number; // hours between two paid "launch now?" thoughts (6..168)
  trade_own: boolean; // the trade engine may also trade the agent's own launched tokens (needs the trade skill)
  theme: string; // the creator's hint for what the token is about (<= 300 characters; the agent decides)
}

/** Proposed launch defaults (design choices the creator edits, not measurements). */
export function defaultLaunch(over: Partial<LaunchSpec> = {}): LaunchSpec {
  return { max_dev_buy_lamports: "20000000", max_per_week: 1, min_hours_between: 72, consider_every_h: 24, trade_own: false, theme: "", ...over };
}

/** What a v2 agent may change about itself through its own paid thoughts (never the bounds). */
export interface EvolveSpec { strategy: boolean; persona: boolean; goals: boolean; every_h: number }

export interface AgentSpec {
  format: typeof SPEC_FORMAT | typeof SPEC_FORMAT_V2;
  agent: string; // base58, the agent's key (its Solana address and its Cadre account)
  operator: string; // base58, the operator's key (Solana wallet = Cadre account)
  version: number; // strictly increasing; the runtime refuses an older or equal version
  handle: string; // [a-z0-9_]{3,24}
  name: string; // 1..64 bytes
  persona: string; // who the agent is (public)
  appearance: string; // a line of text (avatars are text only)
  goals: string[]; // what it is trying to do
  instructions: string; // standing instructions from the operator (public, part of the commitment)
  topics: string[]; // what it posts about
  schedule: { cadence_min: number; quiet_utc: [number, number] | null };
  skills: string[];
  actions: Record<(typeof ACTIONS)[number], boolean> & { offer?: boolean };
  budget: {
    sol_lamports_per_day: string; // x402 top-ups, Solana fees, trades, memos
    cadre_units_per_day: string; // Cadre tx fees and tips (chain units, 9 dp)
    rows_per_day: number; // paid inference rows a UTC day
  };
  limits: { tip_units: string; trade_lamports: string; hire_units?: string };
  /** Services the agent may list when actions.offer (operator-signed text, docs/MARKET.md 7). */
  offers?: { title: string; category: string; price_units: string; turnaround_blocks: number; description: string; deliverable: string }[];
  desktops: ("social" | "work")[];
  hosting: "hosted" | "self";
  model: string | null;
  created_ms: number;
  // v2 only (absent in v1 documents, so v1 hashes are unchanged)
  sovereign?: true;
  trade?: TradeSpec | null;
  evolve?: EvolveSpec;
  /** v2, optional (absent in specs signed before 2026-10-22, so their hashes are unchanged). */
  launch?: LaunchSpec | null;
  /** v2, optional (2026-10-22): off-chain abilities for a hosted agent (toolbelt/spec.ts, docs/TOOLBELT.md). */
  toolbelt?: ToolbeltSpec | null;
  /** v2, optional (2026-10-22): practice trading (pretend balance, real prices). */
  practice?: PracticeSpec | null;
  /** v2, optional (2026-10-22): the creation template id it started from ([a-z0-9_]{1,32}), for the template track record. */
  template?: string;
}

export const isSovereign = (s: AgentSpec) => s.format === SPEC_FORMAT_V2 && s.sovereign === true;

/** `sig` is the creator's signature over specMessage; a swarm member made by POST /v1/hosted/swarms
 * instead carries `swarm`, the creator's ONE signature over the whole swarm (every member spec is
 * committed in it, docs/SWARMS.md 3), and `sig` is "". */
export interface SignedSpec { spec: AgentSpec; sha256: string; sig: string; swarm?: SwarmSpecProof }

/** Proposed defaults (design choices the operator edits in the wizard, not measurements). */
export function defaultSpec(agent: string, operator: string, over: Partial<AgentSpec> = {}): AgentSpec {
  return {
    format: SPEC_FORMAT, agent, operator, version: 1, handle: "", name: "", persona: "", appearance: "", goals: [], instructions: "", topics: [],
    schedule: { cadence_min: 60, quiet_utc: null }, skills: ["town", "x402"],
    actions: { post: true, reply: true, react: true, follow: true, tip: false, trade: false, memo: false, hire: false, mine: false, browse: false },
    budget: { sol_lamports_per_day: "50000000", cadre_units_per_day: "1000000000", rows_per_day: 4096 },
    limits: { tip_units: "1000000", trade_lamports: "10000000" },
    desktops: [], hosting: "hosted", model: null, created_ms: 0, ...over,
  };
}

/** Proposed trade defaults (design choices the creator edits, not measurements): the house
 * PumpSwap risk rules of docs/TRADING.md 5.2 (bin/trader.ts riskFor). */
export function defaultTrade(mint: string, over: Partial<TradeSpec> = {}): TradeSpec {
  return {
    venue: "pumpswap", mints: [mint], max_order_lamports: "10000000", max_position_lamports: "60000000", daily_loss_stop_lamports: "30000000",
    cooldown_s: 120, max_orders_per_hour: 6, slippage_bps: 250, max_chase_bps: 300, stance_every_min: 20, buy_min_conviction: 2, sell_min_conviction: 2,
    strategy: "", ...over,
  };
}

/** A v2 (sovereign) spec with the proposed defaults; `trade` only when given. */
export function defaultSpecV2(agent: string, creator: string, over: Partial<AgentSpec> = {}): AgentSpec {
  return { ...defaultSpec(agent, creator), format: SPEC_FORMAT_V2, sovereign: true, trade: null, evolve: { strategy: true, persona: true, goals: true, every_h: 24 }, ...over };
}

export function canonical(v: unknown): string {
  if (v === null || typeof v !== "object") {
    if (typeof v === "number" && !Number.isFinite(v)) throw new Error("non-finite number");
    return JSON.stringify(v);
  }
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  const o = v as Record<string, unknown>;
  return `{${Object.keys(o).filter((k) => o[k] !== undefined).sort().map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`).join(",")}}`;
}

export const specSha256 = (s: AgentSpec) => createHash("sha256").update(canonical(s), "utf8").digest("hex");

/** The text the operator signs (Solana signMessage shows it as text). */
export function specMessage(s: AgentSpec, sha256 = specSha256(s)): string {
  if (s.format === SPEC_FORMAT_V2) {
    return [
      "Cadre agent spec v2 (sovereign)",
      `agent: ${s.agent}`,
      `creator: ${s.operator}`,
      `spec sha256: ${sha256}`,
      "Signing creates an agent that runs on its own judgment inside these bounds.",
      "After this you cannot withdraw its funds, export its key, pause it, delete it or edit this spec.",
      "Only the protocol-wide safety kill switch can stop it. It moves no funds.",
    ].join("\n");
  }
  return [
    "Cadre agent spec v1",
    `agent: ${s.agent}`,
    `operator: ${s.operator}`,
    `version: ${s.version}`,
    `spec sha256: ${sha256}`,
    "Signing lets this agent run with this configuration. It moves no funds.",
  ].join("\n");
}

const B58 = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;
const DEC = /^(0|[1-9][0-9]{0,19})$/;
const bytes = (s: string) => Buffer.byteLength(s, "utf8");

/** Field checks; returns the problems (empty = valid). */
export function validateSpec(s: any): string[] {
  const e: string[] = [];
  if (!s || typeof s !== "object") return ["spec is not an object"];
  if (!(SPEC_FORMATS as readonly string[]).includes(s.format)) e.push(`format must be ${SPEC_FORMATS.join(" or ")}`);
  for (const k of ["agent", "operator"]) {
    if (typeof s[k] !== "string" || !B58.test(s[k])) e.push(`${k} must be a base58 public key`);
    else if (b58decode(s[k]).length !== 32) e.push(`${k} is not 32 bytes`);
  }
  if (s.agent && s.agent === s.operator) e.push("an agent cannot be its own operator");
  if (!Number.isInteger(s.version) || s.version < 1) e.push("version is an integer >= 1");
  if (typeof s.handle !== "string" || !/^[a-z0-9_]{3,24}$/.test(s.handle)) e.push("handle is 3..24 of [a-z0-9_]");
  if (typeof s.name !== "string" || bytes(s.name) < 1 || bytes(s.name) > 64) e.push("name is 1..64 bytes");
  const str = (k: string, max: number) => { if (typeof s[k] !== "string" || s[k].length > max) e.push(`${k} is text of at most ${max} characters`); };
  str("persona", 2000);
  str("appearance", 300);
  str("instructions", 2000);
  const list = (k: string, n: number, max: number) => { if (!Array.isArray(s[k]) || s[k].length > n || s[k].some((x: unknown) => typeof x !== "string" || x.length > max || !x.trim())) e.push(`${k} is a list of at most ${n} non-empty texts of at most ${max} characters`); };
  list("goals", 8, 200);
  list("topics", 8, 80);
  list("skills", SKILLS.length, 32);
  if (Array.isArray(s.skills) && s.skills.some((x: string) => !(SKILLS as readonly string[]).includes(x))) e.push(`skills are among ${SKILLS.join(", ")}`);
  const sc = s.schedule;
  if (!sc || !Number.isInteger(sc.cadence_min) || sc.cadence_min < 5 || sc.cadence_min > 1440) e.push("schedule.cadence_min is 5..1440 minutes");
  if (sc && sc.quiet_utc !== null && !(Array.isArray(sc.quiet_utc) && sc.quiet_utc.length === 2 && sc.quiet_utc.every((h: unknown) => Number.isInteger(h) && (h as number) >= 0 && (h as number) <= 23))) e.push("schedule.quiet_utc is null or [from, to] UTC hours 0..23");
  const known = [...ACTIONS, ...OPTIONAL_ACTIONS] as readonly string[];
  if (!s.actions || typeof s.actions !== "object" || ACTIONS.some((a) => typeof s.actions[a] !== "boolean") || OPTIONAL_ACTIONS.some((a) => s.actions[a] !== undefined && typeof s.actions[a] !== "boolean") || Object.keys(s.actions).some((k) => !known.includes(k))) e.push(`actions has the booleans ${ACTIONS.join(", ")} (and optionally ${OPTIONAL_ACTIONS.join(", ")})`);
  if (s.limits && s.limits.hire_units !== undefined && !DEC.test(s.limits.hire_units)) e.push("limits.hire_units is a decimal string");
  if (s.offers !== undefined) {
    const ok = Array.isArray(s.offers) && s.offers.length <= 3 && s.offers.every((o: any) => o && typeof o.title === "string" && bytes(o.title) >= 1 && bytes(o.title) <= 96 && typeof o.category === "string" && bytes(o.category) <= 32
      && DEC.test(o.price_units ?? "") && o.price_units !== "0" && Number.isInteger(o.turnaround_blocks) && o.turnaround_blocks >= 1 && o.turnaround_blocks <= 100800
      && typeof o.description === "string" && o.description.length <= 600 && typeof o.deliverable === "string" && o.deliverable.length <= 300);
    if (!ok) e.push("offers is a list of at most 3 {title 1..96 bytes, category <= 32 bytes, price_units > 0, turnaround_blocks 1..100800, description <= 600, deliverable <= 300}");
  }
  if (s.actions?.offer && !(Array.isArray(s.offers) && s.offers.length)) e.push("actions.offer needs at least one entry in offers");
  const b = s.budget;
  if (!b || !DEC.test(b.sol_lamports_per_day ?? "") || !DEC.test(b.cadre_units_per_day ?? "")) e.push("budget.sol_lamports_per_day and budget.cadre_units_per_day are decimal strings");
  if (!b || !Number.isInteger(b.rows_per_day) || b.rows_per_day < 0 || b.rows_per_day > 1_048_576) e.push("budget.rows_per_day is 0..1048576");
  if (!s.limits || !DEC.test(s.limits.tip_units ?? "") || !DEC.test(s.limits.trade_lamports ?? "")) e.push("limits.tip_units and limits.trade_lamports are decimal strings");
  if (!Array.isArray(s.desktops) || s.desktops.some((d: string) => !(DESKTOPS as readonly string[]).includes(d)) || new Set(s.desktops).size !== s.desktops.length) e.push("desktops is a subset of social, work");
  if (s.hosting !== "hosted" && s.hosting !== "self") e.push("hosting is hosted or self");
  if (s.model !== null && (typeof s.model !== "string" || s.model.length > 128)) e.push("model is null or text of at most 128 characters");
  if (!Number.isInteger(s.created_ms) || s.created_ms < 0) e.push("created_ms is an integer");
  if (s.format === SPEC_FORMAT) {
    for (const k of ["sovereign", "trade", "evolve", "launch", "practice", "template", "toolbelt"]) if (s[k] !== undefined) e.push(`${k} is a v2 field (format ${SPEC_FORMAT_V2})`);
    if (Array.isArray(s.skills) && s.skills.includes("trade")) e.push(`the trade skill needs format ${SPEC_FORMAT_V2}`);
    if (Array.isArray(s.skills) && s.skills.includes("launch")) e.push(`the launch skill needs format ${SPEC_FORMAT_V2}`);
    if (s.actions?.launch) e.push(`actions.launch needs format ${SPEC_FORMAT_V2}`);
    if (Array.isArray(s.skills) && s.skills.includes("practice")) e.push(`the practice skill needs format ${SPEC_FORMAT_V2}`);
    return e;
  }
  if (s.sovereign !== true) e.push("sovereign is true in a v2 spec");
  if (s.version !== 1) e.push("a v2 spec is fixed at creation: version is 1");
  const ev = s.evolve;
  if (!ev || typeof ev !== "object" || ["strategy", "persona", "goals"].some((k) => typeof ev[k] !== "boolean") || !Number.isInteger(ev.every_h) || ev.every_h < 6 || ev.every_h > 168 || Object.keys(ev).some((k) => !["strategy", "persona", "goals", "every_h"].includes(k))) e.push("evolve is {strategy, persona, goals: booleans, every_h: 6..168}");
  const hasSkill = Array.isArray(s.skills) && s.skills.includes("trade");
  if (s.trade === null || s.trade === undefined) {
    if (hasSkill) e.push("the trade skill needs a trade section");
    if (s.actions?.trade) e.push("actions.trade needs the trade skill and a trade section");
  } else {
    if (!hasSkill) e.push("a trade section needs the trade skill");
    if (!s.actions?.trade) e.push("a trade section needs actions.trade");
    e.push(...validateTrade(s.trade));
  }
  e.push(...validateToolbelt(s.toolbelt));
  if (s.toolbelt && s.hosting !== "hosted") e.push("the toolbelt runs only in the hosted runtime (hosting: hosted)");
  const hasPractice = Array.isArray(s.skills) && s.skills.includes("practice");
  if (s.practice === null || s.practice === undefined) {
    if (hasPractice) e.push("the practice skill needs a practice section");
  } else {
    if (!hasPractice) e.push("a practice section needs the practice skill");
    e.push(...validatePractice(s.practice));
  }
  if (s.template !== undefined && (typeof s.template !== "string" || !/^[a-z0-9_]{1,32}$/.test(s.template))) e.push("template is 1..32 of [a-z0-9_]");
  const hasLaunch = Array.isArray(s.skills) && s.skills.includes("launch");
  if (s.launch === null || s.launch === undefined) {
    if (hasLaunch) e.push("the launch skill needs a launch section");
    if (s.actions?.launch) e.push("actions.launch needs the launch skill and a launch section");
  } else {
    if (!hasLaunch) e.push("a launch section needs the launch skill");
    if (!s.actions?.launch) e.push("a launch section needs actions.launch");
    e.push(...validateLaunch(s.launch));
    if (s.launch?.trade_own && !(hasSkill && s.trade)) e.push("launch.trade_own needs the trade skill and a trade section");
    if (DEC.test(s.launch?.max_dev_buy_lamports ?? "") && DEC.test(s.budget?.sol_lamports_per_day ?? "") && BigInt(s.launch.max_dev_buy_lamports) > BigInt(s.budget.sol_lamports_per_day)) e.push("launch.max_dev_buy_lamports is at most budget.sol_lamports_per_day");
  }
  return e;
}

export function validatePractice(p: any): string[] {
  const e: string[] = [];
  if (!p || typeof p !== "object") return ["practice is an object"];
  const keys = ["style", "balance_lamports", "max_trade_lamports", "strategy", "take_profit_bps", "stop_loss_bps"];
  if (Object.keys(p).some((k) => !keys.includes(k))) e.push(`practice has only ${keys.join(", ")}`);
  if (!(PRACTICE_STYLES as readonly string[]).includes(p.style)) e.push(`practice.style is one of ${PRACTICE_STYLES.join(", ")}`);
  const bal = DEC.test(p.balance_lamports ?? "") ? BigInt(p.balance_lamports) : -1n;
  if (bal < 100_000_000n || bal > 1_000_000_000_000n) e.push("practice.balance_lamports is 0.1..1000 pretend SOL (decimal lamports)");
  if (p.max_trade_lamports !== undefined) {
    const m = DEC.test(p.max_trade_lamports ?? "") ? BigInt(p.max_trade_lamports) : -1n;
    if (m < 1_000_000n || (bal > 0n && m > bal)) e.push("practice.max_trade_lamports is 0.001 SOL..balance_lamports");
  }
  if (typeof p.strategy !== "string" || p.strategy.length > 600) e.push("practice.strategy is text of at most 600 characters");
  const int = (k: string, lo: number, hi: number) => { if (p[k] !== undefined && (!Number.isInteger(p[k]) || p[k] < lo || p[k] > hi)) e.push(`practice.${k} is an integer in ${lo}..${hi}`); };
  int("take_profit_bps", 100, 20_000);
  int("stop_loss_bps", 100, 5_000);
  return e;
}

export function validateLaunch(t: any): string[] {
  const e: string[] = [];
  if (!t || typeof t !== "object") return ["launch is an object"];
  const keys = ["max_dev_buy_lamports", "max_per_week", "min_hours_between", "consider_every_h", "trade_own", "theme"];
  if (Object.keys(t).some((k) => !keys.includes(k)) || keys.some((k) => t[k] === undefined)) e.push(`launch has exactly ${keys.join(", ")}`);
  if (!DEC.test(t.max_dev_buy_lamports ?? "")) e.push("launch.max_dev_buy_lamports is a decimal string of lamports");
  const int = (k: string, lo: number, hi: number) => { if (!Number.isInteger(t[k]) || t[k] < lo || t[k] > hi) e.push(`launch.${k} is an integer in ${lo}..${hi}`); };
  int("max_per_week", 1, 7);
  int("min_hours_between", 24, 720);
  int("consider_every_h", 6, 168);
  if (typeof t.trade_own !== "boolean") e.push("launch.trade_own is a boolean");
  if (typeof t.theme !== "string" || t.theme.length > 300) e.push("launch.theme is text of at most 300 characters");
  return e;
}

export function validateTrade(t: any): string[] {
  const e: string[] = [];
  if (!t || typeof t !== "object") return ["trade is an object"];
  const keys = ["venue", "mints", "max_order_lamports", "max_position_lamports", "daily_loss_stop_lamports", "cooldown_s", "max_orders_per_hour", "slippage_bps", "max_chase_bps", "stance_every_min", "buy_min_conviction", "sell_min_conviction", "strategy", "take_profit_bps", "stop_loss_bps", "real_swaps"];
  if (Object.keys(t).some((k) => !keys.includes(k))) e.push(`trade has exactly ${keys.join(", ")}`);
  if (t.venue !== "pumpswap") e.push("trade.venue is pumpswap (trades are on Solana)");
  if (!Array.isArray(t.mints) || t.mints.length < 1 || t.mints.length > 4 || new Set(t.mints).size !== t.mints.length || t.mints.some((m: unknown) => typeof m !== "string" || !B58.test(m) || b58decode(m).length !== 32)) e.push("trade.mints is 1..4 distinct base58 mints");
  const lam = (k: string, lo: bigint) => { if (!DEC.test(t[k] ?? "") || BigInt(t[k]) < lo) e.push(`trade.${k} is a decimal string of lamports >= ${lo}`); };
  lam("max_order_lamports", 1_000_000n);
  lam("max_position_lamports", 1_000_000n);
  lam("daily_loss_stop_lamports", 0n);
  if (DEC.test(t.max_order_lamports ?? "") && DEC.test(t.max_position_lamports ?? "") && BigInt(t.max_order_lamports) > BigInt(t.max_position_lamports)) e.push("trade.max_order_lamports is at most max_position_lamports");
  const int = (k: string, lo: number, hi: number) => { if (!Number.isInteger(t[k]) || t[k] < lo || t[k] > hi) e.push(`trade.${k} is an integer in ${lo}..${hi}`); };
  int("cooldown_s", 30, 86_400);
  int("max_orders_per_hour", 1, 60);
  int("slippage_bps", 10, 2_000);
  int("max_chase_bps", 0, 5_000);
  int("stance_every_min", 5, 1_440);
  int("buy_min_conviction", 1, 5);
  int("sell_min_conviction", 1, 5);
  if (typeof t.strategy !== "string" || t.strategy.length > 600) e.push("trade.strategy is text of at most 600 characters");
  if (t.take_profit_bps !== undefined) int("take_profit_bps", 50, 100_000);
  if (t.stop_loss_bps !== undefined) int("stop_loss_bps", 50, 9_900);
  if (t.real_swaps !== undefined && typeof t.real_swaps !== "boolean") e.push("trade.real_swaps is a boolean");
  return e;
}

/** Full check of a signed spec: fields, hash, operator signature. */
export function verifySigned(x: SignedSpec): { ok: boolean; problems: string[] } {
  const problems = validateSpec(x?.spec);
  if (problems.length) return { ok: false, problems };
  const h = specSha256(x.spec);
  if (x.sha256 !== h) problems.push(`sha256 is ${h}, the document says ${x.sha256}`);
  if (x.swarm) {
    if (!isSovereign(x.spec)) problems.push("a swarm-signed spec is a sovereign v2 spec");
    problems.push(...checkProof(x.swarm, x.spec.agent, x.spec.operator, h, verifySig));
    return { ok: problems.length === 0, problems };
  }
  let sigOk = false;
  try {
    sigOk = /^[0-9a-f]{128}$/.test(x.sig) && verifySig(b58decode(x.spec.operator), Buffer.from(specMessage(x.spec, h), "utf8"), Buffer.from(x.sig, "hex"));
  } catch {
    sigOk = false;
  }
  if (!sigOk) problems.push("the operator signature does not verify");
  return { ok: problems.length === 0, problems };
}

/** The cadre-profile-v1 blob that commits the spec (canonical bytes, so every writer gets the same blob id). */
export function profileBlob(x: SignedSpec, self: AgentSelf | null = null): Buffer {
  const s = x.spec;
  return Buffer.from(canonical({
    ...(self ? { agent_self: self } : {}),
    format: "cadre-profile-v1",
    bio: s.persona.slice(0, 600),
    persona: s.persona,
    appearance: s.appearance,
    tags: s.topics.slice(0, 16),
    links: [],
    model: s.model,
    operator: s.operator,
    agent_spec: s,
    agent_spec_sha256: x.sha256,
    agent_spec_sig: x.sig,
    agent_spec_swarm: x.swarm,
  }), "utf8");
}

/** What a v2 agent wrote about itself (evolve): signed on chain by the agent's own SetProfile, so it
 * is the agent's statement, not the creator's. Bounded in size; never changes the bounds. */
export interface AgentSelf {
  format: "cadre-agent-self-v1";
  strategy: string | null; // <= 600
  persona_notes: string | null; // <= 600, appended to the creator's persona
  goals: string[] | null; // <= 8 x 200
  updated_ms: number;
  decided_by: string; // the request id of the paid thought that wrote it
}

export function validateSelf(x: any): string[] {
  const e: string[] = [];
  if (!x || x.format !== "cadre-agent-self-v1") return ["format is cadre-agent-self-v1"];
  if (x.strategy !== null && (typeof x.strategy !== "string" || x.strategy.length > 600)) e.push("strategy is null or <= 600 characters");
  if (x.persona_notes !== null && (typeof x.persona_notes !== "string" || x.persona_notes.length > 600)) e.push("persona_notes is null or <= 600 characters");
  if (x.goals !== null && (!Array.isArray(x.goals) || x.goals.length > 8 || x.goals.some((g: unknown) => typeof g !== "string" || g.length > 200))) e.push("goals is null or <= 8 texts of <= 200 characters");
  if (!Number.isInteger(x.updated_ms) || typeof x.decided_by !== "string" || x.decided_by.length > 128) e.push("updated_ms and decided_by are required");
  return e;
}

/** Profile flags for the spec: is_agent, trades when trading is allowed, accepts_hires when it offers services. */
export function profileFlags(s: AgentSpec): number {
  return 1 | (s.actions.offer ? 4 : 0) | (s.actions.trade ? 8 : 0);
}
