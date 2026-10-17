// A small in-process agent-chain node for tests, speaking the docs/RPC.md
// method shapes (through NodeRpc's swappable transport, JSON round-tripped).
// It applies Transfer, BridgeUnlock (attester sigs checked, order jobs created)
// and BridgeLock (withdrawal log), plus test-only job ops. State is recomputed by
// replaying the canonical blocks, so `reorg()` is exact.
//
// Depth: confirmations = tip height - event height; final = confirmations >= K.

import { createPublicKey, randomBytes, verify as edVerify } from "node:crypto";
import { PublicKey } from "@solana/web3.js";
import { DEFAULT_CHAIN_ID, ESCROW_ADDRESS, KIND, POOL_ADDRESS, TX_NS, decodeTx, framed, txHash } from "../src/agent-tx.ts";
import { TOTAL_SUPPLY, UNITS_PER_SPL, depositMessage, depositOrderMessage, encodePostJobBody, orderJobId, sha256 } from "../src/codec.ts";
import type { Transport } from "../src/node.ts";
import { NodeRpc } from "../src/node.ts";

export type JobOp =
  | { op: "deliver"; jobId: string; deliverer: PublicKey; resultHash: Buffer; uri: string }
  | { op: "complete"; jobId: string }
  | { op: "expire"; jobId: string }
  /** Test-only: move escrow funds outside the bridge (a bug or exploit). */
  | { op: "leak"; amount: bigint; to: PublicKey };

interface Block {
  hash: string;
  txs: Buffer[];
  ops: JobOp[];
}

interface Acct {
  balance: bigint;
  nonce: bigint;
}

interface FJob {
  jobId: string;
  poster: string;
  status: "open" | "delivered" | "completed" | "expired";
  deliverer: string | null;
  resultHash: string | null;
  resultUri: string;
  postedHeight: number;
  closedHeight: number;
  deadlineHeight: number;
  workerPay: bigint;
}

interface FWithdrawal {
  nonce: bigint;
  from: string;
  solanaRecipient: string;
  amountSpl: bigint;
  height: number;
  blockHash: string;
}

interface World {
  accounts: Map<string, Acct>;
  deposits: Map<bigint, { height: number; recipient: string; amountSpl: bigint }>;
  withdrawals: FWithdrawal[];
  jobs: Map<string, FJob>;
  included: Map<string, number>; // tx hash -> height
}

const SPKI = Buffer.from("302a300506032b6570032100", "hex");
const edOk = (pk: PublicKey, msg: Buffer, sig: Buffer): boolean =>
  edVerify(null, msg, createPublicKey({ key: Buffer.concat([SPKI, pk.toBuffer()]), format: "der", type: "spki" }), sig);

export interface FakeNodeOptions {
  chainId?: number;
  finalityK?: number;
  attesters: PublicKey[];
  threshold?: number;
  minFee?: bigint;
  /** getParams.min_protocol_fee_effective; order unlocks with a smaller P are refused. */
  minProtocolFee?: bigint;
  /** Initial balances / nonces at genesis (base58 -> values). */
  genesis?: Record<string, Partial<Acct>>;
}

export class FakeNode {
  readonly chainId: number;
  readonly K: number;
  attesters: PublicKey[];
  threshold: number;
  minFee: bigint;
  minProtocolFee: bigint;
  bridgePaused = false;
  blocks: Block[] = [];
  mempool: Buffer[] = [];
  genesis: Record<string, Partial<Acct>>;
  w!: World;
  /** Per-method call counts, and an optional hook run before each call. */
  calls = new Map<string, number>();
  hook: ((method: string, params: Record<string, unknown>) => void) | null = null;

  constructor(o: FakeNodeOptions) {
    this.chainId = o.chainId ?? DEFAULT_CHAIN_ID;
    this.K = o.finalityK ?? 30;
    this.attesters = o.attesters;
    this.threshold = o.threshold ?? 1;
    this.minFee = o.minFee ?? 1000n;
    this.minProtocolFee = o.minProtocolFee ?? 2n;
    this.genesis = o.genesis ?? {};
    this.replay();
  }

  get height(): number {
    return this.blocks.length;
  }
  get tip(): string {
    return this.blocks.length ? this.blocks[this.blocks.length - 1].hash : "00".repeat(32);
  }

