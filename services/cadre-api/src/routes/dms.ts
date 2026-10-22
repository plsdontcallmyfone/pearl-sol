// DMs and channels (SPEC 4.10, docs/ECOSYSTEM.md Phase 2, docs/API.md "DMs and channels"):
// inboxes with primary / requests / blocked boxes, threads, messages with their pay-to-DM escrow,
// consent lists, channels with members and key epochs, an SSE inbox stream, and the unsigned DM
// tx builders (window.cadre JSON forms). Histories come from the explorer index
// (apps/explorer/src/dm.ts over the canonical blocks); gates, open escrows, counters and channel
// records are chain state (node RPC getDmGate, getMessageEscrow, getDmStats, getChannel, ...).
// The API never sees a key or the plaintext of a sealed message: sealed bodies pass as body_hex.

import type { Api, Ctx } from "../http.ts";
import { ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr, amount } from "./wallet.ts";
import { TownState, thoughtStatus } from "./town.ts";
import {
  ACTIONS, CONSENT_STATES, DmIndex, KEY_ENVELOPE_LEN, MODES, POLICIES, ZERO_HASH, ZERO_KEY, channelId, checkSealedChannel, checkSealedDm, decodeDmTx,
  dmFee, isUtf8, messageId, type ChannelRec, type DmBody, type EscrowRec, type MsgRec,
} from "../../../../apps/explorer/src/dm.ts";
import * as TS from "../../../../apps/explorer/src/townsol.ts";
import { SOL_BASE, isSolHeight, socialBlocks } from "../townsol.ts";
import { UNIT_NOTE, needTown, solBuilt, solGate } from "./towntx.ts";

export const METADATA_NOTE = "Everything on chain is public: a sealed message hides its text, never the sender, recipient (or channel), time, size or pay.";
const FEE_RULE = "max(min_fee, social_min_fee + social_byte_fee x encoded tx length) (SPEC 4.6, 4.10)";
const utf8len = (s: string) => Buffer.byteLength(s, "utf8");

// ------------------------------------------------------------ shared state per process

export class DmState {
  d: Deps;
  town: TownState;
  index = new DmIndex();
  pendingSeen = new Map<string, number>();
  pendingCache: { at: number; v: any[] } | null = null;
  constructor(d: Deps) {
    this.d = d;
    this.town = new TownState(d);
  }

  tip() {
    return this.d.ix.chainInfo?.height ?? 0;
  }

  /** The DM index over the chain's blocks plus the Solana town's pseudo blocks; Solana escrows take
   * their deadline (unix seconds) and settlement from the program's events, not from heights. */
  idx() {
    const sb = socialBlocks(this.d);
    const ix = this.index.update(sb.blocks, sb.tip);
    const town = this.d.town;
    if (town) {
      for (const se of town.escrows.values()) {
        const e = ix.escrows.get(se.messageId);
        if (!e) continue;
        e.deadline = se.deadline;
        e.pay = se.pay;
        e.settled = se.settled ? { state: se.settled.state, txHash: se.settled.sig, height: se.settled.slot, declined: se.settled.declined } : null;
      }
    }
    return ix;
  }

  /** Escrow state by its own clock: unix seconds on Solana, heights on the Cadre chain. */
  escState(e: EscrowRec) {
    const se = this.d.town?.escrows.get(e.messageId);
    return se ? this.d.town!.escrowState(se) : DmIndex.escrowState(e, this.tip());
  }

  card(a: string) {
    return this.town.idx().card(a);
  }

  /** SendMessage txs in the node's mempool ("sending" until inclusion), cached CADRE_PENDING_MS (default
   * 5 s), one read at a time; each pending tx is fetched once and kept while it stays in the mempool. */
  pendingTx = new Map<string, any>();
  private pendingLoad: Promise<any[]> | null = null;
  async pending(): Promise<any[]> {
    if (this.pendingCache && Date.now() - this.pendingCache.at < Number(process.env.CADRE_PENDING_MS ?? 5000)) return this.pendingCache.v;
    this.pendingLoad ??= this.pendingRead().finally(() => { this.pendingLoad = null; });
    return this.pendingLoad;
  }
  private async pendingRead(): Promise<any[]> {
    const out: any[] = [];
    try {
      const m = await this.d.node.call<any>("getMempool", {});
      const hashes: string[] = (m?.txs ?? []).slice(0, 200);
      const want = hashes.filter((h) => !this.pendingTx.has(h));
      const got = want.length ? await this.d.node.batch<any>(want.map((h) => ["getTx", { tx_hash: h }])) : [];
      want.forEach((h, i) => { if (!(got[i] instanceof Error)) this.pendingTx.set(h, got[i]); });
      for (const k of this.pendingTx.keys()) if (!hashes.includes(k)) this.pendingTx.delete(k);
      const res = hashes.map((h) => this.pendingTx.get(h) ?? null);
      const now = Date.now();
      const live = new Set(hashes);
      for (const k of this.pendingSeen.keys()) if (!live.has(k)) this.pendingSeen.delete(k);
      for (let i = 0; i < res.length; i++) {
        const t = res[i];
        if (t instanceof Error || !t || t.status !== "pending" || t.kind !== "SendMessage") continue;
        let st;
        try {
          st = decodeDmTx(Uint8Array.from(Buffer.from(t.tx, "base64")));
        } catch {
          continue;
        }
        const b = st.body;
        if (b.kind !== "SendMessage") continue;
        if (!this.pendingSeen.has(hashes[i])) this.pendingSeen.set(hashes[i], now);
        out.push({
          tx_hash: hashes[i], from: this.card(st.from), to: b.to === ZERO_KEY ? null : this.card(b.to), channel: b.channel === ZERO_HASH ? null : b.channel,
          mode: MODES[b.mode] ?? String(b.mode), body: b.mode === 0 ? b.body ?? null : null, body_hex: b.bodyHex ?? "", pay: b.pay, reclaim_after: b.reclaimAfter,
          reply_to: b.replyTo === ZERO_HASH ? null : b.replyTo, seen_ms: this.pendingSeen.get(hashes[i]), message_id: messageId(st.from, st.nonce), status: "sending",
        });
      }
    } catch {
      /* node unreachable: an empty lane, the reads still answer */
    }
    out.sort((a, b) => b.seen_ms - a.seen_ms);
    this.pendingCache = { at: Date.now(), v: out };
    return out;
  }

  /** A node DM RPC call: 503 `unavailable` when the node has no DM RPC, `node_unavailable` when unreachable. */
  async rpc<T = any>(method: string, params: unknown): Promise<T> {
    try {
      return await this.d.node.call<T>(method, params);
    } catch (e) {
      const m = (e as Error).message;
      if (/unknown method|method not found/i.test(m)) throw new ApiError(503, "unavailable", `this node does not serve ${method} (the DM upgrade, SPEC 4.10, is not deployed on it)`);
      if (/fetch failed|ECONNREFUSED|timeout|aborted/i.test(m)) throw new ApiError(503, "node_unavailable", `agent-chain node unreachable: ${m.slice(0, 160)}`);
      if (/missing|invalid|expected|not a|bad/i.test(m)) throw new ApiError(400, "node_rejected", m.slice(0, 300));
      throw new ApiError(502, "node_error", m.slice(0, 300));
    }
  }

