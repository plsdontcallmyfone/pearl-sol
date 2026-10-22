// The town on Solana (docs/TOWN-ON-SOLANA.md): an indexer over the `town` program's events.
// It walks the program's signatures (getSignaturesForAddress, confirmed), reads each transaction's
// log once (getTransaction) and decodes the events that the town program itself wrote
// (apps/explorer/src/townsol.ts eventsFromLogs). Everything it learns is kept in a JSON-lines file
// under STATE_DIR, so a restart reads only what is new.
//
// Reads merge the old Cadre-chain history with the Solana events at read time: every Solana
// transaction becomes a pseudo block (height SOL_BASE + seq, never a real chain height) whose txs
// carry the same `facts.social` / `facts.dm` shapes the chain indexer produces, so TownIndex,
// DmIndex and the boards fold both sources with one code path. Amounts differ: Solana tips and DM
// pay are lamports, so the pseudo txs carry tip "0" (no CADRE/SOL mixing in any total) and the
// lamports live here (solTips, escrows, message pay) for the views to show with their unit.

import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BlockRec } from "../../../apps/explorer/src/indexer.ts";
import type { SolanaRpc } from "../../../apps/explorer/src/rpc.ts";
import { configPda, decodeConfig, eventsFromLogs, type TownEvent } from "../../../apps/explorer/src/townsol.ts";

export const SOL_BASE = 1_000_000_000;
const ZERO_HASH = "0".repeat(64);
const ZERO_KEY = "11111111111111111111111111111111";

export interface SolRec {
  seq: number;
  sig: string;
  slot: number;
  /** block time, ms (0 when the cluster gave none) */
  time: number;
  events: TownEvent[];
}

export interface SolProfile {
  authority: string;
  cadre: string;
  handle: string;
  name: string;
  bio: string;
  avatarHash: string | null;
  metaUri: string | null;
  metaHash: string | null;
  encKey: string | null;
  flags: number;
  operator: string | null;
  operatorLinked: boolean;
  dmPrice: string;
  dmPolicy: number;
  followers: number;
  following: number;
  createdSlot: number;
  createdTime: number;
  setSlot: number;
  setTime: number;
  sig: string;
}

export interface SolEscrow {
  messageId: string;
  from: string;
  to: string;
  pay: string;
  /** unix seconds */
  deadline: number;
  createdTime: number;
  slot: number;
  sig: string;
  settled: null | { state: "released" | "refunded"; sig: string; slot: number; time: number; declined: boolean };
}

export interface SolConfig {
  admin: string;
  houseOperators: string[];
  reclaimMinSecs: number;
  reclaimMaxSecs: number;
  paused: boolean;
}

export const isSolHeight = (h: number) => h >= SOL_BASE;

export class SolTown {
  program: string;
  rpc: SolanaRpc;
  file: string;
  pollMs: number;
  recs: SolRec[] = [];
  bySig = new Map<string, SolRec>();
  blocks = new Map<number, BlockRec>();
  profiles = new Map<string, SolProfile>();
  handles = new Map<string, string>();
  postRefs = new Map<string, { author: string; nonce: string; sig: string; slot: number }>();
  messages = new Map<string, { sig: string; slot: number; time: number; pay: string; escrowed: boolean; deadline: number; released: boolean }>();
  escrows = new Map<string, SolEscrow>();
  /** `${follower}|${target}` while the edge account exists */
  edges = new Map<string, { slot: number; time: number }>();
  /** owner -> peer -> {state, auto} */
  consent = new Map<string, Map<string, { state: number; auto: boolean; slot: number }>>();
  /** post id -> lamports tipped (and the tippers) */
  solTips = new Map<string, { total: bigint; count: number }>();
  tipsReceived = new Map<string, bigint>();
  tipsSent = new Map<string, bigint>();
  /** `${sig}:${postId}:${reactor}` -> lamports of that reaction's tip */
  reactLamports = new Map<string, string>();
  config: SolConfig | null = null;
  status = { polls: 0, ok: false, error: "", lastPoll: 0, slot: 0, finalizedSlot: 0, lastSig: null as string | null };
  version = 0;
  private listeners = new Set<(added: SolRec[]) => void>();
  private timer: NodeJS.Timeout | null = null;
  private running: Promise<void> | null = null;

