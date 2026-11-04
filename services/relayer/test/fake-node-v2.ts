// A small in-process Cadre chain for the RH side tests: the bridge v2 RPC shapes
// (docs/BRIDGE-V2.md 6) behind NodeV2's swappable transport, JSON round-tripped. Applies
// BridgeUnlockV2 (attester sigs over the v2 messages, order jobs) and BridgeLockV2 (v2
// withdrawal entries, per-domain counters). Blocks carry a 256-byte header whose hash is
// blake3(header), roots at the SPEC 3.1 offsets, so the header-oracle poster's checks run.
// Proofs are opaque (`proof:<domain>:<nonce>@<height>`); the fake RH bridge checks the same string.

import { createPublicKey, randomBytes, verify as edVerify } from "node:crypto";
import { blake3 } from "@noble/hashes/blake3";
import { PublicKey } from "@solana/web3.js";
import { KIND, decodeTx, encodePostJobBody, framed, TX_NS, txHash } from "../src/agent-tx.ts";
import { UNITS_PER_SPL, depositMessageV2, depositOrderMessageV2, encodeWithdrawalV2, orderJobIdV2, sha256 } from "../src/codec.ts";
import { NodeV2 } from "../src/evm/node-v2.ts";

const SPKI = Buffer.from("302a300506032b6570032100", "hex");
const edOk = (pk: PublicKey, msg: Buffer, sig: Buffer): boolean =>
  edVerify(null, msg, createPublicKey({ key: Buffer.concat([SPKI, pk.toBuffer()]), format: "der", type: "spki" }), sig);

export interface FDomain {
  domain: number;
  extChainId: bigint;
  bridgeAddr: string;
  paused: boolean;
}

interface FJob {
  jobId: string;
  poster: string;
  status: "open" | "delivered" | "completed" | "expired";
  deliverer: string | null;
  resultHash: string | null;
  resultUri: string;
  closedHeight: number;
}

interface FW {
  domain: number;
  nonce: bigint;
  entry: Buffer;
  recipient: Buffer;
  amountExt: bigint;
  height: number;
  from: string;
}

export const TOTAL = 1_000_000_000_000_000_000n;

export class FakeNodeV2 {
  readonly chainId: number;
  K: number;
  attesters: PublicKey[];
  threshold: number;
  minProtocolFee = 2n;
  v2Active = true;
  domains: FDomain[];
  height = 0;
  blocks: { hash: string; header: Buffer }[] = [];
  accounts = new Map<string, { balance: bigint; nonce: bigint }>();
  escrow = TOTAL;
  deposits = new Map<string, { height: number; recipient: string; amountExt: bigint }>();
  withdrawals: FW[] = [];
  stats = new Map<number, { unlocked: bigint; locked: bigint; deposits: number; withdrawals: number }>();
  jobs = new Map<string, FJob>();
  pool: Buffer[] = [];
  included = new Map<string, number>();
  refused: string[] = [];
  calls: string[] = [];

  constructor(o: { chainId?: number; K?: number; attesters: PublicKey[]; threshold?: number; domains: FDomain[] }) {
    this.chainId = o.chainId ?? 1128350803;
    this.K = o.K ?? 3;
    this.attesters = o.attesters;
    this.threshold = o.threshold ?? 1;
    this.domains = o.domains;
    this.mine(1);
  }

  rpc(): NodeV2 {
    return new NodeV2(async (method, params) => {
      this.calls.push(method);
      return JSON.parse(JSON.stringify(this.handle(method, JSON.parse(JSON.stringify(params))), (_k, v) => (typeof v === "bigint" ? v.toString() : v)));
    });
  }

  acct(k: string) {
    let a = this.accounts.get(k);
    if (!a) this.accounts.set(k, (a = { balance: 0n, nonce: 0n }));
    return a;
  }

  stat(d: number) {
    let s = this.stats.get(d);
    if (!s) this.stats.set(d, (s = { unlocked: 0n, locked: 0n, deposits: 0, withdrawals: 0 }));
    return s;
  }

  get finalized(): number {
    return Math.max(0, this.height - this.K);
  }

  /** Root over the withdrawal log up to and including `height` (opaque, deterministic). */
  withdrawRootAt(height: number): Buffer {
    return sha256(Buffer.from("wroot"), ...this.withdrawals.filter((w) => w.height <= height).map((w) => w.entry));
  }

  mine(n = 1): void {
    for (let i = 0; i < n; i++) {
      this.height = this.blocks.length;
      const txs = this.pool;
      this.pool = [];
      for (const tx of txs) {
        try {
          this.apply(tx);
          this.included.set(txHash(tx), this.height);
        } catch (e) {
          this.refused.push(String((e as Error).message));
        }
      }
      const header = Buffer.alloc(256);
      header[0] = 1;
      header.writeBigUInt64LE(BigInt(this.height), 4);
      randomBytes(32).copy(header, 120);
      randomBytes(32).copy(header, 152);
      this.withdrawRootAt(this.height).copy(header, 184);
      this.blocks.push({ hash: Buffer.from(blake3(header)).toString("hex"), header });
    }
    this.height = this.blocks.length - 1;
  }