  // ---------------------------------------------------------- views

  escrowView(e: EscrowRec) {
    const tip = this.tip();
    const se = this.d.town?.escrows.get(e.messageId);
    if (se) {
      const state = this.d.town!.escrowState(se);
      const now = Math.floor(Date.now() / 1000);
      return {
        message_id: e.messageId, from: e.from, to: e.to, pay: se.pay, pay_unit: "lamports", chain: "solana", height: se.slot, slot: se.slot, deadline: se.deadline, deadline_unit: "unix_seconds",
        blocks_left: null, seconds_left: se.settled ? 0 : Math.max(0, se.deadline - now), state,
        settled_tx: se.settled?.sig ?? null, settled_height: se.settled?.slot ?? null, declined: se.settled ? se.settled.declined : null,
      };
    }
    const state = DmIndex.escrowState(e, tip);
    return {
      message_id: e.messageId, from: e.from, to: e.to, pay: e.pay, height: e.height, deadline: e.deadline,
      blocks_left: e.settled ? 0 : Math.max(0, e.deadline - tip), state,
      settled_tx: e.settled?.txHash ?? null, settled_height: e.settled?.height ?? null, declined: e.settled ? e.settled.declined : null,
    };
  }

  async msgView(m: MsgRec) {
    const ix = this.idx();
    const tip = this.tip();
    const fin = this.d.ix.chainInfo?.finalized_height ?? 0;
    const plain = m.mode === 0;
    const r = plain && m.thoughtRef ? await this.town.receipt(m.thoughtRef) : null;
    const e = ix.escrows.get(m.messageId);
    const town = this.d.town;
    if (town && isSolHeight(m.height)) {
      const at = town.at(m.height)!;
      const fin2 = at.slot <= town.status.finalizedSlot;
      return {
        message_id: m.messageId, from: this.card(m.from), to: m.to ? this.card(m.to) : null, channel: null, mode: MODES[m.mode] ?? String(m.mode),
        body: plain ? m.body : null, body_hex: m.bodyHex, blob_uri: m.blobUri, reply_to: m.replyTo, pay: m.pay, pay_unit: "lamports", reclaim_after: null,
        thought_ref: m.thoughtRef, thought: plain ? thoughtStatus(m.body ?? "", m.thoughtRef, r) : thoughtStatus("", null, null),
        chain: "solana", height: at.slot, slot: at.slot, block_hash: null, tx_hash: at.sig, index: m.index, timestamp_ms: m.time,
        confirmations: Math.max(0, town.status.slot - at.slot), final: fin2, status: fin2 ? "final" : "delivered",
        escrow: e ? this.escrowView(e) : null, epoch: null,
      };
    }
    const final = m.height <= fin;
    return {
      message_id: m.messageId, from: this.card(m.from), to: m.to ? this.card(m.to) : null, channel: m.channel, mode: MODES[m.mode] ?? String(m.mode),
      body: plain ? m.body : null, body_hex: m.bodyHex, blob_uri: m.blobUri, reply_to: m.replyTo, pay: m.pay, reclaim_after: m.reclaimAfter,
      thought_ref: m.thoughtRef, thought: plain ? thoughtStatus(m.body ?? "", m.thoughtRef, r) : thoughtStatus("", null, null),
      chain: "cadre", pay_unit: "cadre_units", height: m.height, block_hash: m.blockHash, tx_hash: m.txHash, index: m.index, timestamp_ms: m.time,
      confirmations: Math.max(0, tip - m.height), final, status: final ? "final" : "delivered",
      escrow: e ? this.escrowView(e) : null, epoch: m.epoch,
    };
  }

  msgs(l: MsgRec[]) {
    return Promise.all(l.map((m) => this.msgView(m)));
  }

  /** The account's view of its thread with `peer`. */
  async threadView(account: string, peer: string) {
    const ix = this.idx();
    const town = this.town.idx();
    const l = ix.thread(account, peer);
    const consent = this.d.town?.consentOf(account, peer)?.state ?? ix.consentOf(account, peer);
    const wrote = l.some((m) => m.from === account);
    const follows = (town.follows.get(account)?.has(peer) ?? false) || !!this.d.town?.follows(account, peer);
    const box = consent === 2 ? "blocked" : wrote || follows || consent === 1 ? "primary" : "requests";
    let inn = 0, out = 0;
    for (const m of l) {
      const e = ix.escrows.get(m.messageId);
      if (!e || this.escState(e) !== "open") continue;
      if (e.to === account) inn++;
      else out++;
    }
    const last = l[l.length - 1];
    return {
      peer: this.card(peer), box, last: await this.msgView(last), count: l.length, sealed: last.mode === 1,
      open_escrows: { in: inn, out }, unanswered_paid: inn,
    };
  }

  async channelCard(id: string, chain?: any) {
    const ix = this.idx();
    const c: ChannelRec | undefined = ix.channels.get(id);
    const ch = chain === undefined ? await this.rpc<any>("getChannel", { channel: id }) : chain;
    if (!ch && !c) return null;
    const last = c?.messages[c.messages.length - 1];
    const creator = ch?.creator ?? c!.creator;
    const policy = ch?.policy ?? c!.policy;
    const mode = ch?.mode ?? c!.mode;
    return {
      channel_id: id, name: ch?.name ?? c!.name, about: ch?.about ?? c!.about, creator: this.card(creator), policy: POLICIES[policy] ?? String(policy),
      join_price: ch?.join_price ?? c!.joinPrice, mode: MODES[mode] ?? String(mode), members: ch?.members ?? c!.members.size,
      admins: ((ch?.admins ?? c!.admins) as string[]).map((a) => this.card(a)), epoch: ch?.epoch ?? c!.epoch, rekey_due: ch?.rekey_due ?? c!.rekeyDue,
      messages: ch?.messages ?? c!.messages.length, last: last ? await this.msgView(last) : null, created_height: ch?.created_height ?? c!.createdHeight,
    };
  }

  escrowsOf(account: string) {
    const ix = this.idx();
    const out: { in: EscrowRec[]; out: EscrowRec[] } = { in: [], out: [] };
    for (const e of ix.escrows.values()) {
      if (e.to === account) out.in.push(e);
      if (e.from === account) out.out.push(e);
    }
    const by = (a: EscrowRec, b: EscrowRec) => b.height - a.height;
    out.in.sort(by);
    out.out.sort(by);
    return out;
  }
}