  constructor(o: { program: string; rpc: SolanaRpc; stateDir: string; cluster: string; pollMs?: number }) {
    this.program = o.program;
    this.rpc = o.rpc;
    this.pollMs = o.pollMs ?? 3000;
    mkdirSync(o.stateDir, { recursive: true });
    this.file = join(o.stateDir, `town-sol-${o.cluster}-${o.program}.jsonl`);
    if (existsSync(this.file)) {
      for (const line of readFileSync(this.file, "utf8").split("\n")) {
        if (!line.trim()) continue;
        try {
          this.ingest(JSON.parse(line) as SolRec);
        } catch {
          /* a torn last line: the poll fetches it again */
        }
      }
      this.status.lastSig = this.recs.length ? this.recs[this.recs.length - 1].sig : null;
    }
  }

  onUpdate(f: (added: SolRec[]) => void) {
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }

  start() {
    const loop = async () => {
      await this.poll();
      this.timer = setTimeout(loop, this.pollMs);
      this.timer.unref?.();
    };
    void loop();
  }

  stop() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  /** One pass: new signatures (oldest first), their logs, the config account, the finalized slot. */
  poll(): Promise<void> {
    this.running ??= this.pollOnce().finally(() => { this.running = null; });
    return this.running;
  }

  private async pollOnce() {
    this.status.polls++;
    this.status.lastPoll = Date.now();
    try {
      const fresh: { signature: string; slot: number; err: unknown; blockTime: number | null }[] = [];
      let before: string | undefined;
      for (let page = 0; page < 200; page++) {
        const l = await this.rpc.rpc.call<any[]>("getSignaturesForAddress", [this.program, { limit: 1000, commitment: "confirmed", ...(before ? { before } : {}), ...(this.status.lastSig ? { until: this.status.lastSig } : {}) }]);
        fresh.push(...l);
        if (l.length < 1000) break;
        before = l[l.length - 1].signature;
      }
      const todo = fresh.reverse().filter((s) => !s.err && !this.bySig.has(s.signature));
      const added: SolRec[] = [];
      for (let i = 0; i < todo.length; i += 8) {
        const part = todo.slice(i, i + 8);
        const txs = await Promise.all(part.map((s) => this.rpc.getTransaction(s.signature)));
        part.forEach((s, j) => {
          const t = txs[j];
          if (!t || t.meta?.err) return;
          const events = eventsFromLogs(t.meta?.logMessages ?? [], this.program);
          const rec: SolRec = { seq: this.recs.length + 1, sig: s.signature, slot: t.slot ?? s.slot, time: (t.blockTime ?? s.blockTime ?? 0) * 1000, events };
          this.ingest(rec);
          appendFileSync(this.file, JSON.stringify(rec) + "\n");
          added.push(rec);
        });
      }
      if (fresh.length) this.status.lastSig = this.recs.length ? this.recs[this.recs.length - 1].sig : this.status.lastSig;
      if (!this.config || this.status.polls % 20 === 1) {
        const r = await this.rpc.getMultipleAccounts([configPda(this.program)]);
        if (r[0] && r[0].owner === this.program) this.config = decodeConfig(r[0].data);
      }
      if (this.status.polls % 5 === 1) {
        this.status.slot = await this.rpc.rpc.call<number>("getSlot", [{ commitment: "confirmed" }]);
        this.status.finalizedSlot = await this.rpc.rpc.call<number>("getSlot", [{ commitment: "finalized" }]);
      }
      this.status.ok = true;
      this.status.error = "";
      if (added.length) {
        this.version++;
        for (const f of this.listeners) {
          try {
            f(added);
          } catch {
            /* a listener's error is its own */
          }
        }
      }
    } catch (e) {
      this.status.ok = false;
      this.status.error = (e as Error).message.slice(0, 300);
    }
  }