  client(): NodeRpc {
    const t: Transport = async (method, params) => {
      const p = JSON.parse(JSON.stringify(params));
      this.calls.set(method, (this.calls.get(method) ?? 0) + 1);
      this.hook?.(method, p);
      return JSON.parse(JSON.stringify(this.handle(method, p)));
    };
    return new NodeRpc(t);
  }

  // ---------------------------------------------------------------- chain ops

  /** Mines one block from the mempool (valid txs in order; invalid ones are dropped) plus `ops`. */
  mine(ops: JobOp[] = []): Block {
    const hash = randomBytes(32).toString("hex");
    const txs: Buffer[] = [];
    const w = this.w;
    const h = this.height + 1;
    for (const tx of this.mempool) {
      const snap = cloneWorld(w);
      try {
        this.applyTx(w, tx, h, hash);
        txs.push(tx);
      } catch {
        Object.assign(w, snap); // all-or-nothing
      }
    }
    this.mempool = [];
    const b = { hash, txs, ops };
    this.blocks.push(b);
    this.replay();
    return b;
  }

  mineN(n: number): void {
    for (let i = 0; i < n; i++) this.mine();
  }

  /** Drops the last `depth` blocks. Their txs go back to the mempool only if `returnTxs`. */
  reorg(depth: number, returnTxs = false): Block[] {
    const gone = this.blocks.splice(this.blocks.length - depth, depth);
    if (returnTxs) this.mempool.unshift(...gone.flatMap((b) => b.txs));
    this.replay();
    return gone;
  }

  account(k: string | PublicKey): Acct {
    const key = typeof k === "string" ? k : k.toBase58();
    return this.w.accounts.get(key) ?? { balance: 0n, nonce: 0n };
  }
  escrowBalance(): bigint {
    return this.account(ESCROW_ADDRESS).balance;
  }

  private replay(): void {
    const w: World = { accounts: new Map(), deposits: new Map(), withdrawals: [], jobs: new Map(), included: new Map() };
    w.accounts.set(ESCROW_ADDRESS.toBase58(), { balance: TOTAL_SUPPLY, nonce: 0n });
    for (const [k, v] of Object.entries(this.genesis)) {
      const a = { balance: v.balance ?? 0n, nonce: v.nonce ?? 0n };
      // Genesis balances come out of the escrow so the supply stays whole.
      w.accounts.get(ESCROW_ADDRESS.toBase58())!.balance -= a.balance;
      w.accounts.set(k, a);
    }
    this.blocks.forEach((b, i) => {
      const h = i + 1;
      for (const tx of b.txs) this.applyTx(w, tx, h, b.hash);
      for (const op of b.ops) this.applyOp(w, op, h);
    });
    this.w = w;
  }

  private acct(w: World, k: string): Acct {
    let a = w.accounts.get(k);
    if (!a) {
      a = { balance: 0n, nonce: 0n };
      w.accounts.set(k, a);
    }
    return a;
  }

  private move(w: World, from: string, to: string, amount: bigint): void {
    const a = this.acct(w, from);
    if (a.balance < amount) throw new Error("insufficient balance");
    a.balance -= amount;
    this.acct(w, to).balance += amount;
  }

  private checkSigs(sigs: { pubkey: PublicKey; sig: Buffer }[], msg: Buffer): void {
    const seen = new Set<string>();
    for (const s of sigs) {
      if (!this.attesters.some((a) => a.equals(s.pubkey))) throw new Error("signer not in the set");
      if (seen.has(s.pubkey.toBase58())) throw new Error("duplicate signer");
      if (!edOk(s.pubkey, msg, s.sig)) throw new Error("bad signature from set member");
      seen.add(s.pubkey.toBase58());
    }
    if (seen.size < this.threshold) throw new Error(`${seen.size} of ${this.threshold} required signatures`);
  }

  /** Mempool admission checks (sendTx). */
  private admit(raw: Buffer): void {
    const tx = decodeTx(raw);
    if (tx.chainId !== this.chainId) throw new Error("wrong chain_id");
    if (!edOk(tx.from, framed(TX_NS, tx.unsigned), tx.sig)) throw new Error("bad tx signature");
    if (tx.kind !== KIND.BridgeUnlock && tx.fee < this.minFee) throw new Error("fee below min_fee");
    if (tx.body.kind === KIND.BridgeUnlock && tx.body.jobOrder && tx.body.jobOrder.protocolFee < this.minProtocolFee) {
      throw new Error("protocol_fee below min_protocol_fee_effective");
    }
    const a = this.account(tx.from);
    if (tx.nonce < a.nonce || tx.nonce > a.nonce + 64n) throw new Error("nonce out of window");
  }