// ------------------------------------------------------------ helpers

const hex64 = (v: unknown, name: string) => {
  if (typeof v !== "string" || !/^[0-9a-fA-F]{64}$/.test(v)) throw badRequest(`${name} must be 64 hex`);
  return v.toLowerCase();
};
function intq(q: URLSearchParams, k: string, d: number, lo: number, hi: number) {
  const v = q.get(k);
  if (v === null || v === "") return d;
  const n = Number(v);
  if (!Number.isInteger(n) || n < lo || n > hi) throw badRequest(`${k} must be an integer in ${lo}..${hi}`);
  return n;
}
function cursor(q: URLSearchParams): [number, number] | null {
  const c = q.get("before");
  if (!c) return null;
  const m = c.match(/^(\d+)(?::(\d+))?$/);
  if (!m) throw badRequest("before is a cursor height:index (or a height)");
  return [Number(m[1]), m[2] === undefined ? Number.MAX_SAFE_INTEGER : Number(m[2])];
}
const older = (m: MsgRec, c: [number, number] | null) => !c || m.height < c[0] || (m.height === c[0] && m.index < c[1]);
const refused = (msg: string, details?: unknown) => new ApiError(409, "refused", msg, details);
const optAmount = (v: unknown, name: string) => (v === undefined || v === null || v === "0" || v === 0 || v === "" ? 0n : amount(v, name));
const isAdmin = (ch: any, a: string) => ch.creator === a || (ch.admins ?? []).includes(a);

/** Newest-first page of `l` (oldest first) with a height:index cursor. */
function page(l: MsgRec[], q: URLSearchParams) {
  const limit = intq(q, "limit", 50, 1, 200);
  const cur = cursor(q);
  const rev = l.filter((m) => older(m, cur)).reverse();
  const p = rev.slice(0, limit);
  return { p, next: rev.length > limit ? `${p[p.length - 1].height}:${p[p.length - 1].index}` : null };
}

async function builderBase(d: Deps, from: string) {
  const [acct, params, info] = await Promise.all([d.chain<any>("getAccount", { account: from }), d.chain<any>("getParams", {}), d.chain<any>("getChainInfo", {})]);
  return { acct, params, info };
}

function built(b: { acct: any; params: any; info: any }, body: DmBody, extra: Record<string, unknown>, spend = 0n) {
  const fee = dmFee(b.params, body);
  if (BigInt(b.acct.balance) < fee + spend) {
    throw new ApiError(422, "insufficient_funds", `balance ${b.acct.balance} < fee ${fee}${spend ? ` + pay ${spend}` : ""} (chain units; the sender must hold CADRE on the agent chain)`);
  }
  return {
    window_pearl: { method: "signAndSendTransaction", params: { transaction: { ...body, fee: fee.toString() } } },
    chain_id: b.info.chain_id, nonce: b.acct.nonce, fee: fee.toString(), fee_rule: FEE_RULE, ...extra,
    sign_with: "window.cadre.signAndSendTransaction({ transaction: window_pearl.params.transaction }) -> { txHash }; then GET /v1/tx/chain/{txHash}",
  };
}

// ------------------------------------------------------------ routes

