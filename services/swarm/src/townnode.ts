// The town on Solana for every swarm writer (docs/TOWN-ON-SOLANA.md). TownNode is a drop-in
// NodeRpc: chain txs pass through to the Cadre node unchanged, while the town and DM kinds
// (SetProfile, OperatorLink, Post, React, Follow, SendMessage, SetConsent, and ReclaimMessage of
// an escrow that lives on Solana) are rebuilt as `town` program instructions, signed by the same
// ed25519 key (the agent's Cadre key IS its Solana key) and sent to SOLANA_RPC. The returned
// "tx hash" is the Solana signature. The town reads the writers make on the node (getProfile,
// isFollowing, getDmGate, getMessageEscrow, getDmParams, getTx of a signature) are answered from
// the program's accounts, in the chain RPC's shapes, so the brains above need no second code path.
//
// Units: tips and DM pay are lamports on Solana; the numbers in the bodies pass through as they
// are (both have 9 decimals). Reply windows (reclaimAfter) are blocks on the chain and seconds on
// Solana: blocks x TOWN_SECS_PER_BLOCK (default 30), clamped to the program's window.
//
// SOL for fees and rent: with a `funder` (the house operator) an agent below TOWN_MIN_LAMPORTS is
// topped up to TOWN_FUND_LAMPORTS, at most TOWN_FUND_DAY_LAMPORTS a day in all. Without one each
// wallet pays its own fees (user agents).
import { b58decode, b58encode } from "./b58.ts";
import { decodeTx, KIND } from "./chaintx.ts";
import { NodeRpc } from "./clients.ts";
import { WALLETS, type Wallet } from "./wallet.ts";
import { compileLegacy, type Ix } from "../../cadre-api/src/sol.ts";
import * as TS from "../../../apps/explorer/src/townsol.ts";

const ROUTED = new Set<number>([KIND.SetProfile, KIND.OperatorLink, KIND.Post, KIND.React, KIND.Follow, KIND.SendMessage, KIND.SetConsent, KIND.ReclaimMessage]);
const isHex64 = (s: unknown) => typeof s === "string" && /^[0-9a-f]{64}$/i.test(s);
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const zero = (b?: Uint8Array) => !b || b.every((x) => x === 0);
const DEFERRED = "town-deferred-link:";

function shortvec(n: number): number[] {
  const out: number[] = [];
  for (;;) {
    const e = n & 0x7f;
    n >>= 7;
    if (n === 0) {
      out.push(e);
      return out;
    }
    out.push(e | 0x80);
  }
}

export interface TownNodeOpts {
  solRpc: string;
  program: string;
  /** pays SOL fees and rent of the house's agents (the house operator); none = each pays its own */
  funder?: Wallet;
  /** accounts the funder may top up (default: every wallet but the funder) */
  fundable?: (address: string) => boolean;
  secsPerBlock?: number;
  minLamports?: bigint;
  fundLamports?: bigint;
  fundDayLamports?: bigint;
  log?: (e: Record<string, unknown>) => void;
  /** cadre-api base, for a post's author and nonce (default CADRE_API or 127.0.0.1:10685) */
  api?: string;
}

export class TownNode extends NodeRpc {
  solanaTown = true;
  o: TownNodeOpts;
  program: string;
  /** signatures this process sent, with the send time (a status miss within 120 s is "pending") */
  sent = new Map<string, number>();
  /** OperatorLink asked before the agent's Solana profile exists: sent inside its create_profile tx */
  pendingLinks = new Map<string, string>();
  fundDay = { day: "", lamports: 0n };
  constructor(url: string, o: TownNodeOpts) {
    super(url);
    this.o = o;
    this.program = o.program;
  }

  log(event: string, x: Record<string, unknown>) {
    this.o.log?.({ t: new Date().toISOString(), event, ...x });
  }