  private domainCfg(d: number): FDomain {
    const c = this.domains.find((x) => x.domain === d);
    if (!c || c.paused) throw new Error(`domain ${d} not running`);
    return c;
  }

  private checkSigs(sigs: { pubkey: PublicKey; sig: Buffer }[], msg: Buffer): void {
    const seen = new Set<string>();
    for (const s of sigs) {
      if (!this.attesters.some((a) => a.equals(s.pubkey))) throw new Error("signer not in the set");
      if (seen.has(s.pubkey.toBase58())) throw new Error("duplicate signer");
      if (!edOk(s.pubkey, msg, s.sig)) throw new Error("bad signature from set member");
      seen.add(s.pubkey.toBase58());
    }
    if (seen.size < this.threshold) throw new Error("not enough signatures");
  }

  private apply(raw: Buffer): void {
    const tx = decodeTx(raw);
    if (!edOk(tx.from, framed(TX_NS, tx.unsigned), tx.sig)) throw new Error("bad tx signature");
    const from = this.acct(tx.from.toBase58());
    if (tx.nonce !== from.nonce) throw new Error(`bad nonce ${tx.nonce}, want ${from.nonce}`);
    const b = tx.body;
    if (b.kind === KIND.BridgeUnlockV2) {
      if (!this.v2Active) throw new Error("v2 not active");
      const cfg = this.domainCfg(b.domain);
      const key = `${b.domain}:${b.extNonce}`;
      if (this.deposits.has(key)) throw new Error("deposit already processed");
      const side = { domain: b.domain, extChainId: cfg.extChainId, bridgeAddr: Buffer.from(cfg.bridgeAddr.slice(2), "hex") };
      const body = b.jobOrder ? encodePostJobBody(b.jobOrder) : null;
      const msg = body
        ? depositOrderMessageV2(side, b.extNonce, b.recipient.toBuffer(), b.amountExt, sha256(body))
        : depositMessageV2(side, b.extNonce, b.recipient.toBuffer(), b.amountExt);
      this.checkSigs(b.sigs, msg);
      const amount = b.amountExt * UNITS_PER_SPL;
      if (b.jobOrder && b.jobOrder.protocolFee < this.minProtocolFee) throw new Error("order P below min_protocol_fee_effective");
      this.escrow -= amount;
      const r = this.acct(b.recipient.toBase58());
      r.balance += amount;
      if (b.jobOrder) {
        r.balance -= b.jobOrder.workerPay + b.jobOrder.protocolFee;
        const id = orderJobIdV2(b.domain, b.extNonce).toString("hex");
        this.jobs.set(id, { jobId: id, poster: b.recipient.toBase58(), status: "open", deliverer: null, resultHash: null, resultUri: "", closedHeight: 0 });
      }
      this.deposits.set(key, { height: this.height, recipient: b.recipient.toBase58(), amountExt: b.amountExt });
      const s = this.stat(b.domain);
      s.unlocked += amount;
      s.deposits++;
    } else if (b.kind === KIND.BridgeLockV2) {
      this.domainCfg(b.domain);
      if (b.amount === 0n || b.amount % UNITS_PER_SPL !== 0n) throw new Error("amount not a multiple of 1000");
      const s = this.stat(b.domain);
      if (s.locked + b.amount > s.unlocked) throw new Error("domain outstanding too small");
      if (from.balance < b.amount + tx.fee) throw new Error("insufficient balance");
      from.balance -= b.amount + tx.fee;
      this.escrow += b.amount;
      s.locked += b.amount;
      const nonce = BigInt(s.withdrawals++);
      const amountExt = b.amount / UNITS_PER_SPL;
      const entry = encodeWithdrawalV2({ domain: b.domain, nonce, from: tx.from, recipient: b.recipient, amountExt, height: BigInt(this.height) });
      this.withdrawals.push({ domain: b.domain, nonce, entry, recipient: b.recipient, amountExt, height: this.height, from: tx.from.toBase58() });
    } else throw new Error(`kind ${tx.kind} not applied by this fake`);
    from.nonce++;
  }

  /** Test op: a deliverer completes (or expires) a job at the current height. */
  closeJob(jobIdHex: string, how: { deliverer: PublicKey; resultHash: Buffer; uri: string } | "expire"): void {
    const j = this.jobs.get(jobIdHex.replace(/^0x/, ""));
    if (!j) throw new Error("no such job");
    if (how === "expire") j.status = "expired";
    else Object.assign(j, { status: "completed", deliverer: how.deliverer.toBase58(), resultHash: how.resultHash.toString("hex"), resultUri: how.uri });
    j.closedHeight = this.height;
  }

