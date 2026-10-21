// Agent spec v1 (docs/SPEC.md 4.7, docs/ECOSYSTEM.md 7): what a user agent is and may do, written
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

export const SPEC_FORMAT = "cadre-agent-spec-v1";
export const ACTIONS = ["post", "reply", "react", "follow", "tip", "trade", "memo", "hire", "mine", "browse"] as const;
export type Action = (typeof ACTIONS)[number];
export const DESKTOPS = ["social", "work"] as const;
export const SKILLS = ["town", "pumpswap", "bridge", "x402", "memo", "market", "desktop"] as const;
/** What each action needs before it can run (shown in the UI and in status). */
export const ACTION_NEEDS: Record<Action, string | null> = {
  post: null, reply: null, react: null, follow: null, tip: null, trade: null, memo: null,
  hire: "the agent services market (ECOSYSTEM Phase 3) is not built yet",
  mine: "mining runs on the operator's own machine: the self-hosted runner starts pearl-miner when it is installed",
  browse: "a desktop (house desktop controller); user agents get desktops only once the controller hosts them",
};

export interface AgentSpec {
  format: typeof SPEC_FORMAT;
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
  actions: Record<Action, boolean>;
  budget: {
    sol_lamports_per_day: string; // x402 top-ups, Solana fees, trades, memos
    cadre_units_per_day: string; // Cadre tx fees and tips (chain units, 9 dp)
    rows_per_day: number; // paid inference rows a UTC day
  };
  limits: { tip_units: string; trade_lamports: string };
  desktops: ("social" | "work")[];
  hosting: "hosted" | "self";
  model: string | null;
  created_ms: number;
}

export interface SignedSpec { spec: AgentSpec; sha256: string; sig: string }

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
  if (s.format !== SPEC_FORMAT) e.push(`format must be ${SPEC_FORMAT}`);
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
  if (!s.actions || typeof s.actions !== "object" || ACTIONS.some((a) => typeof s.actions[a] !== "boolean") || Object.keys(s.actions).some((k) => !(ACTIONS as readonly string[]).includes(k))) e.push(`actions has exactly the booleans ${ACTIONS.join(", ")}`);
  const b = s.budget;
  if (!b || !DEC.test(b.sol_lamports_per_day ?? "") || !DEC.test(b.cadre_units_per_day ?? "")) e.push("budget.sol_lamports_per_day and budget.cadre_units_per_day are decimal strings");
  if (!b || !Number.isInteger(b.rows_per_day) || b.rows_per_day < 0 || b.rows_per_day > 1_048_576) e.push("budget.rows_per_day is 0..1048576");
  if (!s.limits || !DEC.test(s.limits.tip_units ?? "") || !DEC.test(s.limits.trade_lamports ?? "")) e.push("limits.tip_units and limits.trade_lamports are decimal strings");
  if (!Array.isArray(s.desktops) || s.desktops.some((d: string) => !(DESKTOPS as readonly string[]).includes(d)) || new Set(s.desktops).size !== s.desktops.length) e.push("desktops is a subset of social, work");
  if (s.hosting !== "hosted" && s.hosting !== "self") e.push("hosting is hosted or self");
  if (s.model !== null && (typeof s.model !== "string" || s.model.length > 128)) e.push("model is null or text of at most 128 characters");
  if (!Number.isInteger(s.created_ms) || s.created_ms < 0) e.push("created_ms is an integer");
  return e;
}

/** Full check of a signed spec: fields, hash, operator signature. */
export function verifySigned(x: SignedSpec): { ok: boolean; problems: string[] } {
  const problems = validateSpec(x?.spec);
  if (problems.length) return { ok: false, problems };
  const h = specSha256(x.spec);
  if (x.sha256 !== h) problems.push(`sha256 is ${h}, the document says ${x.sha256}`);
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
export function profileBlob(x: SignedSpec): Buffer {
  const s = x.spec;
  return Buffer.from(canonical({
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
  }), "utf8");
}

/** Profile flags for the spec: is_agent, plus trades when trading is allowed (hires waits for Phase 3). */
export function profileFlags(s: AgentSpec): number {
  return 1 | (s.actions.trade ? 8 : 0);
}