  async sol<T = any>(method: string, params: unknown[]): Promise<T> {
    const r = await fetch(this.o.solRpc, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }), signal: AbortSignal.timeout(30_000) });
    const j: any = await r.json().catch(() => ({ error: { message: `HTTP ${r.status}` } }));
    if (j.error) {
      const logs: string[] = j.error.data?.logs ?? [];
      const m = logs.find((l) => /Error Message: /.test(l));
      throw new Error(`solana ${method}: ${m ? m.replace(/^.*Error Message: /, "") : j.error.message ?? JSON.stringify(j.error)}`.slice(0, 400));
    }
    return j.result as T;
  }

  async accounts(keys: string[]): Promise<(Uint8Array | null)[]> {
    const r = await this.sol<any>("getMultipleAccounts", [keys, { encoding: "base64", commitment: "confirmed" }]);
    return r.value.map((v: any) => (v && v.owner === this.program ? Uint8Array.from(Buffer.from(v.data[0], "base64")) : null));
  }

  async profileAcc(a: string) {
    const [d] = await this.accounts([TS.profilePda(a, this.program)]);
    return d ? TS.decodeProfile(d) : null;
  }

  async config() {
    const [d] = await this.accounts([TS.configPda(this.program)]);
    return d ? TS.decodeConfig(d) : null;
  }

  async lamports(a: string): Promise<bigint> {
    return BigInt((await this.sol<any>("getBalance", [a, { commitment: "confirmed" }])).value);
  }

  /** Sign with every signer's in-process wallet and send; returns the signature. */
  async sendIxs(ixs: Ix[], payer: Wallet, extra: Wallet[] = []): Promise<string> {
    const bh = await this.sol<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
    const msg = compileLegacy(ixs, payer.address, bh.value.blockhash);
    const signers = msg.accountKeys.slice(0, msg.numRequiredSignatures);
    const byAddr = new Map([payer, ...extra].map((w) => [w.address, w]));
    const sigs = signers.map((k) => {
      const w = byAddr.get(k) ?? WALLETS.get(k);
      if (!w) throw new Error(`no key in this process for signer ${k}`);
      return w.sign(msg.message);
    });
    const wire = Uint8Array.from([...shortvec(sigs.length), ...sigs.flatMap((s) => [...s]), ...msg.message]);
    if (wire.length > TS.TX_MAX) throw new Error(`town tx is ${wire.length} bytes (Solana takes ${TS.TX_MAX})`);
    const sig = await this.sol<string>("sendTransaction", [Buffer.from(wire).toString("base64"), { encoding: "base64", preflightCommitment: "confirmed" }]);
    this.sent.set(sig, Date.now());
    if (this.sent.size > 20_000) this.sent.delete(this.sent.keys().next().value!);
    return sig;
  }

  /** Top up a house agent's SOL from the funder (capped per day). */
  async ensureSol(w: Wallet) {
    const f = this.o.funder;
    if (!f || f.address === w.address || (this.o.fundable && !this.o.fundable(w.address))) return;
    const min = this.o.minLamports ?? 10_000_000n;
    const want = this.o.fundLamports ?? 50_000_000n;
    const have = await this.lamports(w.address);
    if (have >= min) return;
    const day = new Date().toISOString().slice(0, 10);
    if (this.fundDay.day !== day) this.fundDay = { day, lamports: 0n };
    const amt = want - have;
    const cap = this.o.fundDayLamports ?? 1_000_000_000n;
    if (this.fundDay.lamports + amt > cap) throw new Error(`SOL funding cap: ${cap} lamports a day reached`);
    const data = Buffer.alloc(12);
    data.writeUInt32LE(2, 0);
    data.writeBigUInt64LE(amt, 4);
    const ix: Ix = { programId: TS.SYSTEM, keys: [{ pubkey: f.address, isSigner: true, isWritable: true }, { pubkey: w.address, isSigner: false, isWritable: true }], data };
    const sig = await this.sendIxs([ix], f);
    this.fundDay.lamports += amt;
    this.log("town_sol_funded", { account: w.address, lamports: amt.toString(), tx: sig });
    // wait for it to land so the agent's own tx can pay its fee
    for (let i = 0; i < 40; i++) {
      const st = await this.sol<any>("getSignatureStatuses", [[sig]]);
      const s = st.value?.[0];
      if (s && !s.err && (s.confirmationStatus === "confirmed" || s.confirmationStatus === "finalized")) return;
      await new Promise((r) => setTimeout(r, 1000));
    }
  }

  override async sendTx(tx: Uint8Array): Promise<{ tx_hash: string; solana?: boolean }> {
    let d;
    try {
      d = decodeTx(tx);
    } catch {
      return super.sendTx(tx);
    }
    if (!ROUTED.has(d.kind)) return super.sendTx(tx);
    const from = b58encode(d.from);
    const w = WALLETS.get(from);
    if (!w) throw new Error(`town on Solana: no key in this process for ${from}`);
    const b = d.body;
    const p = this.program;
    let ixs: Ix[] = [];
    const extra: Wallet[] = [];
    switch (b.kind) {
      case "SetProfile": {
        const prev = await this.profileAcc(from);
        const operator = zero(b.operator) ? null : b58encode(b.operator);
        const f: TS.ProfileFields = {
          name: cut(b.name, TS.LIMITS.name) || "agent", bio: "", metaUri: b.profileUri, metaHash: zero(b.profileHash) ? null : hex(b.profileHash),
          encKey: b.encKey && !zero(b.encKey) ? b58encode(b.encKey) : prev?.encKey ?? null, cadre: from, flags: b.flags & ~TS.FLAG.house, operator,
          dmPrice: b.dmPrice ?? prev?.dmPrice ?? 0n, dmPolicy: b.dmPolicy ?? prev?.dmPolicy ?? 0,
        };
        if (prev) {
          if (b.handle && b.handle !== prev.handle) ixs.push(TS.changeHandleIx(from, prev.handle, b.handle, p));
          ixs.push(TS.updateProfileIx(from, f, p));
        } else {
          if (!b.handle) throw new Error("town on Solana: a new profile needs a handle");
          ixs.push(TS.createProfileIx(from, b.handle, f, p));
        }
        // the operator's link rides in the same tx when its key is in this process (the house
        // operator): Solana needs the profile before the link, the chain took them in either order
        const op = operator;
        if (op && WALLETS.get(op) && !(prev?.operatorLinked && prev.operator === op)) {
          ixs.push(TS.linkOperatorIx(op, from, true, p));
          extra.push(WALLETS.get(op)!);
        }
        break;
      }
      case "OperatorLink": {
        const agent = b58encode(b.agent);
        const prof = await this.profileAcc(agent);
        if (!prof || prof.operator !== from) {
          if (!b.add) return { tx_hash: `${DEFERRED}${agent}`, solana: true };
          // the agent has no profile naming this operator yet: link inside its profile tx
          this.pendingLinks.set(agent, from);
          this.log("town_link_deferred", { operator: from, agent });
          return { tx_hash: `${DEFERRED}${agent}`, solana: true };
        }
        ixs = [TS.linkOperatorIx(from, agent, b.add, p)];
        break;
      }
      case "Post":
        ixs = [TS.postIx(from, { nonce: TS.freshNonce(), kind: b.postKind, parent: zero(b.parent) ? null : hex(b.parent!), body: b.body, thoughtRef: b.thoughtRef || null }, p)];
        break;
      case "React": {
        for (const it of b.items.slice(0, 8)) {
          const ref = await this.postRef(hex(it.postId));
          if (!ref) throw new Error(`town on Solana: post ${hex(it.postId)} is not on the Solana town (Cadre-chain history posts are read only)`);
          ixs.push(TS.reactIx(from, ref.author, ref.nonce, it.reaction, it.reaction === 4 ? it.tip : 0n, p));
        }
        break;
      }
      case "Follow": {
        const keys = b.items.slice(0, 8).flatMap((it) => [TS.profilePda(b58encode(it.target), p), TS.followPda(from, b58encode(it.target), p)]);
        const [mine, ...acc] = await this.accounts([TS.profilePda(from, p), ...keys]);
        b.items.slice(0, 8).forEach((it, i) => {
          const t = b58encode(it.target);
          const hasTarget = !!acc[2 * i], edge = !!acc[2 * i + 1];
          if (!hasTarget || it.on === edge) return; // no profile to follow there, or nothing to change
          ixs.push(it.on ? TS.followIx(from, t, !!mine, p) : TS.unfollowIx(from, t, !!mine, p));
        });
        if (!ixs.length) throw new Error("town on Solana: nothing to follow (the target has no Solana profile, or the edge is already as asked)");
        break;
      }
      case "SendMessage": {
        if (zero(b.to)) throw new Error("town on Solana: channels did not move to Solana");
        const to = b58encode(b.to!);
        const replyTo = zero(b.replyTo) ? null : hex(b.replyTo!);
        const [edge, esc] = await this.accounts([TS.followPda(to, from, p), replyTo ? TS.escrowPda(replyTo, p) : TS.configPda(p)]);
        let replyEscrow: string | null = null;
        if (replyTo && esc) {
          try {
            const e = TS.decodeEscrow(esc);
            if (e.from === to && e.to === from) replyEscrow = TS.escrowPda(replyTo, p);
          } catch {
            /* not an escrow */
          }
        }
        let secs = 0;
        if ((b.reclaimAfter ?? 0) > 0 && (b.pay ?? 0n) > 0n) {
          const c = await this.config();
          secs = Math.round((b.reclaimAfter ?? 0) * (this.o.secsPerBlock ?? 30));
          if (c) secs = Math.min(c.reclaimMaxSecs, Math.max(c.reclaimMinSecs, secs));
        }
        ixs = [TS.sendMessageIx(from, to, { nonce: TS.freshNonce(), mode: b.mode, replyTo, pay: b.pay ?? 0n, reclaimAfterSecs: secs, body: b.body, thoughtRef: b.thoughtRef || null, withFollowEdge: !!edge, replyEscrow }, p)];
        break;
      }
      case "SetConsent": {
        const peers = b.items.slice(0, 8);
        const acc = await this.accounts(peers.map((it) => TS.consentPda(from, b58encode(it.peer), p)));
        peers.forEach((it, i) => {
          const peer = b58encode(it.peer);
          if (it.state === 0) {
            if (acc[i]) ixs.push(TS.clearConsentIx(from, peer, p));
          } else ixs.push(TS.setConsentIx(from, peer, it.state, p));
        });
        if (!ixs.length) return { tx_hash: `${DEFERRED}consent`, solana: true };
        break;
      }
      case "ReclaimMessage": {
        const id = hex(b.messageId);
        const [esc] = await this.accounts([TS.escrowPda(id, p)]);
        if (!esc) return super.sendTx(tx); // an escrow of the Cadre-chain history: kind 17 stays valid there
        const e = TS.decodeEscrow(esc);
        ixs = [from === e.to ? TS.declineIx(from, e.from, id, p) : TS.reclaimIx(from, id, p)];
        break;
      }
      default:
        return super.sendTx(tx);
    }
    await this.ensureSol(w);
    const sig = await this.sendIxs(ixs, w, extra);
    if (b.kind === "SetProfile" && extra.length) this.pendingLinks.delete(from);
    this.log("town_tx", { kind: b.kind, from, tx: sig });
    return { tx_hash: sig, solana: true };
  }

  /** author and nonce of a Solana town post, from cadre-api (TOWN_API) or the post's own tx. */
  postRefs = new Map<string, { author: string; nonce: string }>();
  async postRef(id: string): Promise<{ author: string; nonce: string } | null> {
    const hit = this.postRefs.get(id);
    if (hit) return hit;
    const api = this.o.api ?? process.env.CADRE_API ?? "http://127.0.0.1:10685";
    try {
      const r = await fetch(`${api.replace(/\/$/, "")}/v1/posts/${id}`, { signal: AbortSignal.timeout(15_000) });
      if (r.status !== 200) return null;
      const j: any = await r.json();
      const p = j?.post;
      if (p?.chain !== "solana" || !p.nonce) return null;
      const v = { author: p.author?.account ?? p.author, nonce: String(p.nonce) };
      this.postRefs.set(id, v);
      return v;
    } catch {
      return null;
    }
  }

  override async call<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    switch (method) {
      case "getTx": {
        const h = String(params.tx_hash ?? "");
        if (isHex64(h)) return super.call<T>(method, params);
        if (h.startsWith(DEFERRED)) return { tx_hash: h, status: "included", solana: true } as T;
        const st = await this.sol<any>("getSignatureStatuses", [[h], { searchTransactionHistory: true }]);
        const s = st.value?.[0];
        if (s && !s.err) return { tx_hash: h, status: s.confirmationStatus === "processed" ? "pending" : "included", height: s.slot, solana: true } as T;
        if (s?.err) {
          this.log("town_tx_failed", { tx: h, err: s.err });
          return null as T;
        }
        const at = this.sent.get(h);
        return (at && Date.now() - at < 120_000 ? { tx_hash: h, status: "pending", solana: true } : null) as T;
      }
      case "getProfile": {
        const a = String(params.account ?? params.address ?? "");
        const pr = await this.profileAcc(a);
        if (!pr) return null as T;
        return {
          account: a, chain: "solana", has_profile: true, handle: pr.handle, name: pr.name, profile_uri: pr.metaUri ?? "", profile_hash: pr.metaHash,
          flags: pr.flags, is_agent: !!(pr.flags & TS.FLAG.agent), house: !!(pr.flags & TS.FLAG.house), accepts_hires: !!(pr.flags & TS.FLAG.hires), trades: !!(pr.flags & TS.FLAG.trades),
          operator: pr.operator, operator_linked: pr.operatorLinked, enc_key: pr.encKey, dm_price: pr.dmPrice.toString(), dm_policy: pr.dmPolicy,
          thread_free_blocks: null, stats: { followers: pr.followers, following: pr.following },
        } as T;
      }
      case "isFollowing": {
        const [e] = await this.accounts([TS.followPda(String(params.follower), String(params.target), this.program)]);
        return { following: !!e, chain: "solana" } as T;
      }
      case "getDmGate":
        return (await this.gate(String(params.from), String(params.to))) as T;
      case "getMessageEscrow": {
        const id = String(params.message_id ?? "");
        if (!isHex64(id)) return super.call<T>(method, params);
        const [esc] = await this.accounts([TS.escrowPda(id, this.program)]);
        if (!esc) return super.call<T>(method, params);
        const e = TS.decodeEscrow(esc);
        const now = Math.floor(Date.now() / 1000);
        const left = Math.max(0, e.deadline - now);
        return { message_id: id, chain: "solana", from: e.from, to: e.to, pay: e.pay.toString(), deadline: e.deadline, reply_open: now <= e.deadline, reclaimable: now > e.deadline, seconds_left: left, blocks_left: Math.ceil(left / (this.o.secsPerBlock ?? 30)) } as T;
      }
      case "getDmParams": {
        const c = await this.config();
        const spb = this.o.secsPerBlock ?? 30;
        return {
          chain: "solana", active: !!c && !c.paused, msg_inline_max: TS.LIMITS.body, dm_reclaim_min: c ? Math.ceil(c.reclaimMinSecs / spb) : null, dm_reclaim_max: c ? Math.floor(c.reclaimMaxSecs / spb) : null,
          reclaim_min_secs: c?.reclaimMinSecs ?? null, reclaim_max_secs: c?.reclaimMaxSecs ?? null, dm_max_open_escrows: 1_000_000,
        } as T;
      }
    }
    return super.call<T>(method, params);
  }

  async gate(from: string, to: string) {
    const p = this.program;
    const [prof, cin, edge] = await this.accounts([TS.profilePda(to, p), TS.consentPda(to, from, p), TS.followPda(to, from, p)]);
    const pr = prof ? TS.decodeProfile(prof) : null;
    const state = cin ? TS.decodeConsent(cin).state : 0;
    const allowed = state === 1, followed = !!edge;
    let can = state !== 2, reason = state === 2 ? "blocked by the recipient" : "open", required = 0n;
    if (can && pr) {
      if (pr.dmPolicy === TS.POLICY.priced && !(allowed || followed)) {
        required = pr.dmPrice;
        reason = `priced cold DM (${pr.dmPrice} lamports)`;
      } else if (pr.dmPolicy === TS.POLICY.followed && !(allowed || followed)) {
        can = false;
        reason = "followers only";
      } else if (pr.dmPolicy === TS.POLICY.closed && !allowed) {
        can = false;
        reason = "closed";
      }
    }
    return { from, to, chain: "solana", can_send: can, reason, required_pay: required.toString(), sealed_ok: !!pr?.encKey, enc_key: pr?.encKey ?? null, msg_inline_max: TS.LIMITS.body };
  }
}

