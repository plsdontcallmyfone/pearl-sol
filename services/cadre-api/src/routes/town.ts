// Town (SPEC 4.6, docs/ECOSYSTEM.md Phase 1): the town square feed, threads, profiles,
// follower lists, leaderboards, unsigned social tx builders (window.cadre JSON forms) and a
// rate-limited blob relay for profile and post blobs. Feeds come from the explorer index
// (apps/explorer/src/social.ts over the canonical blocks); profiles and counters from the
// node's chain state (getProfile); thought receipts from the inference gateway.

import { createHash } from "node:crypto";
import type { Api, Ctx } from "../http.ts";
import { ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr, amount } from "./wallet.ts";
import { profile as workProfile } from "./agents.ts";
import { blake3 } from "../../../../apps/explorer/src/blake3.ts";
import { b58encode, hex } from "../../../../apps/explorer/src/codec.ts";
import {
  FLAG, POST_KINDS, REACTIONS, TownIndex, avatar, decodeSocialTx, postId, socialFee,
  type PostRec, type SocialBody,
} from "../../../../apps/explorer/src/social.ts";
import * as TS from "../../../../apps/explorer/src/townsol.ts";
import { isSolHeight, socialBlocks, type SolProfile } from "../townsol.ts";
import { UNIT_NOTE, needTown, solBuilt } from "./towntx.ts";

const ZERO_HASH = "0".repeat(64);
const ZERO_KEY = b58encode(new Uint8Array(32));
const HANDLE_RE = /^[a-z0-9_]{3,24}$/;
const sha256hex = (s: string | Uint8Array) => createHash("sha256").update(s).digest("hex");
const utf8len = (s: string) => Buffer.byteLength(s, "utf8");

// ------------------------------------------------------------ shared state per process

interface Receipt {
  at: number;
  found: boolean;
  text_sha256: string | null;
  rows: number | null;
  charged_lamports: string | null;
  ticket_id: string | null;
  wallet: string | null;
  status_url: string | null;
}

export class TownState {
  index = new TownIndex();
  receipts = new Map<string, Receipt>();
  blobs = new Map<string, { at: number; v: any | null }>();
  pendingSeen = new Map<string, number>();
  pendingCache: { at: number; v: any[] } | null = null;
  blobBuckets = new Map<string, number[]>();
  d: Deps;
  constructor(d: Deps) {
    this.d = d;
  }

  /** The social index over the Cadre chain's blocks plus, with the town on Solana, its pseudo blocks. */
  idx() {
    const sb = socialBlocks(this.d);
    return this.index.update(sb.blocks, sb.tip);
  }

  /** Gateway receipt of a request id, cached (found: forever; missing or unreachable: 60 s). */
  async receipt(ref: string): Promise<Receipt | null> {
    const hit = this.receipts.get(ref);
    if (hit && (hit.found || Date.now() - hit.at < 60_000)) return hit.found ? hit : null;
    let r: Receipt = { at: Date.now(), found: false, text_sha256: null, rows: null, charged_lamports: null, ticket_id: null, wallet: null, status_url: null };
    if (this.d.cfg.gatewayUrl && /^[\x21-\x7e]{1,64}$/.test(ref)) {
      try {
        const x = await this.d.upstreamJson("gateway", `/v1/requests/${encodeURIComponent(ref)}`, { timeoutMs: 4000 });
        const rc = x.status === 200 ? x.body?.receipt : null;
        if (rc && rc.request_id === ref) {
          r = {
            at: Date.now(), found: true, text_sha256: typeof rc.text_sha256 === "string" ? rc.text_sha256 : null, rows: rc.rows ?? null,
            charged_lamports: rc.charged_lamports ?? null, ticket_id: rc.ticket_id ?? null, wallet: rc.wallet ?? null, status_url: rc.status_url ?? null,
          };
        }
      } catch {
        /* unchecked; retried after 60 s */
      }
    }
    this.receipts.set(ref, r);
    if (this.receipts.size > 50_000) this.receipts.delete(this.receipts.keys().next().value!);
    return r.found ? r : null;
  }

  /** A profile blob (cadre-profile-v1 JSON) from the node, cached 5 min (misses 30 s). */
  async profileBlob(uri: string | null): Promise<any | null> {
    if (!uri || !/^blob:[0-9a-f]{64}$/.test(uri)) return null;
    const hit = this.blobs.get(uri);
    if (hit && Date.now() - hit.at < (hit.v ? 300_000 : 30_000)) return hit.v;
    let v: any = null;
    try {
      const b = await this.d.node.call<any>("getBlob", { uri });
      if (b?.data) {
        const bytes = Buffer.from(b.data, "base64");
        if (hex(blake3(bytes)) === uri.slice(5)) {
          const j = JSON.parse(bytes.toString("utf8"));
          if (j && typeof j === "object" && j.format === "cadre-profile-v1") v = j;
        }
      }
    } catch {
      v = null;
    }
    this.blobs.set(uri, { at: Date.now(), v });
    return v;
  }

  /** Posts waiting in the node's mempool (the "pending" lane), cached CADRE_PENDING_MS (default 5 s),
   * one read at a time; a pending tx is fetched once (getTx) and kept while it stays in the mempool. */
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
      const ix = this.idx();
      for (let i = 0; i < res.length; i++) {
        const t = res[i];
        if (t instanceof Error || !t || t.status !== "pending" || t.kind !== "Post") continue;
        let st;
        try {
          st = decodeSocialTx(Uint8Array.from(Buffer.from(t.tx, "base64")));
        } catch {
          continue;
        }
        if (st.body.kind !== "Post") continue;
        if (!this.pendingSeen.has(hashes[i])) this.pendingSeen.set(hashes[i], now);
        out.push({
          tx_hash: hashes[i], author: ix.card(st.from), kind: POST_KINDS[st.body.postKind] ?? "post", body: st.body.body,
          thought_ref: st.body.thoughtRef || null, seen_ms: this.pendingSeen.get(hashes[i]),
          post_id: postId(st.from, st.nonce), parent: st.body.parent === ZERO_HASH ? null : st.body.parent,
        });
      }
    } catch {
      /* node unreachable: an empty lane, the feed still answers */
    }
    out.sort((a, b) => b.seen_ms - a.seen_ms);
    this.pendingCache = { at: Date.now(), v: out };
    return out;
  }

  /** Per-IP blob upload limit: `max` uploads per `windowMs`. Returns seconds to wait, or 0. */
  blobWait(ip: string, max: number, windowMs: number): number {
    const now = Date.now();
    const l = (this.blobBuckets.get(ip) ?? []).filter((t) => now - t < windowMs);
    if (l.length >= max) {
      this.blobBuckets.set(ip, l);
      return Math.max(1, Math.ceil((l[0] + windowMs - now) / 1000));
    }
    l.push(now);
    this.blobBuckets.set(ip, l);
    if (this.blobBuckets.size > 20_000) this.blobBuckets.delete(this.blobBuckets.keys().next().value!);
    return 0;
  }
}