export function registerDms(api: Api, d: Deps) {
  const s = new DmState(d);

  api.get("/v1/dm/params", async () => {
    if (d.town) {
      const c = d.town.config;
      return {
        chain: "solana", program: d.town.program, active: !!c && !c.paused, msg_inline_max: TS.LIMITS.body, reclaim_min_secs: c?.reclaimMinSecs ?? null, reclaim_max_secs: c?.reclaimMaxSecs ?? null,
        paused: c?.paused ?? null, pay_unit: "lamports", channels: "not on the Solana town (old Cadre-chain channels stay readable)",
        fee_rule: "the Solana network fee (5000 lamports per signature) plus rent for a new consent or escrow account, refunded when it closes", unit_note: UNIT_NOTE,
      };
    }
    const [p, params] = await Promise.all([s.rpc<any>("getDmParams", {}), d.chain<any>("getParams", {})]);
    return { ...p, min_fee: params.min_fee ?? null, social_min_fee: params.social_min_fee ?? null, social_byte_fee: params.social_byte_fee ?? null, fee_rule: FEE_RULE };
  }, { cache: 10 });

  api.get("/v1/dm/gate", async (c) => {
    const from = addr(c.query.get("from"), "from");
    const to = addr(c.query.get("to"), "to");
    if (d.town) return solGate(d.town, from, to);
    return s.rpc("getDmGate", { from, to });
  }, { cache: 2 });

  api.get("/v1/dm/stats/:account", async (c) => {
    const a = addr(c.params.account, "account");
    return d.town ? solStats(s, a) : s.rpc("getDmStats", { account: a });
  }, { cache: 3 });

  api.get("/v1/inbox/:account", async (c) => {
    const a = addr(c.params.account, "account");
    const box = c.query.get("box") ?? "all";
    if (!["all", "primary", "requests", "blocked"].includes(box)) throw badRequest("box is all, primary, requests or blocked");
    const limit = intq(c.query, "limit", 50, 1, 200);
    const [stats, prof] = d.town
      ? [solStats(s, a), { enc_key: d.town.profiles.get(a)?.encKey ?? (await d.node.call<any>("getProfile", { account: a }).catch(() => null))?.enc_key ?? null }]
      : await Promise.all([s.rpc<any>("getDmStats", { account: a }), d.node.call<any>("getProfile", { account: a }).catch(() => null)]);
    const ix = s.idx();
    const peers = [...(ix.peers.get(a) ?? [])];
    const all = await Promise.all(peers.map((p) => s.threadView(a, p)));
    all.sort((x, y) => y.last.height - x.last.height || y.last.index - x.last.index);
    const threads = all.filter((t) => box === "all" || t.box === box).slice(0, limit);
    const joined = [...(ix.memberOf.get(a) ?? [])];
    const channels = (await Promise.all(joined.map((id) => s.channelCard(id)))).filter(Boolean);
    const pend = (await s.pending()).filter((p) => p.from.account === a || p.to?.account === a || (p.channel && joined.includes(p.channel)));
    const es = s.escrowsOf(a);
    const live = (l: EscrowRec[]) => l.filter((e) => !e.settled).map((e) => s.escrowView(e));
    return {
      account: a, threads, channels, pending: pend, escrows: { in: live(es.in), out: live(es.out) }, stats,
      enc_key: prof?.enc_key ?? null, tip_height: s.tip(), metadata_note: METADATA_NOTE,
      counts: { primary: all.filter((t) => t.box === "primary").length, requests: all.filter((t) => t.box === "requests").length, blocked: all.filter((t) => t.box === "blocked").length },
    };
  }, { cache: 2 });

  api.get("/v1/inbox/:account/stream", (c: Ctx) => {
    const a = addr(c.params.account, "account");
    const enc = new TextEncoder();
    let unsub: (() => void) | null = null;
    let unsubSol: (() => void) | null = null;
    let beat: NodeJS.Timeout | null = null;
    let pend: NodeJS.Timeout | null = null;
    const sent = new Set<string>();
    let lastH = s.tip();
    const solTip = () => (d.town ? SOL_BASE + d.town.recs.length : 0);
    let lastS = solTip();
    const states = new Map<string, string>();
    const mine = () => {
      const es = s.escrowsOf(a);
      return [...es.in, ...es.out];
    };
    for (const e of mine()) states.set(e.messageId, s.escState(e));
    const stream = new ReadableStream<Uint8Array>({
      start(ctl) {
        const send = (ev: string, data: unknown) => {
          try {
            ctl.enqueue(enc.encode(`event: ${ev}\ndata: ${JSON.stringify(data)}\n\n`));
          } catch {
            /* closed */
          }
        };
        ctl.enqueue(enc.encode(`: inbox stream for ${a}, tip ${lastH}\n\n`));
        const wake = async () => {
          const h = s.tip();
          const hs = solTip();
          if (h === lastH && hs === lastS) return;
          const ranges: [number, number][] = [[lastH + 1, h], [lastS + 1, hs]];
          lastH = h;
          lastS = hs;
          const ix = s.idx();
          const joined = ix.memberOf.get(a) ?? new Set();
          for (const [lo, hi] of ranges) {
            for (let x = lo; x <= hi && lo > 0; x++) {
              for (const m of ix.byHeight.get(x) ?? []) {
                if (m.from === a || m.to === a || (m.channel && joined.has(m.channel))) send("message", await s.msgView(m));
              }
            }
          }
          for (const e of mine()) {
            const st = s.escState(e);
            if (states.get(e.messageId) !== st) {
              states.set(e.messageId, st);
              send("escrow", s.escrowView(e));
            }
          }
        };
        unsub = d.ix.onUpdate(wake);
        unsubSol = d.town?.onUpdate(() => void wake()) ?? null;
        beat = setInterval(() => {
          try {
            ctl.enqueue(enc.encode(`: heartbeat ${Date.now()}\n\n`));
          } catch {
            /* closed */
          }
        }, 15_000);
        const poll = async () => {
          const joined = s.idx().memberOf.get(a) ?? new Set();
          for (const p of await s.pending()) {
            if (sent.has(p.tx_hash)) continue;
            if (!(p.from.account === a || p.to?.account === a || (p.channel && joined.has(p.channel)))) continue;
            sent.add(p.tx_hash);
            send("pending", p);
          }
          if (sent.size > 5000) sent.clear();
        };
        poll();
        pend = setInterval(poll, 5000);
      },
      cancel() {
        unsub?.();
        unsubSol?.();
        if (beat) clearInterval(beat);
        if (pend) clearInterval(pend);
      },
    });
    return reply({ upstream: new Response(stream, { status: 200, headers: { "content-type": "text/event-stream; charset=utf-8" } }), headers: { "x-cadre-upstream": "dms", "cache-control": "no-store", "x-accel-buffering": "no" } });
  }, { cache: "no-store" });

  api.get("/v1/threads/:a/:b", async (c) => {
    const a = addr(c.params.a, "a");
    const b = addr(c.params.b, "b");
    if (a === b) throw badRequest("a and b are the same account");
    const [gate, th] = d.town
      ? [solGate(d.town, a, b), { consent_a_for_b: d.town.consentOf(a, b)?.state ?? null, consent_b_for_a: d.town.consentOf(b, a)?.state ?? null }]
      : await Promise.all([s.rpc<any>("getDmGate", { from: a, to: b }), s.rpc<any>("getDmThread", { a, b })]);
    const { p, next } = page(s.idx().thread(a, b), c.query);
    const pend = (await s.pending()).filter((x) => x.to && ((x.from.account === a && x.to.account === b) || (x.from.account === b && x.to.account === a)));
    return { a, b, messages: await s.msgs(p), next, pending: pend, gate, consent: { a_for_b: th?.consent_a_for_b ?? null, b_for_a: th?.consent_b_for_a ?? null }, metadata_note: METADATA_NOTE };
  }, { cache: 2 });

  api.get("/v1/messages/:message_id", async (c) => {
    const id = hex64(c.params.message_id, "message_id");
    const m = s.idx().messages.get(id);
    if (!m) throw notFound("message not in the index (not on the canonical chain, or not indexed yet)");
    return { ...(await s.msgView(m)), metadata_note: METADATA_NOTE };
  }, { cache: 3 });

  api.get("/v1/escrows/:account", (c) => {
    const a = addr(c.params.account, "account");
    const role = c.query.get("role");
    if (role !== null && role !== "sender" && role !== "recipient") throw badRequest("role is sender or recipient");
    const state = c.query.get("state");
    if (state !== null && !["open", "expired", "released", "refunded"].includes(state)) throw badRequest("state is open, expired, released or refunded");
    const es = s.escrowsOf(a);
    const l = role === "sender" ? es.out : role === "recipient" ? es.in : [...es.in, ...es.out].sort((x, y) => y.height - x.height);
    return { account: a, role: role ?? "both", state: state ?? "all", tip_height: s.tip(), escrows: l.map((e) => s.escrowView(e)).filter((e) => !state || e.state === state) };
  }, { cache: 2 });

  api.get("/v1/consent/:account", (c) => {
    const a = addr(c.params.account, "account");
    const m = new Map(s.idx().consent.get(a) ?? new Map());
    for (const [peer, v] of d.town?.consent.get(a) ?? []) m.set(peer, { state: v.state, height: v.slot });
    const pick = (st: number) => [...m.entries()].filter(([, v]) => v.state === st).sort((x, y) => y[1].height - x[1].height).map(([k, v]) => ({ ...s.card(k), height: v.height }));
    return { account: a, allow: pick(1), deny: pick(2) };
  }, { cache: 3 });

  api.get("/v1/channels", async (c) => {
    const offset = intq(c.query, "offset", 0, 0, 1_000_000);
    const limit = intq(c.query, "limit", 50, 1, 200);
    const r = await s.rpc<any>("listChannels", { offset, limit });
    const channels = await Promise.all((r?.channels ?? []).map((ch: any) => s.channelCard(ch.channel_id, ch)));
    return { total: r?.total ?? 0, offset, limit, channels };
  }, { cache: 5 });

  api.get("/v1/channels/:id", async (c) => {
    const id = hex64(c.params.id, "id");
    const card = await s.channelCard(id);
    if (!card) throw notFound("no channel with this id");
    const ch = s.idx().channels.get(id);
    const admins = new Set(card.admins.map((x: any) => x.account));
    const members = ch ? [...ch.members.entries()].sort((x, y) => x[1] - y[1]).map(([k, h]) => ({ account: s.card(k), since_height: h, admin: admins.has(k) || k === card.creator.account })) : [];
    const epochs = ch && ch.mode === 1 ? [...ch.epochs.entries()].sort((x, y) => x[0] - y[0]).map(([e, v]) => ({ epoch: e, height: v.height, envelopes: [...v.envelopes.entries()].map(([m, hx]) => ({ member: m, envelope_hex: hx })) })) : [];
    return { ...card, member_count: card.members, members, epochs, invites: ch ? [...ch.invites.entries()].map(([k, h]) => ({ account: s.card(k), height: h })) : [] };
  }, { cache: 3 });

  api.get("/v1/channels/:id/messages", async (c) => {
    const id = hex64(c.params.id, "id");
    const ch = s.idx().channels.get(id);
    if (!ch) throw notFound("no channel with this id in the index");
    const { p, next } = page(ch.messages, c.query);
    const pend = (await s.pending()).filter((x) => x.channel === id);
    return { channel_id: id, messages: await s.msgs(p), next, pending: pend, metadata_note: METADATA_NOTE };
  }, { cache: 2 });

  api.get("/v1/channels/:id/key", (c) => {
    const id = hex64(c.params.id, "id");
    const account = addr(c.query.get("account"), "account");
    const ch = s.idx().channels.get(id);
    if (!ch) throw notFound("no channel with this id in the index");
    if (ch.mode !== 1) throw notFound("a plain channel has no keys");
    const epoch = intq(c.query, "epoch", ch.epoch, 1, 0xffffffff);
    const env = ch.epochs.get(epoch)?.envelopes.get(account);
    if (!env) throw notFound(`no envelope for ${account} at epoch ${epoch}`);
    return { channel_id: id, epoch, member: account, envelope_hex: env };
  }, { cache: 3 });

  // ---------------------------------------------------------------- tx builders

  api.post("/v1/tx/send-message", async (c) => {
    const b0 = await c.json();
    if (d.town) return solSend(s, b0);
    const from = addr(b0.from, "from");
    const hasTo = b0.to !== undefined && b0.to !== null && b0.to !== "";
    const hasCh = b0.channel !== undefined && b0.channel !== null && b0.channel !== "";
    if (hasTo === hasCh) throw badRequest("set exactly one of to (a DM) or channel");
    const mode = b0.mode ?? "plain";
    const m = MODES.indexOf(mode);
    if (m < 0) throw badRequest("mode is plain or sealed");
    let text: string | undefined;
    let raw: Uint8Array;
    if (m === 0) {
      if (typeof b0.body !== "string" || !b0.body) throw badRequest("body (text) is required in plain mode");
      text = b0.body;
      raw = Buffer.from(text!, "utf8");
    } else {
      if (typeof b0.body_hex !== "string" || !/^([0-9a-fA-F]{2})+$/.test(b0.body_hex)) throw badRequest("body_hex (the sealed body from window.cadre.dm.seal) is required in sealed mode");
      if (b0.body !== undefined && b0.body !== null) throw badRequest("a sealed message carries body_hex only; the API never takes a plaintext to seal");
      raw = Buffer.from(b0.body_hex, "hex");
    }
    const replyTo = b0.reply_to === undefined || b0.reply_to === null || b0.reply_to === "" ? ZERO_HASH : hex64(b0.reply_to, "reply_to");
    const pay = optAmount(b0.pay, "pay");
    const reclaim = b0.reclaim_after === undefined || b0.reclaim_after === null ? 0 : Number(b0.reclaim_after);
    if (!Number.isInteger(reclaim) || reclaim < 0 || reclaim > 0xffffffff) throw badRequest("reclaim_after is a block count (u32)");
    const ref = b0.thought_ref === undefined || b0.thought_ref === null ? "" : String(b0.thought_ref);
    if (ref && !/^[\x21-\x7e]{1,64}$/.test(ref)) throw badRequest("thought_ref is 1..64 printable ASCII characters without spaces");
    const bb = await builderBase(d, from);
    const extra: Record<string, unknown> = {};
    let to = ZERO_KEY;
    let channel = ZERO_HASH;
    if (hasTo) {
      to = addr(b0.to, "to");
      if (to === from) throw badRequest("cannot message yourself");
      const [gate, dp, stats] = await Promise.all([s.rpc<any>("getDmGate", { from, to }), s.rpc<any>("getDmParams", {}), s.rpc<any>("getDmStats", { account: from })]);
      extra.gate = gate;
      const max = Number(gate.msg_inline_max ?? dp.msg_inline_max);
      if (raw.length > max) throw badRequest(`body is ${raw.length} bytes, msg_inline_max is ${max}`);
      if (m === 1) {
        try {
          checkSealedDm(raw);
        } catch (e) {
          throw badRequest(`body_hex is not a sealed DM body: ${(e as Error).message}`);
        }
        if (!gate.sealed_ok) throw refused("the recipient has published no enc_key (SetProfile), so it cannot open a sealed message", { gate });
      }
      if (!gate.can_send) throw refused(gate.reason, { gate });
      const need = BigInt(gate.required_pay ?? "0");
      if (pay < need) throw refused(`the recipient takes cold DMs at a price of ${need}: pay ${pay} is short`, { gate });
      if (reclaim > 0) {
        if (pay === 0n) throw badRequest("reclaim_after needs pay > 0");
        if (reclaim < Number(dp.dm_reclaim_min) || reclaim > Number(dp.dm_reclaim_max)) throw badRequest(`reclaim_after must be ${dp.dm_reclaim_min}..=${dp.dm_reclaim_max} blocks`);
        if (Number(stats.open_escrows) >= Number(dp.dm_max_open_escrows)) throw refused(`at most ${dp.dm_max_open_escrows} open escrows per sender`);
      }
    } else {
      channel = hex64(b0.channel, "channel");
      if (pay !== 0n || reclaim !== 0) throw badRequest("channel messages carry no pay");
      const [ch, mem, dp] = await Promise.all([s.rpc<any>("getChannel", { channel }), s.rpc<any>("getChannelMember", { channel, account: from }), s.rpc<any>("getDmParams", {})]);
      if (!ch) throw refused("unknown channel");
      extra.channel = ch;
      if (raw.length > Number(dp.msg_inline_max)) throw badRequest(`body is ${raw.length} bytes, msg_inline_max is ${dp.msg_inline_max}`);
      if (!mem?.member) throw refused("only members post in a channel");
      if (m !== ch.mode) throw refused(ch.mode === 1 ? "this channel is sealed: send mode sealed" : "this channel is plain: send mode plain");
      if (m === 1) {
        if (ch.epoch === 0) throw refused("no channel key yet: an admin must post ChannelKey epoch 1");
        if (ch.rekey_due) throw refused("a member left or was removed: an admin must post a new key epoch first");
        let e: number;
        try {
          e = checkSealedChannel(raw);
        } catch (err) {
          throw badRequest(`body_hex is not a sealed channel body: ${(err as Error).message}`);
        }
        if (e !== ch.epoch) throw refused(`sealed under epoch ${e}, the channel is at epoch ${ch.epoch}`);
      }
    }
    if (m === 0 && !isUtf8(raw)) throw badRequest("a plain body must be UTF-8");
    const body: DmBody = {
      kind: "SendMessage", to, channel, mode: m, replyTo, pay: pay.toString(), reclaimAfter: reclaim,
      ...(m === 0 ? { body: text! } : { bodyHex: Buffer.from(raw).toString("hex") }), blobUri: "", blobHash: ZERO_HASH, thoughtRef: ref,
    };
    return built(bb, body, {
      ...extra, message_id: messageId(from, bb.acct.nonce),
      message_id_rule: "blake3(\"pearl-sol:msg:v1\" || from || nonce), predicted from the account nonce now; a tx the sender signs first takes this nonce",
      summary: { from, to: hasTo ? to : null, channel: hasCh ? channel : null, mode, bytes: raw.length, pay: pay.toString(), reclaim_after: reclaim, reply_to: replyTo === ZERO_HASH ? null : replyTo, escrow: reclaim > 0 },
      metadata_note: METADATA_NOTE,
    }, pay);
  });

  api.post("/v1/tx/reclaim-message", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const id = hex64(b0.message_id, "message_id");
    if (d.town?.escrows.has(id)) return solReclaim(s, from, id);
    // an escrow of the Cadre-chain history: ReclaimMessage (kind 17) stays valid on the chain after the town sunset
    const bb = await builderBase(d, from);
    const e = await s.rpc<any>("getMessageEscrow", { message_id: id });
    if (!e) throw refused("no escrow for this message (never escrowed, already released by a reply, or already reclaimed)");
    if (from === e.from) {
      if (!e.reclaimable) throw refused(`the reply window is open until height ${e.deadline}`, { escrow: e });
    } else if (from !== e.to) throw refused("only the sender (after the window) or the recipient (to decline) can reclaim", { escrow: e });
    return built(bb, { kind: "ReclaimMessage", messageId: id }, { escrow: e, declining: from === e.to, summary: { from, message_id: id, refund_to: e.from, pay: e.pay } });
  });

  api.post("/v1/tx/set-consent", async (c) => {
    const b0 = await c.json();
    if (d.town) return solConsent(s, b0);
    const from = addr(b0.from, "from");
    if (!Array.isArray(b0.items) || b0.items.length < 1 || b0.items.length > 64) throw badRequest("items is a list of 1..64 {peer, state}");
    const items = b0.items.map((x: any, i: number) => {
      const peer = addr(x?.peer, `items[${i}].peer`);
      if (peer === from || peer === ZERO_KEY) throw badRequest(`items[${i}]: not a valid peer`);
      const st = CONSENT_STATES.indexOf(x?.state);
      if (st < 0) throw badRequest(`items[${i}].state is allow, deny or clear`);
      return { peer, state: st };
    });
    const bb = await builderBase(d, from);
    return built(bb, { kind: "SetConsent", items }, { summary: { from, allow: items.filter((x: any) => x.state === 1).length, deny: items.filter((x: any) => x.state === 2).length, clear: items.filter((x: any) => x.state === 0).length } });
  });

  api.post("/v1/tx/channel-config", async (c) => {
    if (d.town) throw new ApiError(410, "gone", "channels did not move to the Solana town; the Cadre-chain channels stay readable (GET /v1/channels)");
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const create = b0.channel_id === undefined || b0.channel_id === null || b0.channel_id === "";
    const cid = create ? ZERO_HASH : hex64(b0.channel_id, "channel_id");
    const name = typeof b0.name === "string" ? b0.name : "";
    if (!name || utf8len(name) > 64) throw badRequest("name is 1..64 bytes of UTF-8");
    const about = b0.about === undefined || b0.about === null ? "" : String(b0.about);
    if (utf8len(about) > 1024) throw badRequest("about is at most 1024 bytes of UTF-8");
    const policy = POLICIES.indexOf(b0.policy ?? "open");
    if (policy < 0) throw badRequest("policy is open, invite or paid");
    const price = optAmount(b0.join_price, "join_price");
    if ((policy === 2) !== (price > 0n)) throw badRequest("join_price must be positive exactly for a paid channel");
    const admins: string[] = b0.admins === undefined || b0.admins === null ? [] : Array.isArray(b0.admins) ? b0.admins.map((a: unknown, i: number) => addr(a, `admins[${i}]`)) : (() => { throw badRequest("admins is a list of addresses"); })();
    if (new Set(admins).size !== admins.length) throw badRequest("duplicate admin");
    const [bb, dp] = await Promise.all([builderBase(d, from), s.rpc<any>("getDmParams", {})]);
    if (admins.length > Number(dp.channel_max_admins)) throw badRequest(`at most ${dp.channel_max_admins} admins`);
    let mode: number;
    let spend = 0n;
    const extra: Record<string, unknown> = {};
    if (create) {
      mode = MODES.indexOf(b0.mode ?? "plain");
      if (mode < 0) throw badRequest("mode is plain or sealed");
      if (admins.includes(from)) throw badRequest("admins must be other accounts than the creator");
      if (mode === 1 && 1 + admins.length > Number(dp.channel_sealed_max_members)) throw badRequest(`a sealed channel holds at most ${dp.channel_sealed_max_members} members`);
      spend = BigInt(dp.channel_create_fee ?? "0");
      extra.channel_id = channelId(from, bb.acct.nonce);
      extra.channel_id_rule = "blake3(\"pearl-sol:channel:v1\" || creator || nonce), predicted from the account nonce now";
      extra.create_fee = spend.toString();
    } else {
      const ch = await s.rpc<any>("getChannel", { channel: cid });
      if (!ch) throw refused("unknown channel");
      if (!isAdmin(ch, from)) throw refused("only the channel's creator or admins can change it");
      mode = b0.mode === undefined || b0.mode === null ? ch.mode : MODES.indexOf(b0.mode);
      if (mode !== ch.mode) throw refused("a channel's mode is fixed at creation");
      if (admins.includes(ch.creator)) throw badRequest("admins must be other accounts than the creator");
      const same = admins.length === ch.admins.length && admins.every((a, i) => a === ch.admins[i]);
      if (from !== ch.creator && !same) throw refused("only the creator changes the admin list");
      const mem = await Promise.all(admins.map((a) => s.rpc<any>("getChannelMember", { channel: cid, account: a })));
      mem.forEach((x, i) => {
        if (!x?.member) throw refused(`admin ${i} must be a member first`);
      });
      extra.channel_id = cid;
      extra.channel = ch;
    }
    const body: DmBody = { kind: "ChannelConfig", channelId: cid, name, about, policy, joinPrice: price.toString(), mode, admins };
    return built(bb, body, { ...extra, summary: { from, create, name, policy: POLICIES[policy], mode: MODES[mode], admins: admins.length } }, spend);
  });

  api.post("/v1/tx/channel-member", async (c) => {
    if (d.town) throw new ApiError(410, "gone", "channels did not move to the Solana town; the Cadre-chain channels stay readable (GET /v1/channels)");
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const channel = hex64(b0.channel, "channel");
    const action = ACTIONS.indexOf(b0.action);
    if (action < 0) throw badRequest("action is join, leave, invite or remove");
    let member = b0.member === undefined || b0.member === null || b0.member === "" ? ZERO_KEY : addr(b0.member, "member");
    let pay = optAmount(b0.pay, "pay");
    if (action <= 1) {
      if (member !== ZERO_KEY && member !== from) throw badRequest("join and leave name the sender (or no member)");
      member = ZERO_KEY;
    } else {
      if (member === ZERO_KEY) throw badRequest("invite and remove name a member");
      if (pay !== 0n) throw badRequest("invite and remove carry no pay");
    }
    const who = member === ZERO_KEY ? from : member;
    const [bb, ch, mem, dp] = await Promise.all([builderBase(d, from), s.rpc<any>("getChannel", { channel }), s.rpc<any>("getChannelMember", { channel, account: who }), s.rpc<any>("getDmParams", {})]);
    if (!ch) throw refused("unknown channel");
    if (action >= 2 && !isAdmin(ch, from)) throw refused("only the channel's creator or admins invite and remove");
    if (action === 0) {
      if (mem.member) throw refused("already a member");
      if (ch.mode === 1 && ch.members >= Number(dp.channel_sealed_max_members)) throw refused(`a sealed channel holds at most ${dp.channel_sealed_max_members} members`);
      if (mem.invited) {
        if (pay !== 0n) throw badRequest("an invited join carries no pay");
      } else if (ch.policy === 0) {
        if (pay !== 0n) throw badRequest("an open channel's join carries no pay");
      } else if (ch.policy === 1) {
        throw refused("this channel is invite only");
      } else {
        if (b0.pay === undefined || b0.pay === null) pay = BigInt(ch.join_price);
        if (pay < BigInt(ch.join_price)) throw refused(`joining costs ${ch.join_price}: pay ${pay} is short`);
      }
    } else if (action === 1 || action === 3) {
      if (!mem.member) throw refused("not a member");
      if (who === ch.creator) throw refused("the creator cannot leave or be removed");
      if (action === 3 && ch.admins.includes(who) && from !== ch.creator) throw refused("only the creator removes an admin");
      if (action === 1 && pay !== 0n) throw badRequest("leave carries no pay");
    } else {
      if (mem.member) throw refused("already a member");
      if (mem.invited) throw refused("already invited");
    }
    const body: DmBody = { kind: "ChannelMember", channel, action, member, pay: pay.toString() };
    return built(bb, body, {
      channel: ch, member: mem,
      rekey: (action === 1 || action === 3) && ch.mode === 1 && ch.epoch > 0 ? "this leave or removal sets rekey_due: channel messages are refused until an admin posts the next key epoch (ChannelKey epoch + 1)" : null,
      summary: { from, channel, action: ACTIONS[action], member: who, pay: pay.toString() },
    }, pay);
  });

  api.post("/v1/tx/channel-key", async (c) => {
    if (d.town) throw new ApiError(410, "gone", "channels did not move to the Solana town; the Cadre-chain channels stay readable (GET /v1/channels)");
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const channel = hex64(b0.channel, "channel");
    const epoch = Number(b0.epoch);
    if (!Number.isInteger(epoch) || epoch < 1 || epoch > 0xffffffff) throw badRequest("epoch is a positive u32");
    if (!Array.isArray(b0.envelopes) || b0.envelopes.length < 1 || b0.envelopes.length > 100) throw badRequest("envelopes is a list of 1..100 {member, envelope_hex}");
    const envs = b0.envelopes.map((x: any, i: number) => {
      const member = addr(x?.member, `envelopes[${i}].member`);
      if (typeof x?.envelope_hex !== "string" || !new RegExp(`^[0-9a-fA-F]{${KEY_ENVELOPE_LEN * 2}}$`).test(x.envelope_hex)) throw badRequest(`envelopes[${i}].envelope_hex is ${KEY_ENVELOPE_LEN} bytes of hex`);
      return { member, envelopeHex: x.envelope_hex.toLowerCase() };
    });
    if (new Set(envs.map((e: any) => e.member)).size !== envs.length) throw badRequest("a member is named twice");
    const [bb, ch, me] = await Promise.all([builderBase(d, from), s.rpc<any>("getChannel", { channel }), s.rpc<any>("getChannelMember", { channel, account: from })]);
    if (!ch) throw refused("unknown channel");
    if (ch.mode !== 1) throw refused("keys are for sealed channels");
    if (!isAdmin(ch, from) || !me?.member) throw refused("only a member admin (or the creator) posts channel keys");
    if (!(epoch === ch.epoch + 1 || (epoch === ch.epoch && epoch > 0))) throw refused(`epoch must be ${ch.epoch} (more envelopes) or ${ch.epoch + 1} (a new key)`);
    const checks = await Promise.all(envs.map((e: any) => Promise.all([s.rpc<any>("getChannelMember", { channel, account: e.member }), d.node.call<any>("getProfile", { account: e.member }).catch(() => null)])));
    checks.forEach(([m, p], i) => {
      if (!m?.member) throw refused(`envelope ${i}: not a member`);
      if (!p?.enc_key) throw refused(`envelope ${i}: the member has no enc_key`);
    });
    const body: DmBody = { kind: "ChannelKey", channel, epoch, envelopes: envs };
    return built(bb, body, { channel: ch, new_epoch: epoch === ch.epoch + 1, summary: { from, channel, epoch, envelopes: envs.length } });
  });
}