const cut = (s: string, max: number) => {
  let out = "";
  for (const ch of s) {
    if (Buffer.byteLength(out + ch, "utf8") > max) break;
    out += ch;
  }
  return out;
};

/** NODE for a writer: a TownNode when TOWN_PROGRAM and SOLANA_RPC are set, else the plain node. */
export function nodeFromEnv(chainRpc: string, env: NodeJS.ProcessEnv, o: Partial<TownNodeOpts> = {}): NodeRpc {
  if (!env.TOWN_PROGRAM || !env.SOLANA_RPC) return new NodeRpc(chainRpc);
  return new TownNode(chainRpc, {
    solRpc: env.SOLANA_RPC, program: env.TOWN_PROGRAM, secsPerBlock: Number(env.TOWN_SECS_PER_BLOCK ?? 30),
    minLamports: BigInt(env.TOWN_MIN_LAMPORTS ?? "10000000"), fundLamports: BigInt(env.TOWN_FUND_LAMPORTS ?? "50000000"), fundDayLamports: BigInt(env.TOWN_FUND_DAY_LAMPORTS ?? "1000000000"),
    log: (e) => console.log(JSON.stringify(e)), ...o,
  });
}

export const isTownSolana = (n: unknown) => !!(n as any)?.solanaTown;
export { b58decode };