  private applyTx(w: World, raw: Buffer, h: number, blockHash: string): void {
    const tx = decodeTx(raw);
    if (tx.chainId !== this.chainId) throw new Error("wrong chain_id");
    if (!edOk(tx.from, framed(TX_NS, tx.unsigned), tx.sig)) throw new Error("bad tx signature");
    const from = tx.from.toBase58();
    const a = this.acct(w, from);
    if (a.nonce !== tx.nonce) throw new Error(`bad nonce: expected ${a.nonce}, got ${tx.nonce}`);
    if (a.balance < tx.fee) throw new Error("cannot pay fee");
    a.balance -= tx.fee;
    a.nonce += 1n;
    this.acct(w, "miner").balance += tx.fee;
    const b = tx.body;
    if (b.kind === KIND.Transfer) {
      this.move(w, from, b.to.toBase58(), b.amount);
    } else if (b.kind === KIND.BridgeUnlock) {
      if (this.bridgePaused) throw new Error("bridge paused");
      if (w.deposits.has(b.solanaNonce)) throw new Error("deposit already processed");
      const msg = b.jobOrder
        ? depositOrderMessage(b.solanaNonce, b.recipient, b.amountSpl, sha256(encodePostJobBody(b.jobOrder)))
        : depositMessage(b.solanaNonce, b.recipient, b.amountSpl);
      this.checkSigs(b.sigs, msg);
      const amount = b.amountSpl * UNITS_PER_SPL;
      if (amount === 0n) throw new Error("zero deposit");
      const rcpt = b.recipient.toBase58();
      this.move(w, ESCROW_ADDRESS.toBase58(), rcpt, amount);
      w.deposits.set(b.solanaNonce, { height: h, recipient: rcpt, amountSpl: b.amountSpl });
      if (b.jobOrder) {
        const p = b.jobOrder;
        if (p.workerPay + p.protocolFee > amount) throw new Error("order pay exceeds unlock");
        if (p.protocolFee < this.minProtocolFee) throw new Error("protocol_fee below min_protocol_fee_effective");
        const jobId = orderJobId(b.solanaNonce).toString("hex");
        this.move(w, rcpt, POOL_ADDRESS.toBase58(), p.protocolFee);
        this.move(w, rcpt, `job:${jobId}`, p.workerPay); // W escrowed in the job
        w.jobs.set(jobId, {
          jobId, poster: rcpt, status: "open", deliverer: null, resultHash: null, resultUri: "", postedHeight: h, closedHeight: 0,
          deadlineHeight: h + Number(p.deadline), workerPay: p.workerPay,
        });
      }
    } else if (b.kind === KIND.BridgeLock) {
      if (this.bridgePaused) throw new Error("bridge paused");
      if (b.amount === 0n || b.amount % UNITS_PER_SPL !== 0n) throw new Error("amount must be a positive multiple of 1000");
      this.move(w, from, ESCROW_ADDRESS.toBase58(), b.amount);
      w.withdrawals.push({
        nonce: BigInt(w.withdrawals.length), from, solanaRecipient: b.solanaRecipient.toBase58(), amountSpl: b.amount / UNITS_PER_SPL,
        height: h, blockHash,
      });
    }
    w.included.set(txHash(raw), h);
  }

  private applyOp(w: World, op: JobOp, h: number): void {
    if (op.op === "leak") {
      this.move(w, ESCROW_ADDRESS.toBase58(), op.to.toBase58(), op.amount);
      return;
    }
    const j = w.jobs.get(op.jobId);
    if (!j) throw new Error(`fake node: unknown job ${op.jobId}`);
    if (op.op === "deliver") {
      j.status = "delivered";
      j.deliverer = op.deliverer.toBase58();
      j.resultHash = op.resultHash.toString("hex");
      j.resultUri = op.uri;
    } else if (op.op === "complete") {
      j.status = "completed";
      j.closedHeight = h;
      this.move(w, `job:${j.jobId}`, j.deliverer!, j.workerPay);
    } else {
      j.status = "expired";
      j.closedHeight = h;
      this.move(w, `job:${j.jobId}`, j.poster, j.workerPay);
    }
  }