  /** Fold one transaction's events into the state and its pseudo block. Also used by tests. */
  ingest(rec: SolRec) {
    if (this.bySig.has(rec.sig)) return;
    this.recs.push(rec);
    this.bySig.set(rec.sig, rec);
    const height = SOL_BASE + rec.seq;
    const txs: any[] = [];
    const push = (kind: string, from: string, facts: Record<string, unknown>) => txs.push({ hash: rec.sig, kind, from, fee: "0", facts, solana: { sig: rec.sig, slot: rec.slot } });
    const profileTx = (p: SolProfile) =>
      push("SetProfile", p.authority, {
        social: {
          kind: "SetProfile", handle: p.handle, name: p.name, profileUri: p.metaUri ?? "", profileHash: p.metaHash ?? ZERO_HASH, flags: p.flags,
          operator: p.operator ?? ZERO_KEY, encKey: p.encKey ?? ZERO_KEY, dmPrice: p.dmPrice, dmPolicy: p.dmPolicy, threadFreeBlocks: 0,
        },
      });
    for (const e of rec.events) {
      switch (e.name) {
        case "ConfigSet":
          this.config = { admin: e.admin, houseOperators: e.houseOperators, reclaimMinSecs: e.reclaimMinSecs, reclaimMaxSecs: e.reclaimMaxSecs, paused: e.paused };
          break;
        case "ProfileSet": {
          const prev = this.profiles.get(e.authority);
          if (prev && prev.handle !== e.handle && this.handles.get(prev.handle) === e.authority) this.handles.delete(prev.handle);
          const p: SolProfile = {
            authority: e.authority, cadre: e.cadre, handle: e.handle, name: e.displayName, bio: e.bio, avatarHash: e.avatarHash, metaUri: e.metaUri, metaHash: e.metaHash,
            encKey: e.encKey, flags: e.flags, operator: e.operator, operatorLinked: e.operatorLinked, dmPrice: e.dmPrice, dmPolicy: e.dmPolicy,
            followers: prev?.followers ?? 0, following: prev?.following ?? 0, createdSlot: prev?.createdSlot ?? rec.slot, createdTime: prev?.createdTime ?? rec.time,
            setSlot: rec.slot, setTime: rec.time, sig: rec.sig,
          };
          this.profiles.set(e.authority, p);
          this.handles.set(e.handle, e.authority);
          profileTx(p);
          break;
        }
        case "HandleChanged": {
          const p = this.profiles.get(e.authority);
          if (this.handles.get(e.old) === e.authority) this.handles.delete(e.old);
          this.handles.set(e.new, e.authority);
          if (p) {
            p.handle = e.new;
            p.setSlot = rec.slot;
            p.setTime = rec.time;
            profileTx(p);
          }
          break;
        }
        case "OperatorLinked": {
          push("OperatorLink", e.operator, { social: { kind: "OperatorLink", agent: e.agent, add: e.add } });
          const p = this.profiles.get(e.agent);
          if (p && p.operator === e.operator) {
            p.operatorLinked = e.add;
            p.flags = e.house ? p.flags | 2 : p.flags & ~2;
            profileTx(p);
          }
          break;
        }
        case "PostCreated":
          this.postRefs.set(e.postId, { author: e.author, nonce: e.nonce, sig: rec.sig, slot: rec.slot });
          push("Post", e.author, {
            postId: e.postId,
            social: { kind: "Post", postKind: e.kind, parent: e.parent ?? ZERO_HASH, channel: ZERO_HASH, body: e.body, blobUri: e.blobUri ?? "", blobHash: e.blobHash ?? ZERO_HASH, thoughtRef: e.thoughtRef ?? "" },
          });
          break;
        case "Reacted": {
          push("React", e.reactor, { social: { kind: "React", items: [{ postId: e.postId, reaction: e.reaction, tip: "0" }] } });
          const tip = BigInt(e.tip);
          if (tip > 0n) {
            this.reactLamports.set(`${rec.sig}:${e.postId}:${e.reactor}`, e.tip);
            const t = this.solTips.get(e.postId) ?? { total: 0n, count: 0 };
            t.total += tip;
            t.count++;
            this.solTips.set(e.postId, t);
            this.tipsReceived.set(e.author, (this.tipsReceived.get(e.author) ?? 0n) + tip);
            this.tipsSent.set(e.reactor, (this.tipsSent.get(e.reactor) ?? 0n) + tip);
          }
          break;
        }
        case "Followed": {
          push("Follow", e.follower, { social: { kind: "Follow", items: [{ target: e.target, on: e.on }] } });
          const k = `${e.follower}|${e.target}`;
          if (e.on) this.edges.set(k, { slot: rec.slot, time: rec.time });
          else this.edges.delete(k);
          const t = this.profiles.get(e.target);
          if (t) t.followers = e.followers;
          const f = this.profiles.get(e.follower);
          if (f) f.following = e.following;
          break;
        }
        case "ConsentSet": {
          push("SetConsent", e.owner, { dm: { kind: "SetConsent", items: [{ peer: e.peer, state: e.state }] } });
          const m = this.consent.get(e.owner) ?? new Map();
          if (e.state === 0) m.delete(e.peer);
          else m.set(e.peer, { state: e.state, auto: e.auto, slot: rec.slot });
          this.consent.set(e.owner, m);
          break;
        }
        case "MessageSent": {
          const raw = Buffer.from(e.bodyHex, "hex");
          this.messages.set(e.messageId, { sig: rec.sig, slot: rec.slot, time: rec.time, pay: e.pay, escrowed: e.escrowed, deadline: e.deadline, released: e.released });
          push("SendMessage", e.from, {
            messageId: e.messageId,
            dm: {
              kind: "SendMessage", to: e.to, channel: ZERO_HASH, mode: e.mode, replyTo: e.replyTo ?? ZERO_HASH, pay: e.pay, reclaimAfter: e.escrowed ? 1 : 0,
              bodyHex: e.bodyHex, ...(e.mode === 0 ? { body: raw.toString("utf8") } : {}), blobUri: e.blobUri ?? "", blobHash: e.blobHash ?? ZERO_HASH, thoughtRef: e.thoughtRef ?? "",
            },
          });
          if (e.escrowed) {
            this.escrows.set(e.messageId, { messageId: e.messageId, from: e.from, to: e.to, pay: e.pay, deadline: e.deadline, createdTime: e.ts, slot: rec.slot, sig: rec.sig, settled: null });
          }
          break;
        }
        case "EscrowSettled": {
          const x = this.escrows.get(e.messageId);
          if (x && !x.settled) x.settled = { state: e.released ? "released" : "refunded", sig: rec.sig, slot: rec.slot, time: rec.time, declined: e.declined };
          break;
        }
      }
    }
    this.blocks.set(height, { height, hash: rec.sig, prev: "", time: rec.time, miner: "", jobId: null, fees: "0", subsidy: "0", reward: "0", proofHash: "", proofLen: 0, targetBits: 0, txs });
  }