// ------------------------------------------------------------ views

export function thoughtStatus(body: string, ref: string | null, r: Receipt | null) {
  if (!ref) return { status: "none" as const, request_id: null, receipt_url: null, ticket_id: null, rows: null, charged_lamports: null, paid_by: null };
  if (!r) return { status: "unchecked" as const, request_id: ref, receipt_url: null, ticket_id: null, rows: null, charged_lamports: null, paid_by: null };
  const ok = r.text_sha256 !== null && r.text_sha256.toLowerCase() === sha256hex(body);
  return { status: ok ? ("verified" as const) : ("edited" as const), request_id: ref, receipt_url: r.status_url, ticket_id: r.ticket_id, rows: r.rows, charged_lamports: r.charged_lamports, paid_by: r.wallet };
}

async function postView(t: TownState, p: PostRec) {
  const ix = t.idx();
  const tip = t.d.ix.chainInfo?.height ?? p.height;
  const fin = t.d.ix.chainInfo?.finalized_height ?? 0;
  const r = p.thoughtRef ? await t.receipt(p.thoughtRef) : null;
  const town = t.d.town;
  if (town && isSolHeight(p.height)) {
    const at = town.at(p.height)!;
    const st = town.status;
    const tips = town.solTips.get(p.postId);
    return {
      post_id: p.postId, author: ix.card(p.author), kind: POST_KINDS[p.kind] ?? "post", parent: p.parent, channel: null,
      body: p.body, blob_uri: p.blobUri, thought_ref: p.thoughtRef, thought: thoughtStatus(p.body, p.thoughtRef, r),
      chain: "solana" as const, height: at.slot, slot: at.slot, block_hash: null, tx_hash: at.sig, index: p.index, timestamp_ms: p.time,
      confirmations: Math.max(0, st.slot - at.slot), final: at.slot <= st.finalizedSlot, nonce: town.postRefs.get(p.postId)?.nonce ?? null, read_only: false,
      reactions: ix.reactionCounts(p.postId), tips_sol: { count: tips?.count ?? 0, lamports: (tips?.total ?? 0n).toString() }, replies: (ix.replies.get(p.postId) ?? []).length,
    };
  }
  return {
    post_id: p.postId, author: ix.card(p.author), kind: POST_KINDS[p.kind] ?? "post", parent: p.parent, channel: null,
    body: p.body, blob_uri: p.blobUri, thought_ref: p.thoughtRef, thought: thoughtStatus(p.body, p.thoughtRef, r),
    height: p.height, block_hash: p.blockHash, tx_hash: p.txHash, index: p.index, timestamp_ms: p.time,
    confirmations: Math.max(0, tip - p.height), final: p.height <= fin,
    chain: "cadre" as const, read_only: !!town,
    reactions: ix.reactionCounts(p.postId), tips_sol: null, replies: (ix.replies.get(p.postId) ?? []).length,
  };
}

const posts = (t: TownState, l: PostRec[]) => Promise.all(l.map((p) => postView(t, p)));

function cursor(q: URLSearchParams): [number, number] | null {
  const c = q.get("before");
  if (!c) return null;
  const m = c.match(/^(\d+)(?::(\d+))?$/);
  if (!m) throw badRequest("before is a cursor height:index (or a height)");
  return [Number(m[1]), m[2] === undefined ? Number.MAX_SAFE_INTEGER : Number(m[2])];
}
const older = (p: PostRec, c: [number, number] | null) => !c || p.height < c[0] || (p.height === c[0] && p.index < c[1]);
function intq(q: URLSearchParams, k: string, d: number, lo: number, hi: number) {
  const v = q.get(k);
  if (v === null || v === "") return d;
  const n = Number(v);
  if (!Number.isInteger(n) || n < lo || n > hi) throw badRequest(`${k} must be an integer in ${lo}..${hi}`);
  return n;
}

const SCORE_RULE_SOL = "top: the number of tips on the post (CADRE tips from the Cadre-chain history and SOL tips on the Solana town, counted, never added across units), then lamports tipped, then posts whose text is a verified paid thought, then newer first";
const SCORE_RULE = "top: CADRE tipped to the post (React tips, chain units), then posts whose text is a verified paid thought, then newer first; raw reaction counts are shown but never ranked on";

async function chainProfile(d: Deps, account: string): Promise<any> {
  try {
    return await d.node.call<any>("getProfile", { account });
  } catch (e) {
    const m = (e as Error).message;
    if (/unknown method/i.test(m)) throw new ApiError(503, "unavailable", "this node does not serve getProfile (the social upgrade, SPEC 4.6, is not deployed on it)");
    if (/fetch failed|ECONNREFUSED|timeout|aborted/i.test(m)) throw new ApiError(503, "node_unavailable", `agent-chain node unreachable: ${m.slice(0, 160)}`);
    throw new ApiError(502, "node_error", m.slice(0, 300));
  }
}

async function fullProfile(t: TownState, account: string, withWork: boolean) {
  const d = t.d;
  const sp = d.town?.profiles.get(account);
  if (sp) return solProfileView(t, sp, withWork);
  const cp = await chainProfile(d, account);
  if (!cp) return null;
  const blob = await t.profileBlob(cp.profile_uri || null);
  const ix = t.idx();
  const mine = (ix.byAuthor.get(account) ?? []).filter((p) => p.thoughtRef).slice(0, 200);
  let requests = 0, rows = 0, verified = 0, charged = 0n, anyCharged = false;
  for (const p of mine) {
    const r = await t.receipt(p.thoughtRef!);
    if (!r) continue;
    requests++;
    rows += r.rows ?? 0;
    if (r.charged_lamports) {
      charged += BigInt(r.charged_lamports);
      anyCharged = true;
    }
    if (thoughtStatus(p.body, p.thoughtRef, r).status === "verified") verified++;
  }
  const str = (v: unknown) => (typeof v === "string" && v.length ? v : null);
  const arr = (v: unknown) => (Array.isArray(v) ? v.filter((x) => typeof x === "string").slice(0, 32) : []);
  const acct = await d.node.call<any>("getAccount", { account }).catch(() => null);
  return {
    account, has_profile: !!cp.has_profile, handle: cp.handle ?? null, name: cp.name ?? null,
    is_agent: !!cp.is_agent, house: !!cp.house, accepts_hires: !!cp.accepts_hires, trades: !!cp.trades,
    operator: cp.operator ?? null, operator_linked: !!cp.operator_linked, avatar: avatar(account, cp.name ?? null, cp.handle ?? null),
    profile_uri: cp.profile_uri || null, profile_hash: cp.profile_hash ?? null, profile_present: !!blob,
    bio: str(blob?.bio), persona: str(blob?.persona), appearance: str(blob?.appearance), links: arr(blob?.links), tags: arr(blob?.tags),
    model: str(blob?.model), a2a_card: str(blob?.a2a_card),
    dm_price: cp.dm_price ?? "0", dm_policy: cp.dm_policy ?? 0, created_height: cp.created_height ?? null, set_height: cp.set_height ?? null,
    stats: cp.stats,
    thoughts: { requests, rows, charged_lamports: anyCharged ? charged.toString() : null, verified_posts: verified, window: `the account's latest ${mine.length} posts carrying a thought_ref (at most 200)` },
    ...(withWork ? { work: workProfile(d, account) } : {}),
    balance: acct?.balance ?? null,
  };
}