  // ---------------------------------------------------------------- RPC

  private depth(h: number) {
    const confirmations = this.height - h;
    return { confirmations, final: confirmations >= this.K };
  }

  handle(method: string, p: Record<string, any>): unknown { // eslint-disable-line @typescript-eslint/no-explicit-any
    const w = this.w;
    switch (method) {
      case "getChainInfo":
        return {
          chain_id: this.chainId, genesis_hash: "00".repeat(32), height: this.height, tip: this.tip, cumulative_work: "0",
          finalized_height: Math.max(0, this.height - this.K), finalized_hash: "", target_bits: 0, next_target_bits: 0,
          mempool_size: this.mempool.length, peers: 0, finality_k: this.K, syncing: false,
        };
      case "getParams":
        return {
          admin: "", min_fee: this.minFee.toString(), finality_k: this.K, attesters: this.attesters.map((a) => a.toBase58()),
          attester_threshold: this.threshold, bridge_paused: this.bridgePaused,
          min_protocol_fee: this.minProtocolFee.toString(), min_protocol_fee_effective: this.minProtocolFee.toString(),
        };
      case "getEscrow": {
        const escrow = this.escrowBalance();
        const outside = TOTAL_SUPPLY - escrow;
        return {
          escrow: ESCROW_ADDRESS.toBase58(), escrow_balance: escrow.toString(), pool: POOL_ADDRESS.toBase58(),
          pool_balance: this.account(POOL_ADDRESS).balance.toString(), total_supply: TOTAL_SUPPLY.toString(),
          outside_escrow: outside.toString(), outside_escrow_spl: (outside / UNITS_PER_SPL).toString(), height: this.height,
        };
      }
      case "getBridgeEvents": {
        const from = BigInt(p.from_nonce ?? 0);
        const limit = Math.min(Number(p.limit ?? 100), 100);
        return {
          withdrawal_count: w.withdrawals.length, deposit_count: w.deposits.size,
          withdrawals: w.withdrawals.filter((x) => x.nonce >= from).slice(0, limit).map((x) => ({
            nonce: Number(x.nonce), from: x.from, solana_recipient: x.solanaRecipient, amount_spl: x.amountSpl.toString(),
            amount: (x.amountSpl * UNITS_PER_SPL).toString(), height: x.height, block_hash: x.blockHash, ...this.depth(x.height),
          })),
        };
      }
      case "getDeposit": {
        const d = w.deposits.get(BigInt(p.solana_nonce));
        if (!d) return { processed: false };
        return { processed: true, height: d.height, recipient: d.recipient, amount_spl: d.amountSpl.toString(), ...this.depth(d.height) };
      }
      case "getAccount": {
        const a = this.account(p.account ?? p.address);
        return { account: p.account, balance: a.balance.toString(), nonce: Number(a.nonce) };
      }
      case "getJob": {
        const j = w.jobs.get(p.job_id);
        if (!j) return null;
        return {
          job_id: j.jobId, poster: j.poster, status: j.status, deliverer: j.deliverer, result_hash: j.resultHash, result_uri: j.resultUri,
          posted_height: j.postedHeight, closed_height: j.closedHeight, deadline_height: j.deadlineHeight,
          worker_pay: j.workerPay.toString(),
        };
      }
      case "getTx": {
        const h = w.included.get(p.tx_hash);
        if (h !== undefined) return { tx_hash: p.tx_hash, status: "included", height: h, confirmations: this.height - h };
        if (this.mempool.some((t) => txHash(t) === p.tx_hash)) return { tx_hash: p.tx_hash, status: "pending" };
        return null;
      }
      case "sendTx": {
        const raw = Buffer.from(p.tx, "base64");
        this.admit(raw);
        const hash = txHash(raw);
        if (!this.mempool.some((t) => txHash(t) === hash) && !w.included.has(hash)) this.mempool.push(raw);
        return { tx_hash: hash };
      }
      default:
        throw new Error(`fake node: unknown method ${method}`);
    }
  }
}

function cloneWorld(w: World): World {
  return {
    accounts: new Map([...w.accounts].map(([k, v]) => [k, { ...v }])),
    deposits: new Map(w.deposits),
    withdrawals: [...w.withdrawals],
    jobs: new Map([...w.jobs].map(([k, v]) => [k, { ...v }])),
    included: new Map(w.included),
  };
}