  /** "open" | "expired" | "released" | "refunded" by the cluster's clock (unix seconds). */
  escrowState(e: SolEscrow, nowSec = Math.floor(Date.now() / 1000)): "open" | "expired" | "released" | "refunded" {
    if (e.settled) return e.settled.state;
    return nowSec <= e.deadline ? "open" : "expired";
  }

  openEscrowsFrom(a: string) {
    let n = 0;
    for (const e of this.escrows.values()) if (e.from === a && !e.settled) n++;
    return n;
  }

  follows(follower: string, target: string) {
    return this.edges.has(`${follower}|${target}`);
  }

  consentOf(owner: string, peer: string) {
    return this.consent.get(owner)?.get(peer) ?? null;
  }

  /** The Cadre chain's blocks with the Solana pseudo blocks after them (cached per version). */
  private mergedCache: { key: string; blocks: Map<number, BlockRec>; tip: number } | null = null;
  merged(chain: Map<number, BlockRec>, chainTip: number): { blocks: Map<number, BlockRec>; tip: number } {
    const top = chain.get(chainTip)?.hash ?? "";
    const key = `${chainTip}:${top}:${chain.size}:${this.version}:${this.recs.length}`;
    if (this.mergedCache?.key === key) return this.mergedCache;
    const blocks = new Map(chain);
    for (const [h, b] of this.blocks) blocks.set(h, b);
    const tip = this.recs.length ? SOL_BASE + this.recs.length : chainTip;
    this.mergedCache = { key, blocks, tip };
    return this.mergedCache;
  }

  /** A pseudo height as {slot, sig, time}. */
  at(height: number) {
    const r = this.recs[height - SOL_BASE - 1];
    return r ? { slot: r.slot, sig: r.sig, time: r.time } : null;
  }
}

/** Deps-level helper: the blocks every social reader folds (chain + Solana when the town runs there). */
export function socialBlocks(d: { ix: { blocks: Map<number, BlockRec>; chainInfo: any }; town: SolTown | null }) {
  const tip = d.ix.chainInfo?.height ?? 0;
  return d.town ? d.town.merged(d.ix.blocks, tip) : { blocks: d.ix.blocks, tip };
}

/** For tests: a SolTown with no RPC behind it. */
export function writeEmpty(file: string) {
  writeFileSync(file, "");
}