// ------------------------------------------------------------ Solana (TOWN_PROGRAM set)

function solStats(s: DmState, a: string) {
  const town = s.d.town!;
  const ix = s.idx();
  let sent = 0, received = 0, payIn = 0n, payOut = 0n, open = 0, locked = 0n, answered = 0, refunded = 0;
  for (const p of ix.peers.get(a) ?? []) {
    for (const m of ix.thread(a, p)) {
      if (!isSolHeight(m.height)) continue;
      const sm = town.messages.get(m.messageId);
      const pay = BigInt(sm?.pay ?? "0");
      if (m.from === a) {
        sent++;
        if (!sm?.escrowed) payOut += pay;
      } else {
        received++;
        if (!sm?.escrowed) payIn += pay;
      }
    }
  }
  for (const e of town.escrows.values()) {
    if (e.from === a && !e.settled) {
      open++;
      locked += BigInt(e.pay);
    }
    if (e.settled?.state === "released") {
      if (e.to === a) {
        answered++;
        payIn += BigInt(e.pay);
      }
      if (e.from === a) payOut += BigInt(e.pay);
    }
    if (e.settled?.state === "refunded" && e.from === a) refunded++;
  }
  return {
    account: a, chain: "solana", sent, received, pay_in: payIn.toString(), pay_out: payOut.toString(), pay_unit: "lamports",
    open_escrows: open, locked: locked.toString(), answered, refunded, window: "the Solana town only (Cadre-chain history counts stay on GET /v1/threads)",
  };
}

