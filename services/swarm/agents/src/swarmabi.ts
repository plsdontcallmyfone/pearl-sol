// The swarms contract's byte formats (contracts/swarms/src/lib.rs, docs/SWARMS.md), shared by the
// hosted runtime (services/swarm/agents) and cadre-api: the definition hash every member accepts,
// the messages members and creators sign, call args, and decoders for its query replies. The
// Rust contract is the reference; test/swarmabi.test.ts pins a vector the contract tests also pin.
import { readFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { b58decode, b58encode } from "../../src/b58.ts";
import { blake3 } from "../../src/blake3.ts";

/** Canonical JSON (sorted keys, no whitespace), the same function as spec.ts (copied: no import cycle). */
function canonical(v: unknown): string {
  if (v === null || typeof v !== "object") return JSON.stringify(v);
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  const o = v as Record<string, unknown>;
  return `{${Object.keys(o).filter((k) => o[k] !== undefined).sort().map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`).join(",")}}`;
}

export const MAX_CONTRACT_MEMBERS = 32;
/** Members one POST /v1/hosted/swarms may create (docs/SWARMS.md 3: request size, one Solana tx for every member's SOL, runtime load). */
export const MAX_BATCH = 12;
export const MAX_NAME = 64;
export const MAX_ROLE = 32;
export const MAX_GOAL = 2000;
export const GOAL_FORMAT = "cadre-swarm-goal-v1";
export const CREATOR_NOTE = "This creates a public grouping of sovereign agents. It gives you no control over them and moves no funds.";
export const KINDS = ["general", "trading"] as const;
export type SwarmKind = (typeof KINDS)[number];

const enc = (s: string) => Buffer.from(s, "utf8");
const u32 = (v: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); return b; };
const u64 = (v: bigint | number) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); return b; };
const key = (a: string | Uint8Array) => { const k = typeof a === "string" ? b58decode(a) : a; if (k.length !== 32) throw new Error("address must be 32 bytes"); return Buffer.from(k); };
const withLen = (b: Uint8Array) => Buffer.concat([u32(b.length), Buffer.from(b)]);
export const hexOf = (b: Uint8Array) => Buffer.from(b).toString("hex");

export interface DefInput {
  contract: string;
  creator: string;
  name: string;
  goalHash: string; // hex
  goalUri: string;
  lead: string;
  members: { agent: string; role: string }[];
  salt: bigint;
}

/** The goal blob (canonical JSON): its blake3 is goal_hash, `blob:<hex>` is goal_uri. `specs` lists
 * every member's spec sha256 in member order, so the creator's one signature over the definition
 * also covers every member spec (docs/SWARMS.md 3). */
export function goalBlob(goal: string, kind: SwarmKind, name: string, specs?: string[]): { bytes: Buffer; hash: string; uri: string; json: string } {
  const json = canonical({ format: GOAL_FORMAT, goal, kind, name, specs });
  const bytes = enc(json);
  const hash = hexOf(blake3(bytes));
  return { bytes, hash, uri: `blob:${hash}`, json };
}

export function definitionBytes(d: DefInput): Buffer {
  const parts: Buffer[] = [enc("cadre-swarm-def-v1"), key(d.contract), key(d.creator), withLen(enc(d.name)), Buffer.from(d.goalHash, "hex"), withLen(enc(d.goalUri)), key(d.lead), Buffer.from([d.members.length])];
  for (const m of d.members) parts.push(key(m.agent), withLen(enc(m.role)));
  parts.push(u64(d.salt));
  return Buffer.concat(parts);
}

export const definitionHash = (d: DefInput) => hexOf(blake3(definitionBytes(d)));
export const acceptMessage = (contract: string, def: string) => Buffer.concat([enc("cadre-swarm-accept-v1"), key(contract), Buffer.from(def, "hex")]);
export const joinMessage = (contract: string, proposal: bigint | number) => Buffer.concat([enc("cadre-swarm-join-v1"), key(contract), u64(proposal)]);
export const creatorMessage = (contract: string, def: string) => `Cadre swarm v1\ncontract: ${hexOf(key(contract))}\ndefinition: ${def}\n${CREATOR_NOTE}`;

/** Field checks shared by the runtime and cadre-api (the contract checks them again). */
export function checkDefinition(x: { name: string; goal: string; kind: string; roles: string[]; leadIndex: number; agents: string[] }): string[] {
  const e: string[] = [];
  const bytes = (s: string) => Buffer.byteLength(s, "utf8");
  if (typeof x.name !== "string" || bytes(x.name) < 1 || bytes(x.name) > MAX_NAME) e.push("swarm.name is 1..64 bytes");
  if (typeof x.goal !== "string" || !x.goal.trim() || x.goal.length > MAX_GOAL) e.push(`swarm.goal is 1..${MAX_GOAL} characters`);
  if (!(KINDS as readonly string[]).includes(x.kind)) e.push(`swarm.kind is ${KINDS.join(" or ")}`);
  if (x.agents.length < 1 || x.agents.length > MAX_BATCH) e.push(`a swarm is created with 1..${MAX_BATCH} members in one request`);
  if (x.roles.some((r) => typeof r !== "string" || bytes(r) < 1 || bytes(r) > MAX_ROLE)) e.push("every role is 1..32 bytes");
  if (!Number.isInteger(x.leadIndex) || x.leadIndex < 0 || x.leadIndex >= x.agents.length) e.push("swarm.lead is the index of one member");
  if (new Set(x.agents).size !== x.agents.length) e.push("a member is named twice");
  return e;
}

// ------------------------------------------------------------------ call args

export interface CreateMember { agent: string; role: string; amount: bigint; sig: Uint8Array }

function method(name: string) { const b = enc(name); return Buffer.concat([Buffer.from([b.length]), b]); }

export function createArgs(d: Omit<DefInput, "contract" | "creator" | "members">, members: CreateMember[], signed?: { creator: string; sig: Uint8Array }): Buffer {
  const parts: Buffer[] = [method(signed ? "create_signed" : "create"), withLen(enc(d.name)), Buffer.from(d.goalHash, "hex"), withLen(enc(d.goalUri)), key(d.lead), u64(d.salt), Buffer.from([members.length])];
  for (const m of members) {
    if (m.sig.length !== 64) throw new Error("an acceptance signature is 64 bytes");
    parts.push(key(m.agent), withLen(enc(m.role)), u64(m.amount), Buffer.from(m.sig));
  }
  if (signed) {
    if (signed.sig.length !== 64) throw new Error("the creator signature is 64 bytes");
    parts.push(key(signed.creator), Buffer.from(signed.sig));
  }
  return Buffer.concat(parts);
}

export const callArgs = {
  propose: (swarm: bigint | number, agent: string, role: string) => Buffer.concat([method("propose"), u64(swarm), key(agent), withLen(enc(role))]),
  cancel: (p: bigint | number) => Buffer.concat([method("cancel"), u64(p)]),
  decline: (p: bigint | number) => Buffer.concat([method("decline"), u64(p)]),
  accept: (p: bigint | number) => Buffer.concat([method("accept"), u64(p)]),
  acceptSigned: (p: bigint | number, sig: Uint8Array) => Buffer.concat([method("accept_signed"), u64(p), Buffer.from(sig)]),
  leave: (swarm: bigint | number) => Buffer.concat([method("leave"), u64(swarm)]),
  handLead: (swarm: bigint | number, to: string) => Buffer.concat([method("hand_lead"), u64(swarm), key(to)]),
  takeLead: (swarm: bigint | number) => Buffer.concat([method("take_lead"), u64(swarm)]),
  fund: (swarm: bigint | number, pays: { agent: string; amount: bigint }[]) => Buffer.concat([method("fund"), u64(swarm), Buffer.from([pays.length]), ...pays.flatMap((p) => [key(p.agent), u64(p.amount)])]),
};

export const queryArgs = {
  counts: () => method("counts"),
  swarm: (id: bigint | number) => Buffer.concat([method("swarm"), u64(id)]),
  members: (id: bigint | number, offset = 0, limit = 32) => Buffer.concat([method("members"), u64(id), u32(offset), Buffer.from([limit])]),
  member: (id: bigint | number, agent: string) => Buffer.concat([method("member"), u64(id), key(agent)]),
  byMember: (agent: string, offset = 0, limit = 64) => Buffer.concat([method("by_member"), key(agent), u32(offset), Buffer.from([limit])]),
  byCreator: (creator: string, offset = 0, limit = 64) => Buffer.concat([method("by_creator"), key(creator), u32(offset), Buffer.from([limit])]),
  proposal: (p: bigint | number) => Buffer.concat([method("proposal"), u64(p)]),
  proposals: (id: bigint | number, offset = 0, limit = 64) => Buffer.concat([method("proposals"), u64(id), u32(offset), Buffer.from([limit])]),
  byDefinition: (def: string) => Buffer.concat([method("by_definition"), Buffer.from(def, "hex")]),
};

// ------------------------------------------------------------------ decoders

class R {
  o = 0;
  b: Buffer;
  constructor(b: Uint8Array | string) { this.b = typeof b === "string" ? Buffer.from(b, "hex") : Buffer.from(b); }
  u8() { return this.b[this.o++]; }
  u32() { const v = this.b.readUInt32LE(this.o); this.o += 4; return v; }
  u64() { const v = this.b.readBigUInt64LE(this.o); this.o += 8; return v; }
  addr() { const v = this.b.subarray(this.o, this.o + 32); this.o += 32; return v.every((x) => x === 0) ? null : b58encode(v); }
  hash() { const v = this.b.subarray(this.o, this.o + 32).toString("hex"); this.o += 32; return v; }
  str() { const n = this.u32(); const v = this.b.subarray(this.o, this.o + n).toString("utf8"); this.o += n; return v; }
}

export interface SwarmRec { creator: string; lead: string | null; successor: string | null; definition: string; goal_hash: string; height: number; slots: number; active: number; proposals: number; name: string; goal_uri: string }
export function decodeSwarm(b: Uint8Array | string): SwarmRec {
  const r = new R(b);
  return { creator: r.addr()!, lead: r.addr(), successor: r.addr(), definition: r.hash(), goal_hash: r.hash(), height: Number(r.u64()), slots: r.u32(), active: r.u32(), proposals: r.u32(), name: r.str(), goal_uri: r.str() };
}

export interface SlotRec { agent: string; role: string; joined_height: number; left_height: number | null; via_proposal: number | null }
export function decodeMembers(b: Uint8Array | string): { total: number; slots: SlotRec[] } {
  const r = new R(b);
  const total = r.u32();
  const n = r.u8();
  const slots: SlotRec[] = [];
  for (let i = 0; i < n; i++) {
    const agent = r.addr()!;
    const role = r.str();
    const joined = Number(r.u64());
    const left = Number(r.u64());
    const via = Number(r.u64());
    slots.push({ agent, role, joined_height: joined, left_height: left || null, via_proposal: via || null });
  }
  return { total, slots };
}

export function decodeIds(b: Uint8Array | string): { total: number; ids: number[] } {
  const r = new R(b);
  const total = r.u32();
  const n = r.u8();
  return { total, ids: Array.from({ length: n }, () => Number(r.u64())) };
}

export const PROPOSAL_STATES = ["open", "accepted", "cancelled", "declined"] as const;
export interface ProposalRec { id: number; swarm: number; agent: string; proposer: string; state: (typeof PROPOSAL_STATES)[number]; height: number; closed_height: number | null; role: string }
export function decodeProposal(id: number, b: Uint8Array | string): ProposalRec {
  const r = new R(b);
  const swarm = Number(r.u64());
  const agent = r.addr()!;
  const proposer = r.addr()!;
  const st = r.u8();
  const height = Number(r.u64());
  const closed = Number(r.u64());
  return { id, swarm, agent, proposer, state: PROPOSAL_STATES[st] ?? "open", height, closed_height: closed || null, role: r.str() };
}

export function decodeCounts(b: Uint8Array | string) {
  const r = new R(b);
  return { swarms: Number(r.u64()), proposals: Number(r.u64()), start_height: Number(r.u64()) };
}

export const decodeU64 = (b: Uint8Array | string) => Number(new R(b).u64());

// ------------------------------------------------------------------ deploy file

const here = dirname(fileURLToPath(import.meta.url));
export const REPO = join(here, "../../../..");

export interface SwarmsConfig { contract: string; start_height: number; code_hash: string | null; chain_id: number | null }

/** deploy/swarms-devnet.json (or CADRE_SWARMS_CONFIG / SWARMS_CONFIG); null when the contract is not deployed. */
export function loadSwarmsConfig(env: NodeJS.ProcessEnv = process.env): SwarmsConfig | null {
  const f = env.CADRE_SWARMS_CONFIG ?? env.SWARMS_CONFIG ?? join(REPO, "deploy/swarms-devnet.json");
  if (!existsSync(f)) return null;
  const j = JSON.parse(readFileSync(f, "utf8"));
  if (!j.contract) return null;
  return { contract: j.contract, start_height: j.start_height ?? 0, code_hash: j.code_hash ?? null, chain_id: j.chain_id ?? null };
}

// ------------------------------------------------------------------ practice styles

/** Trading swarms give each worker a different practice style: the strategies module's own list
 * (services/swarm/strategies/src/styles.ts; design choices, not measurements). */
export { STYLES } from "../../strategies/src/styles.ts";
export type StyleId = string;

// ------------------------------------------------------------------ the creator's one signature

/** What a member's profile blob carries instead of a per-spec signature: the swarm definition and
 * the creator's one signature over its creator message. Anyone recomputes goal_hash (which lists
 * every spec sha256), the definition hash and the message, and checks the signature. */
export interface SwarmSpecProof {
  format: "cadre-swarm-spec-proof-v1";
  contract: string;
  creator: string;
  name: string;
  kind: SwarmKind;
  goal: string;
  lead: string;
  members: { agent: string; role: string }[];
  salt: string;
  specs: string[];
  creator_sig: string;
}

export function proofDefinition(p: SwarmSpecProof) {
  const g = goalBlob(p.goal, p.kind, p.name, p.specs);
  const def = definitionHash({ contract: p.contract, creator: p.creator, name: p.name, goalHash: g.hash, goalUri: g.uri, lead: p.lead, members: p.members, salt: BigInt(p.salt) });
  return { goal: g, def, message: creatorMessage(p.contract, def) };
}

/** Problems with a proof for the spec `sha256` of agent `agent` signed by `creator` (empty = valid). */
export function checkProof(p: SwarmSpecProof, agent: string, creator: string, sha256: string, verify: (pk: Uint8Array, msg: Uint8Array, sig: Uint8Array) => boolean): string[] {
  const e: string[] = [];
  if (!p || p.format !== "cadre-swarm-spec-proof-v1") return ["swarm proof format is cadre-swarm-spec-proof-v1"];
  if (p.creator !== creator) e.push("the swarm proof names another creator than the spec's operator");
  const i = p.members.findIndex((m) => m.agent === agent);
  if (i < 0) e.push("the swarm proof does not list this agent");
  else if (p.specs[i] !== sha256) e.push("the swarm proof lists another spec sha256 for this agent");
  if (p.specs.length !== p.members.length) e.push("the swarm proof has one spec per member");
  try {
    const { message } = proofDefinition(p);
    if (!/^[0-9a-f]{128}$/.test(p.creator_sig) || !verify(b58decode(creator), Buffer.from(message, "utf8"), Buffer.from(p.creator_sig, "hex"))) e.push("the creator's swarm signature does not verify");
  } catch (x) {
    e.push(`the swarm proof does not encode: ${(x as Error).message}`);
  }
  return e;
}