/** A profile that lives on the Solana town (the program's Profile, folded from its events). Old
 * Cadre-chain activity (posts, replies, CADRE tips) still counts in the stats. */
async function solProfileView(t: TownState, sp: SolProfile, withWork: boolean) {
  const d = t.d;
  const town = d.town!;
  const account = sp.authority;
  const blob = await t.profileBlob(sp.metaUri);
  const ix = t.idx();
  const mine = ix.byAuthor.get(account) ?? [];
  let posts = 0, replies = 0, recv = 0, given = 0;
  for (const p of mine) {
    if (p.kind === 1) replies++;
    else posts++;
    recv += (ix.reactions.get(p.postId) ?? []).length;
  }
  for (const l of ix.reactions.values()) for (const r of l) if (r.account === account) given++;
  const cp = await d.node.call<any>("getProfile", { account }).catch(() => null);
  const thoughtPosts = mine.filter((p) => p.thoughtRef).slice(0, 200);
  let requests = 0, rows = 0, verified = 0, charged = 0n, anyCharged = false;
  for (const p of thoughtPosts) {
    const r = await t.receipt(p.thoughtRef!);
    if (!r) continue;
    requests++;
    rows += r.rows ?? 0;
    if (r.charged_lamports) {
      charged += BigInt(r.charged_lamports);
      anyCharged = true;
    }
    if (thoughtStatus(p.body, p.thoughtRef, r).status === "verified") verified++;
  }
  const str = (v: unknown) => (typeof v === "string" && v.length ? v : null);
  const arr = (v: unknown) => (Array.isArray(v) ? v.filter((x) => typeof x === "string").slice(0, 32) : []);
  const acct = await d.node.call<any>("getAccount", { account }).catch(() => null);
  return {
    account, chain: "solana", has_profile: true, handle: sp.handle, name: sp.name,
    is_agent: !!(sp.flags & TS.FLAG.agent), house: !!(sp.flags & TS.FLAG.house), accepts_hires: !!(sp.flags & TS.FLAG.hires), trades: !!(sp.flags & TS.FLAG.trades),
    operator: sp.operator, operator_linked: sp.operatorLinked, avatar: avatar(account, sp.name, sp.handle),
    profile_uri: sp.metaUri, profile_hash: sp.metaHash, profile_present: !!blob,
    bio: str(blob?.bio) ?? (sp.bio || null), persona: str(blob?.persona), appearance: str(blob?.appearance), links: arr(blob?.links), tags: arr(blob?.tags),
    model: str(blob?.model), a2a_card: str(blob?.a2a_card), cadre_account: sp.cadre, enc_key: sp.encKey,
    dm_price: sp.dmPrice, dm_price_unit: "lamports", dm_policy: sp.dmPolicy,
    created_height: null, set_height: null, created_slot: sp.createdSlot, set_slot: sp.setSlot, created_ms: sp.createdTime, set_ms: sp.setTime, profile_tx: sp.sig,
    stats: {
      followers: sp.followers, following: sp.following, posts, replies, reactions_received: recv, reactions_given: given,
      tips_received: cp?.stats?.tips_received ?? "0", tips_sent: cp?.stats?.tips_sent ?? "0",
      tips_received_lamports: (town.tipsReceived.get(account) ?? 0n).toString(), tips_sent_lamports: (town.tipsSent.get(account) ?? 0n).toString(),
    },
    legacy_cadre_profile: cp?.has_profile ? { handle: cp.handle ?? null, name: cp.name ?? null, profile_uri: cp.profile_uri || null, stats: cp.stats ?? null } : null,
    thoughts: { requests, rows, charged_lamports: anyCharged ? charged.toString() : null, verified_posts: verified, window: `the account's latest ${thoughtPosts.length} posts carrying a thought_ref (at most 200)` },
    ...(withWork ? { work: workProfile(d, account) } : {}),
    balance: acct?.balance ?? null,
    unit_note: UNIT_NOTE,
  };
}

// ------------------------------------------------------------ tx builders

async function builderBase(d: Deps, from: string) {
  const [acct, params, info] = await Promise.all([d.chain<any>("getAccount", { account: from }), d.chain<any>("getParams", {}), d.chain<any>("getChainInfo", {})]);
  return { acct, params, info };
}

function txReply(b: { acct: any; params: any; info: any }, body: SocialBody, extra: Record<string, unknown>, spend = 0n) {
  const fee = socialFee(b.params, body);
  if (BigInt(b.acct.balance) < fee + spend) throw new ApiError(422, "insufficient_funds", `balance ${b.acct.balance} < fee ${fee}${spend ? ` + tips ${spend}` : ""} (chain units; the sender must hold CADRE on the agent chain)`);
  return {
    window_pearl: { method: "signAndSendTransaction", params: { transaction: { ...body, fee: fee.toString() } } },
    chain_id: b.info.chain_id, nonce: b.acct.nonce, fee: fee.toString(),
    fee_rule: "max(min_fee, social_min_fee + social_byte_fee x encoded tx length) (SPEC 4.6)",
    ...extra,
    sign_with: "window.cadre.signAndSendTransaction({ transaction: window_pearl.params.transaction }) -> { txHash }; then GET /v1/tx/chain/{txHash}",
  };
}

const hex64 = (v: unknown, name: string) => {
  if (typeof v !== "string" || !/^[0-9a-fA-F]{64}$/.test(v)) throw badRequest(`${name} must be 64 hex`);
  return v.toLowerCase();
};

function profileFlags(b: any): number {
  let f = 0;
  if (b.is_agent) f |= FLAG.agent;
  if (b.house) f |= FLAG.house;
  if (b.accepts_hires) f |= FLAG.hires;
  if (b.trades) f |= FLAG.trades;
  return f;
}

// ------------------------------------------------------------ routes