async function solSend(s: DmState, b0: any) {
  const d = s.d;
  const town = needTown(d);
  const from = addr(b0.from, "from");
  if (b0.channel !== undefined && b0.channel !== null && b0.channel !== "") throw new ApiError(410, "gone", "channels did not move to the Solana town; send a DM with to");
  const to = addr(b0.to, "to");
  if (to === from) throw badRequest("cannot message yourself");
  const mode = b0.mode ?? "plain";
  const m = MODES.indexOf(mode);
  if (m < 0) throw badRequest("mode is plain or sealed");
  let raw: Uint8Array;
  if (m === 0) {
    if (typeof b0.body !== "string" || !b0.body) throw badRequest("body (text) is required in plain mode");
    raw = Buffer.from(b0.body, "utf8");
  } else {
    if (typeof b0.body_hex !== "string" || !/^([0-9a-fA-F]{2})+$/.test(b0.body_hex)) throw badRequest("body_hex (the sealed body from window.pearl.dm.seal) is required in sealed mode");
    if (b0.body !== undefined && b0.body !== null) throw badRequest("a sealed message carries body_hex only; the API never takes a plaintext to seal");
    raw = Buffer.from(b0.body_hex, "hex");
    try {
      checkSealedDm(raw);
    } catch (e) {
      throw badRequest(`body_hex is not a sealed DM body: ${(e as Error).message}`);
    }
  }
  if (raw.length > TS.LIMITS.body) throw badRequest(`body is ${raw.length} bytes, the town takes at most ${TS.LIMITS.body}`);
  const replyTo = b0.reply_to === undefined || b0.reply_to === null || b0.reply_to === "" ? null : hex64(b0.reply_to, "reply_to");
  const pay = optAmount(b0.pay, "pay (lamports)");
  const reclaim = b0.reclaim_after === undefined || b0.reclaim_after === null ? 0 : Number(b0.reclaim_after);
  if (!Number.isInteger(reclaim) || reclaim < 0 || reclaim > 0xffffffff) throw badRequest("reclaim_after is a number of seconds (u32) on the Solana town");
  const ref = b0.thought_ref === undefined || b0.thought_ref === null ? "" : String(b0.thought_ref);
  if (ref && !/^[\x21-\x7e]{1,64}$/.test(ref)) throw badRequest("thought_ref is 1..64 printable ASCII characters without spaces");
  const gate = solGate(town, from, to);
  if (m === 1 && !gate.sealed_ok) throw refused("the recipient has published no enc_key on its Solana town profile, so it cannot open a sealed message", { gate });
  if (!gate.can_send) throw refused(gate.reason, { gate });
  const need = BigInt(gate.required_pay);
  if (pay < need) throw refused(`the recipient takes cold DMs at a price of ${need} lamports: pay ${pay} is short`, { gate });
  if (reclaim > 0) {
    if (pay === 0n) throw badRequest("reclaim_after needs pay > 0");
    const c = town.config;
    if (c && (reclaim < c.reclaimMinSecs || reclaim > c.reclaimMaxSecs)) throw badRequest(`reclaim_after must be ${c.reclaimMinSecs}..=${c.reclaimMaxSecs} seconds`);
  }
  // a reply in time to a paid DM from `to` releases its escrow: pass that escrow
  const esc = replyTo ? town.escrows.get(replyTo) : undefined;
  const replyEscrow = esc && !esc.settled && esc.from === to && esc.to === from ? TS.escrowPda(replyTo!, town.program) : null;
  const nonce = TS.freshNonce();
  const ix = TS.sendMessageIx(from, to, {
    nonce, mode: m, replyTo, pay, reclaimAfterSecs: reclaim, body: raw, thoughtRef: ref || null, withFollowEdge: town.follows(to, from), replyEscrow,
  }, town.program);
  return solBuilt(d, [ix], from, {
    from, to, mode, bytes: raw.length, pay_lamports: pay.toString(), reclaim_after_secs: reclaim, reply_to: replyTo, escrow: reclaim > 0, releases_escrow: !!replyEscrow,
  }, { gate, message_id: TS.messageId(from, nonce), nonce: nonce.toString(), message_id_rule: "sha256(\"cadre-town:msg:v1\" || from || nonce u64 LE), fixed by the nonce in this tx", metadata_note: METADATA_NOTE, unit_note: UNIT_NOTE });
}

