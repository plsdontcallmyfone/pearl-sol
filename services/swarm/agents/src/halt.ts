// The protocol halt record (contracts/halt, docs/ECOSYSTEM.md 7.8): one on-chain switch, signed by
// the halt authority, that every agent runner reads before it acts. The hosted runtime
// (cadre-agents) and the self-hosted runner (cadre-agent) both use this watcher; the trading engine
// gets it through the runner's halt() callback, so a halt stops orders too.
//
// Scope bits: 1 = ALL (every agent action), 2 = TRADE (no orders), 4 = LAUNCH (no token launches).
// Reads go through the Cadre node's queryContract every `everyMs`. If the contract is configured
// but cannot be read for longer than `maxStaleMs`, the watcher fails CLOSED (it reports a halt),
// because an agent that cannot read the switch cannot know it may act.
//
// Said plainly: hosted agents are stopped by this (the runtime is ours). A self-hosted runner obeys
// it as shipped, but whoever runs one holds the key and can change the code to ignore it, so for
// self-hosted agents the halt is advisory.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { b58encode } from "../../src/b58.ts";

export const HALT = { all: 1, trade: 2, launch: 4 } as const;
export type HaltScope = keyof typeof HALT;

/** query input: method "status", no fields */
export const STATUS_ARGS = Buffer.concat([Buffer.of(6), Buffer.from("status")]);

export interface HaltRecord { scope: number; seq: bigint; height: bigint; authority: string; reason: string }

/** status() return data: scope u8 | seq u64 | height u64 | authority [32] | reason (u32 len + UTF-8). */
export function decodeHalt(b: Uint8Array): HaltRecord {
  const x = Buffer.from(b);
  if (x.length < 53) throw new Error(`halt status is ${x.length} bytes, at least 53 expected`);
  const n = x.readUInt32LE(49);
  if (x.length !== 53 + n) throw new Error("halt status length mismatch");
  return { scope: x[0], seq: x.readBigUInt64LE(1), height: x.readBigUInt64LE(9), authority: b58encode(x.subarray(17, 49)), reason: x.subarray(53).toString("utf8") };
}

const here = dirname(fileURLToPath(import.meta.url));

/** Where the halt contract is: env (address, or "off"), else deploy/halt-devnet.json in the repo. */
export function haltContractFrom(env: Record<string, string | undefined>, file = join(here, "../../../../deploy/halt-devnet.json")): string | null {
  const v = env.AGENTS_HALT_CONTRACT ?? env.CADRE_HALT_CONTRACT;
  if (v === "off") return null;
  if (v) return v;
  try {
    if (existsSync(file)) return JSON.parse(readFileSync(file, "utf8")).contract ?? null;
  } catch { /* unreadable: no contract */ }
  return null;
}

export interface HaltWatchOpts {
  contract: string | null;
  query: (contract: string, argsHex: string) => Promise<{ ok: boolean; return?: string; error?: string }>;
  everyMs?: number;
  maxStaleMs?: number;
  now?: () => number;
  log?: (e: string, x: Record<string, unknown>) => void;
}

export class HaltWatch {
  o: HaltWatchOpts;
  rec: HaltRecord | null = null;
  readAt = 0; // last good read
  triedAt = Number.NEGATIVE_INFINITY;
  lastError = "";
  startedAt: number;
  constructor(o: HaltWatchOpts) {
    this.o = o;
    this.startedAt = this.now();
  }
  now() { return this.o.now ? this.o.now() : Date.now(); }

  /** Read the record if the last read is older than everyMs (default 15 s). */
  async refresh(force = false): Promise<void> {
    if (!this.o.contract) return;
    const t = this.now();
    if (!force && t - this.triedAt < (this.o.everyMs ?? 15_000)) return;
    this.triedAt = t;
    try {
      const r = await this.o.query(this.o.contract, STATUS_ARGS.toString("hex"));
      if (!r?.ok) throw new Error(`queryContract failed: ${r?.error ?? "no result"}`);
      const rec = decodeHalt(Buffer.from(r.return ?? "", "hex"));
      if (!this.rec || rec.seq !== this.rec.seq) this.o.log?.("halt_record", { contract: this.o.contract, scope: rec.scope, seq: rec.seq.toString(), reason: rec.reason });
      this.rec = rec;
      this.readAt = t;
      this.lastError = "";
    } catch (e) {
      this.lastError = (e as Error).message.slice(0, 200);
    }
  }

  /** "" when `what` may run now, else the reason it may not. */
  blocks(what: HaltScope): string {
    if (!this.o.contract) return "";
    const stale = this.now() - (this.readAt || this.startedAt);
    if (stale > (this.o.maxStaleMs ?? 120_000)) return `the protocol halt record could not be read for ${Math.round(stale / 1000)} s (${this.lastError || "no read yet"}); failing closed`;
    if (!this.rec) return ""; // within the grace period after start
    const s = this.rec.scope;
    if (s & HALT.all) return `protocol halt (seq ${this.rec.seq}): ${this.rec.reason || "no reason given"}`;
    if (what !== "all" && s & HALT[what]) return `protocol halt of ${what} (seq ${this.rec.seq}): ${this.rec.reason || "no reason given"}`;
    return "";
  }

  status() {
    return {
      contract: this.o.contract, scope: this.rec?.scope ?? null, seq: this.rec?.seq.toString() ?? null, reason: this.rec?.reason ?? null,
      read_at: this.readAt || null, error: this.lastError || null, blocks_all: this.blocks("all") || null,
    };
  }
}