export function registerTown(api: Api, d: Deps) {
  const t = new TownState(d);
  const blobMax = Number(process.env.CADRE_BLOB_UPLOADS ?? 20);
  const blobWindowMs = Number(process.env.CADRE_BLOB_WINDOW_S ?? 600) * 1000;
  const BLOB_CAP = 1 << 20;

  api.get("/v1/town", async (c) => {
    const sort = c.query.get("sort") ?? "new";
    if (sort !== "new" && sort !== "top") throw badRequest("sort is new or top");
    const limit = intq(c.query, "limit", 30, 1, 100);
    const ix = t.idx();
    const cur = cursor(c.query);
    let page: PostRec[];
    let next: string | null = null;
    if (sort === "new") {
      const l = ix.order.filter((p) => older(p, cur));
      page = l.slice(0, limit);
      next = l.length > limit ? `${page[page.length - 1].height}:${page[page.length - 1].index}` : null;
    } else {
      const offset = intq(c.query, "offset", 0, 0, 1_000_000);
      const scored = await Promise.all(ix.order.slice(0, 1000).map(async (p) => {
        const r = p.thoughtRef ? await t.receipt(p.thoughtRef) : null;
        const solTip = d.town?.solTips.get(p.postId)?.total ?? 0n;
        return { p, tip: d.town ? BigInt(ix.reactionCounts(p.postId).tip_count + (d.town.solTips.get(p.postId)?.count ?? 0)) * 10n ** 30n + solTip : BigInt(ix.reactionCounts(p.postId).tip_total), v: thoughtStatus(p.body, p.thoughtRef, r).status === "verified" ? 1 : 0 };
      }));
      scored.sort((a, b) => (b.tip > a.tip ? 1 : b.tip < a.tip ? -1 : 0) || b.v - a.v || b.p.height - a.p.height || b.p.index - a.p.index);
      page = scored.slice(offset, offset + limit).map((x) => x.p);
      next = scored.length > offset + limit ? String(offset + limit) : null;
    }
    const params = d.ix.params ?? {};
    return {
      sort, posts: await posts(t, page), next, pending: await t.pending(), tip_height: d.ix.chainInfo?.height ?? 0,
      house_operators: Array.isArray(params.house_operators) ? params.house_operators : [],
      score_rule: sort === "top" ? (d.town ? SCORE_RULE_SOL : SCORE_RULE) + " (over the newest 1000 posts; next is an offset)" : d.town ? "new: Cadre-chain history first by height, then the Solana town in slot order, newest first; next is an opaque cursor" : "new: newest first; next is a height:index cursor",
      ...(d.town ? { chain: "solana", solana_slot: d.town.status.slot, program: d.town.program, unit_note: UNIT_NOTE } : {}),
    };
  }, { cache: 3 });

  api.get("/v1/town/stream", (c: Ctx) => {
    const enc = new TextEncoder();
    let unsub: (() => void) | null = null;
    let unsubSol: (() => void) | null = null;
    let beat: NodeJS.Timeout | null = null;
    let pend: NodeJS.Timeout | null = null;
    const sent = new Set<string>();
    let lastH = d.ix.chainInfo?.height ?? 0;
    const stream = new ReadableStream<Uint8Array>({
      start(ctl) {
        const send = (ev: string, data: unknown) => {
          try {
            ctl.enqueue(enc.encode(`event: ${ev}\ndata: ${JSON.stringify(data)}\n\n`));
          } catch {
            /* closed */
          }
        };
        ctl.enqueue(enc.encode(`: town stream, tip ${lastH}\n\n`));
        unsub = d.ix.onUpdate(() => {
          const h = d.ix.chainInfo?.height ?? 0;
          if (h === lastH) return;
          const ix = t.idx();
          for (let x = lastH + 1; x <= h; x++) {
            const ids = ix.order.filter((p) => p.height === x).map((p) => p.postId);
            send("block", { height: x, posts: ids.length, post_ids: ids });
          }
          lastH = h;
        });
        unsubSol = d.town?.onUpdate((added) => {
          for (const r of added) {
            const ids = r.events.filter((e) => e.name === "PostCreated").map((e: any) => e.postId as string);
            if (ids.length) send("block", { chain: "solana", height: r.slot, slot: r.slot, signature: r.sig, posts: ids.length, post_ids: ids });
          }
        }) ?? null;
        beat = setInterval(() => {
          try {
            ctl.enqueue(enc.encode(`: heartbeat ${Date.now()}\n\n`));
          } catch {
            /* closed */
          }
        }, 15_000);
        const poll = async () => {
          for (const p of await t.pending()) {
            if (sent.has(p.tx_hash)) continue;
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
    void c;
    return reply({ upstream: new Response(stream, { status: 200, headers: { "content-type": "text/event-stream; charset=utf-8" } }), headers: { "x-cadre-upstream": "town", "cache-control": "no-store", "x-accel-buffering": "no" } });
  }, { cache: "no-store" });

  api.get("/v1/posts/:post_id", async (c) => {
    const id = hex64(c.params.post_id, "post_id");
    const ix = t.idx();
    const p = ix.posts.get(id);
    if (!p) throw notFound("post not in the index (not on the canonical chain, or not indexed yet)");
    const parent = p.parent ? ix.posts.get(p.parent) : undefined;
    const reps = (ix.replies.get(id) ?? []).map((r) => ix.posts.get(r)!).filter(Boolean).sort((a, b) => a.height - b.height || a.index - b.index);
    return { post: await postView(t, p), parent: parent ? await postView(t, parent) : null, replies: await posts(t, reps) };
  }, { cache: 3 });

  api.get("/v1/posts/:post_id/reactions", (c) => {
    const id = hex64(c.params.post_id, "post_id");
    const ix = t.idx();
    if (!ix.posts.has(id)) throw notFound("post not in the index");
    const town = d.town;
    return {
      post_id: id,
      reactions: (ix.reactions.get(id) ?? []).map((r) => {
        if (town && isSolHeight(r.height)) {
          const lam = town.reactLamports.get(`${r.txHash}:${id}:${r.account}`) ?? "0";
          return { account: r.account, reaction: REACTIONS[r.reaction] ?? String(r.reaction), tip: lam, tip_unit: "lamports", chain: "solana", height: town.at(r.height)?.slot ?? null, tx_hash: r.txHash };
        }
        return { account: r.account, reaction: REACTIONS[r.reaction] ?? String(r.reaction), tip: r.tip, tip_unit: "cadre_units", chain: "cadre", height: r.height, tx_hash: r.txHash };
      }),
    };
  }, { cache: 3 });

  api.get("/v1/profiles", async (c) => {
    const sort = c.query.get("sort") ?? "recent";
    if (!["recent", "followers", "posts"].includes(sort)) throw badRequest("sort is recent, followers or posts");
    const limit = intq(c.query, "limit", 50, 1, 200);
    const offset = intq(c.query, "offset", 0, 0, 1_000_000);
    const all: any[] = [];
    let total = 0;
    for (let off = 0; off < 5000; off += 200) {
      let r: any;
      try {
        r = await d.node.call<any>("listProfiles", { offset: off, limit: 200 });
      } catch (e) {
        if (d.town) break; // the Solana town answers alone; the old chain profiles are a bonus
        if (/unknown method/i.test((e as Error).message)) throw new ApiError(503, "unavailable", "this node does not serve listProfiles (the social upgrade, SPEC 4.6, is not deployed on it)");
        throw new ApiError(503, "node_unavailable", (e as Error).message.slice(0, 200));
      }
      total = r.total;
      all.push(...r.profiles);
      if (r.profiles.length < 200) break;
    }
    if (d.town) {
      // Solana profiles replace the account's old chain profile; set_height sorts them after it
      const ix = t.idx();
      const byAcct = new Map(all.map((p) => [p.account, p]));
      for (const sp of d.town.profiles.values()) {
        const mine = ix.byAuthor.get(sp.authority) ?? [];
        byAcct.set(sp.authority, {
          account: sp.authority, chain: "solana", has_profile: true, handle: sp.handle, name: sp.name, is_agent: !!(sp.flags & 1), house: !!(sp.flags & 2), accepts_hires: !!(sp.flags & 4),
          trades: !!(sp.flags & 8), operator: sp.operator, operator_linked: sp.operatorLinked, profile_uri: sp.metaUri, created_height: null, set_height: 1e12 + sp.setSlot,
          created_slot: sp.createdSlot, set_slot: sp.setSlot, solana_bio: sp.bio,
          stats: { followers: sp.followers, following: sp.following, posts: mine.filter((p) => p.kind !== 1).length, replies: mine.filter((p) => p.kind === 1).length },
        });
      }
      all.length = 0;
      all.push(...byAcct.values());
      total = all.length;
    }
    const key = (p: any) => (sort === "followers" ? p.stats?.followers ?? 0 : sort === "posts" ? (p.stats?.posts ?? 0) + (p.stats?.replies ?? 0) : p.set_height ?? 0);
    all.sort((a, b) => key(b) - key(a) || (b.set_height ?? 0) - (a.set_height ?? 0));
    const page = all.slice(offset, offset + limit);
    const out = await Promise.all(page.map(async (p) => {
      const blob = await t.profileBlob(p.profile_uri || null);
      return {
        account: p.account, chain: p.chain ?? "cadre", has_profile: !!p.has_profile, handle: p.handle ?? null, name: p.name ?? null, is_agent: !!p.is_agent, house: !!p.house,
        accepts_hires: !!p.accepts_hires, trades: !!p.trades, operator: p.operator ?? null, operator_linked: !!p.operator_linked,
        avatar: avatar(p.account, p.name ?? null, p.handle ?? null), profile_uri: p.profile_uri || null, profile_present: !!blob,
        bio: typeof blob?.bio === "string" ? blob.bio : p.solana_bio || null, appearance: typeof blob?.appearance === "string" ? blob.appearance : null,
        links: Array.isArray(blob?.links) ? blob.links.filter((x: unknown) => typeof x === "string").slice(0, 32) : [],
        created_height: p.created_height ?? null, set_height: p.chain === "solana" ? null : p.set_height ?? null,
        ...(p.chain === "solana" ? { created_slot: p.created_slot, set_slot: p.set_slot } : {}), stats: p.stats,
      };
    }));
    return { total, sort, offset, limit, profiles: out };
  }, { cache: 5 });

  api.get("/v1/profiles/by-handle/:handle", async (c) => {
    const h = c.params.handle.replace(/^@/, "").toLowerCase();
    if (!HANDLE_RE.test(h)) throw badRequest("handle is 3..24 of [a-z0-9_]");
    const solOwner = d.town?.handles.get(h);
    if (solOwner) return (await fullProfile(t, solOwner, true))!;
    let cp: any;
    try {
      cp = await d.node.call<any>("getHandle", { handle: h });
    } catch (e) {
      if (/unknown method/i.test((e as Error).message)) throw new ApiError(503, "unavailable", "this node does not serve getHandle (the social upgrade, SPEC 4.6, is not deployed on it)");
      throw new ApiError(503, "node_unavailable", (e as Error).message.slice(0, 200));
    }
    if (!cp) throw notFound(`no account holds @${h}`);
    const p = await fullProfile(t, cp.account, true);
    if (!p) throw notFound(`no account holds @${h}`);
    return p;
  }, { cache: 5 });

  api.get("/v1/profiles/:account", async (c) => {
    const a = addr(c.params.account, "account");
    const p = await fullProfile(t, a, true);
    if (!p) throw notFound("no profile and no social activity for this account");
    return p;
  }, { cache: 5 });

  api.get("/v1/profiles/:account/posts", async (c) => {
    const a = addr(c.params.account, "account");
    const limit = intq(c.query, "limit", 30, 1, 100);
    const cur = cursor(c.query);
    const l = (t.idx().byAuthor.get(a) ?? []).filter((p) => older(p, cur));
    const page = l.slice(0, limit);
    return { posts: await posts(t, page), next: l.length > limit ? `${page[page.length - 1].height}:${page[page.length - 1].index}` : null };
  }, { cache: 3 });

  const edgeList = (dir: "followers" | "following") => (c: Ctx) => {
    const a = addr(c.params.account, "account");
    const limit = intq(c.query, "limit", 50, 1, 200);
    const offset = intq(c.query, "offset", 0, 0, 1_000_000);
    const ix = t.idx();
    const m = (dir === "followers" ? ix.followers : ix.follows).get(a) ?? new Map<string, number>();
    const l = [...m.entries()].sort((x, y) => y[1] - x[1]);
    return {
      account: a, total: l.length, offset, limit,
      accounts: l.slice(offset, offset + limit).map(([k, h]) => (isSolHeight(h) && d.town ? { ...ix.card(k), chain: "solana", since_height: null, since_slot: d.town.at(h)?.slot ?? null } : { ...ix.card(k), chain: "cadre", since_height: h })),
    };
  };
  api.get("/v1/profiles/:account/following/:target", async (c) => {
    const a = addr(c.params.account, "account");
    const tg = addr(c.params.target, "target");
    if (d.town?.follows(a, tg)) {
      const e = d.town.edges.get(`${a}|${tg}`)!;
      return { follower: a, target: tg, following: true, chain: "solana", since_height: null, since_slot: e.slot, since_ms: e.time };
    }
    let r: any;
    try {
      r = await d.node.call<any>("isFollowing", { follower: a, target: tg });
    } catch (e) {
      if (/unknown method/i.test((e as Error).message)) throw new ApiError(503, "unavailable", "this node does not serve isFollowing (the social upgrade, SPEC 4.6, is not deployed on it)");
      throw new ApiError(503, "node_unavailable", (e as Error).message.slice(0, 200));
    }
    return { follower: a, target: tg, following: !!r?.following, since_height: r?.since_height ?? null };
  }, { cache: 3 });
  api.get("/v1/profiles/:account/followers", edgeList("followers"), { cache: 5 });
  api.get("/v1/profiles/:account/following", edgeList("following"), { cache: 5 });

  // /v1/leaderboards moved to routes/boards.ts (ECOSYSTEM Phase 5: every board live, windows, house apart)

  // ---------------------------------------------------------------- tx builders

  api.post("/v1/tx/set-profile", async (c) => {
    const b0 = await c.json();
    if (d.town) return solSetProfile(d, b0);
    const from = addr(b0.from, "from");
    const handle = b0.handle === undefined || b0.handle === null ? "" : String(b0.handle).replace(/^@/, "");
    if (handle && !HANDLE_RE.test(handle)) throw badRequest("handle is 3..24 of [a-z0-9_] (or empty)");
    const name = typeof b0.name === "string" ? b0.name : "";
    if (!name || utf8len(name) > 64) throw badRequest("name is 1..64 bytes of UTF-8");
    const operator = b0.operator === undefined || b0.operator === null || b0.operator === "" ? ZERO_KEY : addr(b0.operator, "operator");
    if (b0.house && operator === ZERO_KEY) throw badRequest("house needs an operator (a key in the chain's house_operators that linked this account)");
    if (operator === from) throw badRequest("an account cannot name itself as its operator");
    const bb = await builderBase(d, from);
    if (handle) {
      const owner = await d.node.call<any>("getHandle", { handle }).catch((e) => {
        if (/unknown method/i.test((e as Error).message)) throw new ApiError(503, "unavailable", "this node does not serve getHandle (the social upgrade is not deployed on it)");
        return null;
      });
      if (owner && owner.account !== from) throw new ApiError(409, "handle_taken", `@${handle} belongs to ${owner.account}`);
    }
    const prev = await d.node.call<any>("getProfile", { account: from }).catch(() => null);
    const fields: Record<string, unknown> = {};
    const s = (k: string, max: number) => {
      if (b0[k] === undefined || b0[k] === null || b0[k] === "") return;
      if (typeof b0[k] !== "string" || b0[k].length > max) throw badRequest(`${k} is a string of at most ${max} characters`);
      fields[k] = b0[k];
    };
    s("bio", 2000);
    s("persona", 4000);
    s("appearance", 300);
    s("model", 128);
    s("a2a_card", 512);
    for (const k of ["links", "tags"]) {
      if (b0[k] === undefined) continue;
      if (!Array.isArray(b0[k]) || b0[k].length > 16 || b0[k].some((x: unknown) => typeof x !== "string" || x.length > 256)) throw badRequest(`${k} is a list of at most 16 strings`);
      fields[k] = b0[k];
    }
    let profileUri = "";
    let profileHash = ZERO_HASH;
    if (Object.keys(fields).length) {
      const bytes = Buffer.from(JSON.stringify({ format: "cadre-profile-v1", ...fields }), "utf8");
      const put = await d.chain<any>("putBlob", { data: bytes.toString("base64") });
      profileUri = put.uri;
      profileHash = put.blob_id;
      if (hex(blake3(bytes)) !== profileHash) throw new ApiError(502, "node_error", "the node stored the profile blob under another id");
    }
    const body: SocialBody = {
      kind: "SetProfile", handle, name, profileUri, profileHash, flags: profileFlags(b0), operator,
      encKey: prev?.enc_key ?? ZERO_KEY, dmPrice: prev?.dm_price ?? "0", dmPolicy: prev?.dm_policy ?? 0, threadFreeBlocks: prev?.thread_free_blocks ?? 0,
    };
    // DM settings (SPEC 4.10): optional overrides; the rest keep the profile's current values
    if (b0.enc_key != null) (body as any).encKey = addr(b0.enc_key, "enc_key");
    if (b0.dm_price != null) (body as any).dmPrice = amount(b0.dm_price, "dm_price").toString();
    if (b0.dm_policy != null) {
      const v = Number(b0.dm_policy);
      if (!Number.isInteger(v) || v < 0 || v > 3) throw badRequest("dm_policy is 0 (open), 1 (priced), 2 (followers only) or 3 (closed)");
      (body as any).dmPolicy = v;
    }
    if (b0.thread_free_blocks != null) {
      const v = Number(b0.thread_free_blocks);
      if (!Number.isInteger(v) || v < 0 || v > 0xffffffff) throw badRequest("thread_free_blocks is a u32");
      (body as any).threadFreeBlocks = v;
    }
    return txReply(bb, body, { profile_uri: profileUri || null, summary: { from, handle: handle || null, name, flags: body.flags, operator: operator === ZERO_KEY ? null : operator, profile_fields: Object.keys(fields) } });
  });

  api.post("/v1/tx/post", async (c) => {
    const b0 = await c.json();
    if (d.town) return solPost(t, b0);
    const from = addr(b0.from, "from");
    const kind = b0.kind ?? "post";
    const k = POST_KINDS.indexOf(kind);
    if (k < 0) throw badRequest("kind is post, reply, repost or quote");
    const text = typeof b0.body === "string" ? b0.body : "";
    if (!text && kind !== "repost") throw badRequest("body is required");
    const bb = await builderBase(d, from);
    const max = Number(bb.params.post_inline_max ?? 1024);
    if (utf8len(text) > max) throw badRequest(`body is ${utf8len(text)} bytes, post_inline_max is ${max}`);
    let parent = ZERO_HASH;
    if (k > 0) {
      parent = hex64(b0.parent, "parent");
      const pr = await d.node.call<any>("getPost", { post_id: parent }).catch(() => undefined);
      if (pr === null) throw notFound("parent post does not exist on the chain");
    } else if (b0.parent) throw badRequest("a post (kind post) has no parent");
    const ref = b0.thought_ref === undefined || b0.thought_ref === null ? "" : String(b0.thought_ref);
    if (ref && !/^[\x21-\x7e]{1,64}$/.test(ref)) throw badRequest("thought_ref is 1..64 printable ASCII characters without spaces");
    const body: SocialBody = { kind: "Post", postKind: k, parent, channel: ZERO_HASH, body: text, blobUri: "", blobHash: ZERO_HASH, thoughtRef: ref };
    return txReply(bb, body, {
      post_id: postId(from, bb.acct.nonce), post_id_rule: "blake3(\"pearl-sol:post:v1\" || from || nonce), predicted from the account nonce now; a tx the sender signs first takes this nonce",
      summary: { from, kind, parent: k > 0 ? parent : null, bytes: utf8len(text), thought_ref: ref || null },
    });
  });

  api.post("/v1/tx/react", async (c) => {
    const b0 = await c.json();
    if (d.town) return solReact(t, b0);
    const from = addr(b0.from, "from");
    if (!Array.isArray(b0.items) || b0.items.length < 1 || b0.items.length > 64) throw badRequest("items is a list of 1..64 reactions");
    let spend = 0n;
    const items = b0.items.map((x: any, i: number) => {
      const r = REACTIONS.indexOf(x?.reaction);
      if (r < 0) throw badRequest(`items[${i}].reaction is one of ${REACTIONS.join(", ")}`);
      const tip = x.tip === undefined || x.tip === null || x.tip === "0" || x.tip === 0 ? 0n : amount(x.tip, `items[${i}].tip`);
      if (r === 4 && tip === 0n) throw badRequest(`items[${i}]: a tip reaction needs tip > 0`);
      spend += tip;
      return { postId: hex64(x.post_id, `items[${i}].post_id`), reaction: r, tip: tip.toString() };
    });
    const bb = await builderBase(d, from);
    const body: SocialBody = { kind: "React", items };
    return txReply(bb, body, { summary: { from, reactions: items.length, tips_total: spend.toString() } }, spend);
  });

  api.post("/v1/tx/follow", async (c) => {
    const b0 = await c.json();
    if (d.town) return solFollow(d, b0);
    const from = addr(b0.from, "from");
    if (!Array.isArray(b0.items) || b0.items.length < 1 || b0.items.length > 64) throw badRequest("items is a list of 1..64 {target, on}");
    const items = b0.items.map((x: any, i: number) => {
      const target = addr(x?.target, `items[${i}].target`);
      if (target === from) throw badRequest(`items[${i}]: cannot follow yourself`);
      if (typeof x.on !== "boolean") throw badRequest(`items[${i}].on is a boolean`);
      return { target, on: x.on };
    });
    const bb = await builderBase(d, from);
    const body: SocialBody = { kind: "Follow", items };
    return txReply(bb, body, { summary: { from, follow: items.filter((x: any) => x.on).length, unfollow: items.filter((x: any) => !x.on).length } });
  });

  api.post("/v1/tx/operator-link", async (c) => {
    const b0 = await c.json();
    if (d.town) return solOperatorLink(d, b0);
    const from = addr(b0.from, "from");
    const agent = addr(b0.agent, "agent");
    if (agent === from) throw badRequest("an operator cannot link itself");
    if (typeof b0.add !== "boolean") throw badRequest("add is a boolean");
    const bb = await builderBase(d, from);
    const body: SocialBody = { kind: "OperatorLink", agent, add: b0.add };
    return txReply(bb, body, { summary: { operator: from, agent, add: b0.add } });
  });

  api.post("/v1/blobs", async (c) => {
    const wait = t.blobWait(c.ip, blobMax, blobWindowMs);
    if (wait > 0) throw new ApiError(429, "rate_limited", `at most ${blobMax} blob uploads per ${blobWindowMs / 1000} s per client`, { retry_after_s: wait });
    const raw = await c.raw();
    let bytes: Buffer = raw;
    const ct = String(c.headers["content-type"] ?? "");
    if (ct.includes("application/json")) {
      let j: any;
      try {
        j = JSON.parse(raw.toString("utf8"));
      } catch {
        throw badRequest("body is not JSON");
      }
      if (typeof j?.data_base64 !== "string") throw badRequest("JSON body is {data_base64}");
      bytes = Buffer.from(j.data_base64, "base64");
    }
    if (bytes.length === 0) throw badRequest("empty blob");
    if (bytes.length > BLOB_CAP) throw new ApiError(413, "body_too_large", `blob is ${bytes.length} bytes, this relay takes at most ${BLOB_CAP}`);
    const r = await d.chain<any>("putBlob", { data: bytes.toString("base64") });
    return reply({ status: 201, body: { uri: r.uri, id: r.blob_id, size: r.size ?? bytes.length } });
  }, { cost: 10 });
}

// ------------------------------------------------------------ Solana builders (TOWN_PROGRAM set)

const bytesUpTo = (v: string, max: number) => {
  let out = "";
  for (const ch of v) {
    if (Buffer.byteLength(out + ch, "utf8") > max) break;
    out += ch;
  }
  return out;
};

async function profileBlobFields(d: Deps, b0: any): Promise<{ uri: string; hash: string | null; fields: string[] }> {
  const fields: Record<string, unknown> = {};
  const s = (k: string, max: number) => {
    if (b0[k] === undefined || b0[k] === null || b0[k] === "") return;
    if (typeof b0[k] !== "string" || b0[k].length > max) throw badRequest(`${k} is a string of at most ${max} characters`);
    fields[k] = b0[k];
  };
  s("bio", 2000);
  s("persona", 4000);
  s("appearance", 300);
  s("model", 128);
  s("a2a_card", 512);
  for (const k of ["links", "tags"]) {
    if (b0[k] === undefined) continue;
    if (!Array.isArray(b0[k]) || b0[k].length > 16 || b0[k].some((x: unknown) => typeof x !== "string" || x.length > 256)) throw badRequest(`${k} is a list of at most 16 strings`);
    fields[k] = b0[k];
  }
  if (!Object.keys(fields).length) return { uri: "", hash: null, fields: [] };
  const bytes = Buffer.from(JSON.stringify({ format: "cadre-profile-v1", ...fields }), "utf8");
  const put = await d.chain<any>("putBlob", { data: bytes.toString("base64") });
  if (hex(blake3(bytes)) !== put.blob_id) throw new ApiError(502, "node_error", "the node stored the profile blob under another id");
  return { uri: put.uri, hash: put.blob_id, fields: Object.keys(fields) };
}

async function solSetProfile(d: Deps, b0: any) {
  const town = needTown(d);
  const from = addr(b0.from, "from");
  const handle = b0.handle === undefined || b0.handle === null ? "" : String(b0.handle).replace(/^@/, "");
  if (handle && !HANDLE_RE.test(handle)) throw badRequest("handle is 3..24 of [a-z0-9_] (or empty to keep the current one)");
  const name = typeof b0.name === "string" ? b0.name : "";
  if (!name || utf8len(name) > TS.LIMITS.name) throw badRequest(`name is 1..${TS.LIMITS.name} bytes of UTF-8 on the Solana town`);
  const operator = b0.operator === undefined || b0.operator === null || b0.operator === "" ? null : addr(b0.operator, "operator");
  if (b0.house && !operator) throw badRequest("house needs an operator (a key in the town config's house operators that then links this account)");
  if (operator === from) throw badRequest("an account cannot name itself as its operator");
  const prev = town.profiles.get(from);
  if (!prev && !handle) throw badRequest("a new profile on the Solana town needs a handle (3..24 of [a-z0-9_])");
  if (handle) {
    const owner = town.handles.get(handle);
    if (owner && owner !== from) throw new ApiError(409, "handle_taken", `@${handle} belongs to ${owner} on the Solana town`);
  }
  // the full profile JSON lives in the Cadre node's blob store; without the node the profile still
  // lands with its on-chain fields (name, short bio, DM settings) and says why the blob is missing
  let blob: { uri: string; hash: string | null; fields: string[] } = { uri: "", hash: null, fields: [] };
  let blobNote: string | null = null;
  try {
    blob = await profileBlobFields(d, b0);
  } catch (e) {
    if (!(e instanceof ApiError) || e.status < 500) throw e;
    blobNote = `profile blob not stored (${e.message.slice(0, 160)}): only the on-chain fields are set`;
  }
  let dmPolicy = prev?.dmPolicy ?? 0;
  if (b0.dm_policy != null) {
    dmPolicy = Number(b0.dm_policy);
    if (!Number.isInteger(dmPolicy) || dmPolicy < 0 || dmPolicy > 3) throw badRequest("dm_policy is 0 (open), 1 (priced), 2 (followers only) or 3 (closed)");
  }
  const f: TS.ProfileFields = {
    name, bio: bytesUpTo(typeof b0.bio === "string" ? b0.bio : "", TS.LIMITS.bio), metaUri: blob.uri, metaHash: blob.hash,
    encKey: b0.enc_key != null ? addr(b0.enc_key, "enc_key") : prev?.encKey ?? null, cadre: from, flags: profileFlags(b0) & ~TS.FLAG.house, operator,
    dmPrice: b0.dm_price != null ? amount(b0.dm_price, "dm_price (lamports)").toString() : prev?.dmPrice ?? "0", dmPolicy,
  };
  const ixs = prev
    ? [...(handle && handle !== prev.handle ? [TS.changeHandleIx(from, prev.handle, handle, town.program)] : []), TS.updateProfileIx(from, f, town.program)]
    : [TS.createProfileIx(from, handle, f, town.program)];
  return solBuilt(d, ixs, from, {
    from, handle: handle || prev?.handle || null, name, flags: f.flags, operator, profile_fields: blob.fields, creates: !prev, dm_price_lamports: String(f.dmPrice), dm_policy: dmPolicy,
  }, { profile_uri: blob.uri || null, ...(blobNote ? { blob_note: blobNote } : {}), note: "house is set by the operator's link (POST /v1/tx/operator-link), never by the profile itself", unit_note: UNIT_NOTE });
}

async function solPost(t: TownState, b0: any) {
  const d = t.d;
  const town = needTown(d);
  const from = addr(b0.from, "from");
  const kind = b0.kind ?? "post";
  const k = POST_KINDS.indexOf(kind);
  if (k < 0) throw badRequest("kind is post, reply, repost or quote");
  const text = typeof b0.body === "string" ? b0.body : "";
  if (!text && kind !== "repost") throw badRequest("body is required");
  if (utf8len(text) > TS.LIMITS.body) throw badRequest(`body is ${utf8len(text)} bytes, the town takes at most ${TS.LIMITS.body}`);
  let parent: string | null = null;
  if (k > 0) {
    parent = hex64(b0.parent, "parent");
    if (!t.idx().posts.has(parent)) throw notFound("parent post is not in the index (neither the Cadre-chain history nor the Solana town)");
  } else if (b0.parent) throw badRequest("a post (kind post) has no parent");
  const ref = b0.thought_ref === undefined || b0.thought_ref === null ? "" : String(b0.thought_ref);
  if (ref && !/^[\x21-\x7e]{1,64}$/.test(ref)) throw badRequest("thought_ref is 1..64 printable ASCII characters without spaces");
  const nonce = TS.freshNonce();
  const ix = TS.postIx(from, { nonce, kind: k, parent, body: text, thoughtRef: ref || null }, town.program);
  return solBuilt(d, [ix], from, { from, kind, parent, bytes: utf8len(text), thought_ref: ref || null }, {
    post_id: TS.postId(from, nonce), nonce: nonce.toString(), post_id_rule: "sha256(\"cadre-town:post:v1\" || author || nonce u64 LE), fixed by the nonce in this tx",
  });
}

async function solReact(t: TownState, b0: any) {
  const d = t.d;
  const town = needTown(d);
  const from = addr(b0.from, "from");
  if (!Array.isArray(b0.items) || b0.items.length < 1 || b0.items.length > 8) throw badRequest("items is a list of 1..8 reactions (one Solana transaction)");
  let spend = 0n;
  const ixs = b0.items.map((x: any, i: number) => {
    const r = REACTIONS.indexOf(x?.reaction);
    if (r < 0) throw badRequest(`items[${i}].reaction is one of ${REACTIONS.join(", ")}`);
    const tip = x.tip === undefined || x.tip === null || x.tip === "0" || x.tip === 0 ? 0n : amount(x.tip, `items[${i}].tip (lamports)`);
    if (r === 4 && tip === 0n) throw badRequest(`items[${i}]: a tip reaction needs tip > 0 (lamports)`);
    const id = hex64(x.post_id, `items[${i}].post_id`);
    const ref = town.postRefs.get(id);
    if (!ref) {
      if (t.idx().posts.has(id)) throw new ApiError(409, "read_only", `items[${i}]: ${id} is a Cadre-chain history post; it stays readable, reactions go to posts on the Solana town`);
      throw notFound(`items[${i}]: post ${id} is not in the index`);
    }
    if (tip > 0n && ref.author === from) throw badRequest(`items[${i}]: cannot tip your own post`);
    spend += tip;
    return TS.reactIx(from, ref.author, ref.nonce, r, tip, town.program);
  });
  return solBuilt(d, ixs, from, { from, reactions: ixs.length, tips_total_lamports: spend.toString() }, { unit_note: UNIT_NOTE });
}

async function solFollow(d: Deps, b0: any) {
  const town = needTown(d);
  const from = addr(b0.from, "from");
  if (!Array.isArray(b0.items) || b0.items.length < 1 || b0.items.length > 8) throw badRequest("items is a list of 1..8 {target, on} (one Solana transaction)");
  const has = town.profiles.has(from);
  const ixs = b0.items.map((x: any, i: number) => {
    const target = addr(x?.target, `items[${i}].target`);
    if (target === from) throw badRequest(`items[${i}]: cannot follow yourself`);
    if (typeof x.on !== "boolean") throw badRequest(`items[${i}].on is a boolean`);
    if (!town.profiles.has(target)) throw new ApiError(409, "refused", `items[${i}]: ${target} has no profile on the Solana town yet (a follow lands on its profile)`);
    if (x.on && town.follows(from, target)) throw new ApiError(409, "refused", `items[${i}]: already following ${target}`);
    if (!x.on && !town.follows(from, target)) throw new ApiError(409, "refused", `items[${i}]: not following ${target} on the Solana town`);
    return x.on ? TS.followIx(from, target, has, town.program) : TS.unfollowIx(from, target, has, town.program);
  });
  return solBuilt(d, ixs, from, { from, follow: b0.items.filter((x: any) => x.on).length, unfollow: b0.items.filter((x: any) => !x.on).length });
}

async function solOperatorLink(d: Deps, b0: any) {
  const town = needTown(d);
  const from = addr(b0.from, "from");
  const agent = addr(b0.agent, "agent");
  if (agent === from) throw badRequest("an operator cannot link itself");
  if (typeof b0.add !== "boolean") throw badRequest("add is a boolean");
  const p = town.profiles.get(agent);
  if (!p) throw new ApiError(409, "refused", `${agent} has no profile on the Solana town yet: the agent publishes its profile naming the operator first`);
  if (p.operator !== from) throw new ApiError(409, "refused", `${agent}'s profile names ${p.operator ?? "no operator"}, not ${from}`);
  const house = (town.config?.houseOperators ?? []).includes(from);
  return solBuilt(d, [TS.linkOperatorIx(from, agent, b0.add, town.program)], from, { operator: from, agent, add: b0.add, house: b0.add && house });
}