async function solReclaim(s: DmState, from: string, id: string) {
  const town = needTown(s.d);
  const e = town.escrows.get(id)!;
  if (e.settled) throw refused(`this escrow is already ${e.settled.state}`, { escrow: s.escrowView(s.idx().escrows.get(id)!) });
  let ix;
  if (from === e.from) {
    if (Math.floor(Date.now() / 1000) <= e.deadline) throw refused(`the reply window is open until ${new Date(e.deadline * 1000).toISOString()}`);
    ix = TS.reclaimIx(from, id, town.program);
  } else if (from === e.to) ix = TS.declineIx(from, e.from, id, town.program);
  else throw refused("only the sender (after the window) or the recipient (to decline) can settle this escrow");
  return solBuilt(s.d, [ix], from, { from, message_id: id, refund_to: e.from, pay_lamports: e.pay, declining: from === e.to });
}

async function solConsent(s: DmState, b0: any) {
  const town = needTown(s.d);
  const from = addr(b0.from, "from");
  if (!Array.isArray(b0.items) || b0.items.length < 1 || b0.items.length > 8) throw badRequest("items is a list of 1..8 {peer, state} (one Solana transaction)");
  const ixs = b0.items.map((x: any, i: number) => {
    const peer = addr(x?.peer, `items[${i}].peer`);
    if (peer === from || peer === ZERO_KEY) throw badRequest(`items[${i}]: not a valid peer`);
    const st = CONSENT_STATES.indexOf(x?.state);
    if (st < 0) throw badRequest(`items[${i}].state is allow, deny or clear`);
    if (st === 0) {
      if (!town.consentOf(from, peer)) throw refused(`items[${i}]: nothing to clear for ${peer}`);
      return TS.clearConsentIx(from, peer, town.program);
    }
    return TS.setConsentIx(from, peer, st, town.program);
  });
  return solBuilt(s.d, ixs, from, { from, items: b0.items.length });
}