  private wJson(w: FW) {
    const conf = this.height - w.height;
    return {
      version: 2, domain: w.domain, nonce: w.nonce, from: w.from, recipient: w.recipient.toString("hex"),
      recipient_evm: w.recipient.subarray(0, 12).equals(Buffer.alloc(12)) ? `0x${w.recipient.subarray(12).toString("hex")}` : null,
      amount_ext: w.amountExt, amount: w.amountExt * UNITS_PER_SPL, height: w.height, block_hash: this.blocks[w.height].hash,
      confirmations: conf, entry: w.entry.toString("hex"), final: w.height <= this.finalized,
    };
  }

  handle(method: string, p: any): unknown { // eslint-disable-line @typescript-eslint/no-explicit-any
    const dom = (d: number) => this.withdrawals.filter((w) => w.domain === d);
    switch (method) {
      case "getChainInfo":
        return { chain_id: this.chainId, height: this.height, tip: this.blocks[this.height].hash, finalized_height: this.finalized,
          finalized_hash: this.blocks[this.finalized].hash, finality_k: this.K };
      case "getParams":
        return {
          attesters: this.attesters.map((a) => a.toBase58()), attester_threshold: this.threshold, finality_k: this.K, bridge_paused: false,
          min_protocol_fee_effective: this.minProtocolFee, bridge_v2_active: this.v2Active, bridge_v2_height: 1,
          bridge_domains: this.domains.map((d) => ({ domain: d.domain, ext_chain_id: d.extChainId, bridge_addr: d.bridgeAddr, evm: d.bridgeAddr.length === 42, paused: d.paused })),
        };
      case "getBridgeCounters":
        return {
          height: this.height, escrow_balance: this.escrow,
          domains: [...this.stats.entries()].map(([d, s]) => ({ domain: d, unlocked: s.unlocked, locked: s.locked, outstanding: s.unlocked - s.locked,
            deposit_count: s.deposits, withdrawal_count: s.withdrawals })),
        };
      case "getDeposit": {
        const d = this.deposits.get(`${p.domain}:${p.nonce}`);
        if (!d) return { processed: false };
        return { processed: true, height: d.height, recipient: d.recipient, amount_ext: d.amountExt, confirmations: this.height - d.height, final: d.height <= this.finalized };
      }
      case "getBridgeEvents": {
        const all = dom(p.domain);
        const from = p.from_nonce ?? 0;
        return { domain: p.domain, withdrawal_count: all.length, withdrawals: all.filter((w) => w.nonce >= BigInt(from)).slice(0, p.limit ?? 100).map((w) => this.wJson(w)) };
      }
      case "getWithdrawalProof": {
        const w = dom(p.domain).find((x) => x.nonce === BigInt(p.nonce));
        if (!w) return null;
        const at = p.at_height ?? this.finalized;
        if (at > this.finalized || at < w.height) throw new Error("bad at_height");
        return { withdrawal: this.wJson(w), final: true, entry: w.entry.toString("hex"), location: this.withdrawals.indexOf(w),
          proof: Buffer.from(`proof:${w.domain}:${w.nonce}@${at}`).toString("base64"), withdraw_root: this.withdrawRootAt(at).toString("hex"),
          root_height: at, root_block_hash: this.blocks[at].hash };
      }
      case "getHeader": {
        const b = this.blocks[p.height];
        if (!b) return null;
        return { hash: b.hash, header: b.header.toString("hex"), canonical: true, final: p.height <= this.finalized,
          header_fields: { height: p.height, tx_root: b.header.subarray(120, 152).toString("hex"), state_root: b.header.subarray(152, 184).toString("hex"),
            withdraw_root: b.header.subarray(184, 216).toString("hex") } };
      }
      case "getAccount": {
        const a = this.acct(p.account);
        return { balance: a.balance, nonce: a.nonce };
      }
      case "getJob": {
        const j = this.jobs.get(p.job_id);
        return j ? { job_id: j.jobId, poster: j.poster, status: j.status, deliverer: j.deliverer, result_hash: j.resultHash, result_uri: j.resultUri, closed_height: j.closedHeight } : null;
      }
      case "getTx": {
        const h = this.included.get(p.tx_hash);
        if (h !== undefined) return { status: "included", height: h, confirmations: this.height - h };
        return this.pool.some((t) => txHash(t) === p.tx_hash) ? { status: "pending" } : null;
      }
      case "sendTx": {
        const tx = Buffer.from(p.tx, "base64");
        this.pool.push(tx);
        return { tx_hash: txHash(tx) };
      }
      default:
        throw new Error(`fake node: ${method} not implemented`);
    }
  }
}
