// The RH side of the relayer (docs/RH-TESTNET-PLAN.md 4.3, docs/BRIDGE-V2.md, token-evm/FORMATS.md).
// Same shape as the Solana relayer (src/relayer.ts), one process per side. One `tick()`:
//   1. snapshot: the Cadre chain (tip, bridge v2 params, per-domain counters, domain-2
//      withdrawals, deposit statuses) under one chain tip, and Robinhood Chain (CadreBridge,
//      vault balance, deposits, processed bits) at ONE block, the finality block (RH_FINALITY,
//      `safe` on testnet). The chain tip is re-read at the end and the snapshot retried if it moved;
//   2. checks the domain: bridge v2 active, the chain's domain config names this bridge and
//      this chain id, not paused; otherwise signs nothing;
//   3. per-side invariant (chain units): vault x 1000 == outstanding_d + 1000 x (pending
//      deposits + pending withdrawals), outstanding_d from getBridgeCounters. A deficit
//      always refuses; a surplus (a donation) is accepted unless allowSurplus is false;
//   4. RH -> chain: BridgeUnlockV2 (ed25519 attesters over cadre:deposit:v2 / deposit-order:v2)
//      for each final RH deposit the chain has not processed. An order deposit (refundTo set,
//      made by the storefront) carries its PostJob body from the OrderBody event, checked
//      against the storefront's Order (body hash, tokens, recipient, job id, W + P);
//   5. chain -> RH: for each v2 withdrawal of this domain that is final (>= K deep) and not
//      processed on RH: CadreBridge.release with EIP-712 Release signatures (signer mode) or
//      releaseProof against the header oracle's latest final header (proof mode);
//   6. orders: storefront finalizeOrder / expireOrder (EIP-712 Result / Expired) once the
//      order's chain job is Completed / Expired >= K deep, then settleShare;
//   7. refunds: an order deposit held back (P below min_protocol_fee_effective, or the Order
//      no longer Pending) for refundGraceMs is refunded: bridge refundDeposit (EIP-712
//      Refund), storefront markRefunded, settleShare. As on Solana, the choice is persisted
//      before anything is signed and a nonce is never both unlocked and refunded by this process.

import { createHash } from "node:crypto";
import { Keypair, PublicKey } from "@solana/web3.js";
import { encodeFunctionData, getAddress, isAddressEqual } from "viem";
import type { Address, Hex } from "viem";
import { TOTAL_SUPPLY, UNITS_PER_SPL, depositMessageV2, depositOrderMessageV2, ed25519Sign, orderJobIdV2 } from "../codec.ts";
import type { BridgeSide } from "../codec.ts";
import { KIND, bridgeUnlockV2Body, signTx } from "../agent-tx.ts";
import type { ChainInfo, Job, NodeParams } from "../node.ts";
import { bridgeAbi, MODE_PROOF, MODE_SIGNERS, ORDER, storefrontAbi } from "./abi.ts";
import type { BridgeView, EvmDeposit, EvmIO } from "./io.ts";
import type { BridgeV2Params, DepositV2Status, DomainCounters, NodeV2, WithdrawalV2Ev } from "./node-v2.ts";
import { expiredDigest, pickSigners, refundDigest, releaseDigest, resultDigest, signBundle } from "./sign.ts";
import type { EvmKey } from "./sign.ts";
import { loadEvmState, saveEvmState } from "./state.ts";
import type { EvmHeld, EvmState } from "./state.ts";

export interface EvmRelayerOptions {
  node: NodeV2;
  evm: EvmIO;
  domain: number;
  bridge: Address;
  storefront: Address | null;
  /** ed25519 keys; those in the chain's attester set sign BridgeUnlockV2. */
  chainSigners: Keypair[];
  /** Chain account that sends BridgeUnlockV2 (fee 0). */
  agentKey: Keypair;
  /** secp256k1 keys for EIP-712 (bridge signers, storefront attesters). */
  attesters: EvmKey[];
  stateFile: string;
  expectedChainId?: bigint;
  releaseMode?: "auto" | "signers" | "proof";
  totalSupply?: bigint;
  allowSurplus?: boolean;
  maxPerTick?: number;
  staleMs?: number;
  refundGraceMs?: number;
  dryRun?: boolean;
  log?: (msg: string, extra?: Record<string, unknown>) => void;
  now?: () => number;
}

export interface EvmInvariant {
  ok: boolean;
  vaultExt: string;
  outstanding: string;
  pendingDepositExt: string;
  pendingWithdrawalExt: string;
  /** vault x 1000 - outstanding - 1000 x pending; 0 when healthy. */
  delta: string;
  strictFormHolds: boolean;
}

export interface EvmTick {
  at: string;
  side: "rh";
  domain: number;
  chainHeight: number | null;
  finalityK: number | null;
  rhFinalBlock: string | null;
  invariant: EvmInvariant | null;
  pendingDeposits: string[];
  pendingWithdrawals: string[];
  waitingDepth: string[];
  /** Final withdrawals waiting for the header oracle to cover their block (proof mode). */
  waitingOracle: string[];
  unlocked: string[];
  released: string[];
  releasedViaProof: string[];
  finalized: string[];
  expired: string[];
  needsAttester: string[];
  needsRefund: string[];
  refunded: string[];
  refused: string | null;
  errors: string[];
}

interface Snapshot {
  info: ChainInfo;
  params: NodeParams;
  v2: BridgeV2Params;
  counters: DomainCounters | null;
  withdrawals: WithdrawalV2Ev[];
  withdrawalCount: bigint;
  final: bigint;
  bridge: BridgeView;
  deposits: EvmDeposit[];
  status: Map<bigint, DepositV2Status>;
  /** Per withdrawal (same order): processed on RH at the finality block. */
  processed: boolean[];
  side: BridgeSide;
  chainId: bigint;
}

const errText = (e: unknown): string => {
  const m = String((e as { shortMessage?: string; message?: string })?.shortMessage ?? (e as Error)?.message ?? e);
  return m.slice(0, 500);
};
const sha256 = (b: Buffer): Buffer => createHash("sha256").update(b).digest();
const hexBuf = (h: string): Buffer => Buffer.from(h.replace(/^0x/, ""), "hex");
const toHex = (b: Buffer): Hex => `0x${b.toString("hex")}`;
const ZERO_ADDR = "0x0000000000000000000000000000000000000000";

/** W and P (chain units) from a PostJob body (FORMATS.md 4). */
export function bodyFees(body: Buffer): { workerPay: bigint; protocolFee: bigint } {
  const uriLen = body.readUInt16LE(128);
  const at = 130 + uriLen;
  if (body.length < at + 16) throw new Error("PostJob body too short");
  return { workerPay: body.readBigUInt64LE(at), protocolFee: body.readBigUInt64LE(at + 8) };
}

export class EvmRelayer {
  readonly o: Required<Omit<EvmRelayerOptions, "log" | "now" | "expectedChainId">> & {
    log: NonNullable<EvmRelayerOptions["log"]>;
    now: NonNullable<EvmRelayerOptions["now"]>;
    expectedChainId: bigint | null;
  };
  state: EvmState;
  last: EvmTick | null = null;
  lastOkAt: number | null = null;
  ticks = 0;
  readonly failures = new Map<string, { attempts: number; lastError: string }>();

  constructor(opts: EvmRelayerOptions) {
    this.o = {
      releaseMode: "auto",
      totalSupply: TOTAL_SUPPLY,
      allowSurplus: true,
      maxPerTick: 25,
      staleMs: 120_000,
      refundGraceMs: 3_600_000,
      dryRun: false,
      log: (m, e) => console.log(JSON.stringify({ t: new Date().toISOString(), side: "rh", msg: m, ...e })),
      now: () => Date.now(),
      ...opts,
      expectedChainId: opts.expectedChainId ?? null,
    } as EvmRelayer["o"];
    this.state = loadEvmState(this.o.stateFile);
  }

  async tick(): Promise<EvmTick> {
    const res: EvmTick = {
      at: new Date(this.o.now()).toISOString(), side: "rh", domain: this.o.domain, chainHeight: null, finalityK: null, rhFinalBlock: null,
      invariant: null, pendingDeposits: [], pendingWithdrawals: [], waitingDepth: [], waitingOracle: [], unlocked: [], released: [],
      releasedViaProof: [], finalized: [], expired: [], needsAttester: [], needsRefund: [], refunded: [], refused: null, errors: [],
    };
    this.ticks++;
    try {
      await this.run(res);
    } catch (e) {
      res.errors.push(errText(e));
      res.refused ??= "snapshot failed";
    }
    this.last = res;
    if (!res.refused && res.errors.length === 0) this.lastOkAt = this.o.now();
    return res;
  }

  // ------------------------------------------------------------------ snapshot

  private async snapshot(): Promise<Snapshot> {
    const { node, evm, domain } = this.o;
    const chainId = await evm.chainId();
    if (this.o.expectedChainId !== null && chainId !== this.o.expectedChainId) {
      throw new Error(`RH RPC answers chain id ${chainId}, expected ${this.o.expectedChainId}`);
    }
    // RH reads are pinned to one block number, so they are taken once; only the chain reads
    // (a tip that may move under us) sit inside the retry window, kept as short as possible.
    const { final } = await evm.blocks();
    const bridge = await evm.bridge(final);
    const deposits: EvmDeposit[] = [];
    for (let n = this.state.deposit; n < bridge.depositNonce; n++) deposits.push(await evm.deposit(n, final));
    for (let attempt = 0; attempt < 5; attempt++) {
      const info = await node.getChainInfo();
      const [params, v2, ctr] = await Promise.all([node.getParams(), node.bridgeV2Params(), node.bridgeCounters(domain)]);
      const withdrawals: WithdrawalV2Ev[] = [];
      let withdrawalCount = 0n;
      for (let from = this.state.withdrawal; ; ) {
        const ev = await node.bridgeEventsV2(domain, from, 100);
        withdrawalCount = ev.withdrawalCount;
        withdrawals.push(...ev.withdrawals.filter((w) => w.nonce >= from));
        if (ev.withdrawals.length === 0) break;
        from = ev.withdrawals[ev.withdrawals.length - 1].nonce + 1n;
        if (from >= withdrawalCount) break;
      }
      const sts = await Promise.all(deposits.map((d) => node.depositV2(domain, d.nonce)));
      const after = await node.getChainInfo();
      if (after.tip === info.tip && ctr.height === info.height) {
        const status = new Map<bigint, DepositV2Status>(deposits.map((d, i) => [d.nonce, sts[i]]));
        const processed: boolean[] = [];
        for (const w of withdrawals) processed.push(await evm.isProcessed(w.nonce, final));
        return {
          info, params, v2, counters: ctr.counters, withdrawals, withdrawalCount, final, bridge, deposits, status, processed, chainId,
          side: { domain, extChainId: chainId, bridgeAddr: hexBuf(this.o.bridge) },
        };
      }
    }
    throw new Error("agent chain tip moved during five snapshot attempts");
  }

  /** Why this relayer must not sign for the domain right now, or null. */
  private domainProblem(s: Snapshot): string | null {
    const { domain, bridge } = this.o;
    if (!s.v2.active) return "bridge v2 not active on the chain (getParams.bridge_v2_active false)";
    const d = s.v2.domains.find((x) => x.domain === domain);
    if (!d) return `domain ${domain} not configured on the chain (AdminConfig key 51)`;
    if (d.paused) return `domain ${domain} paused on the chain`;
    if (!d.evm) return `domain ${domain} is not an EVM side on the chain`;
    if (d.extChainId !== s.chainId) return `chain's domain ${domain} names chain id ${d.extChainId}, RH RPC is ${s.chainId}`;
    if (d.bridgeAddr !== bridge.toLowerCase()) return `chain's domain ${domain} names bridge ${d.bridgeAddr}, configured ${bridge}`;
    if (s.bridge.domain !== domain) return `CadreBridge.domain() is ${s.bridge.domain}, configured ${domain}`;
    return null;
  }

  private async run(res: EvmTick): Promise<void> {
    const s = await this.snapshot();
    const K = s.info.finalityK || s.params.finalityK;
    res.chainHeight = s.info.height;
    res.finalityK = K;
    res.rhFinalBlock = s.final.toString();

    const pendingDeposits = s.deposits.filter((d) => !s.status.get(d.nonce)?.processed && !d.refunded);
    const pendingWithdrawals = s.withdrawals.filter((_, i) => !s.processed[i]);
    res.pendingDeposits = pendingDeposits.map((d) => d.nonce.toString());
    res.pendingWithdrawals = pendingWithdrawals.map((w) => w.nonce.toString());
    for (const d of s.deposits) {
      if (d.refunded && s.status.get(d.nonce)?.processed) {
        res.errors.push(`deposit ${d.nonce} is BOTH refunded on RH and unlocked on the chain: signers signed both`);
      }
      if (d.refundTo !== ZERO_ADDR && !d.refunded) this.trackOrder(d.nonce);
    }

    const inv = this.invariant(s.bridge.vault, s.counters?.outstanding ?? 0n, pendingDeposits, pendingWithdrawals);
    res.invariant = inv;
    if (!inv.ok) {
      res.refused = `supply invariant broken on domain ${this.o.domain} (delta ${inv.delta} chain units): refusing to sign`;
      this.o.log("invariant broken, refusing to sign", { invariant: inv });
      this.advanceCursors(s);
      if (!this.o.dryRun) saveEvmState(this.o.stateFile, this.state);
      return;
    }
    const problem = this.domainProblem(s);
    if (problem) {
      res.refused = problem;
      this.advanceCursors(s);
      if (!this.o.dryRun) saveEvmState(this.o.stateFile, this.state);
      return;
    }

    const hold = await this.checkOrders(s, pendingDeposits, res);
    if (this.o.dryRun) {
      for (const w of pendingWithdrawals) if (!this.isFinal(w, K)) res.waitingDepth.push(w.nonce.toString());
      return;
    }
    if (s.params.bridgePaused) res.errors.push("bridge paused on the agent chain: not submitting unlocks");
    else if (s.bridge.paused) res.errors.push("CadreBridge paused: not unlocking");
    else await this.unlockDeposits(s, pendingDeposits.filter((d) => !hold.has(d.nonce) && !this.state.refunds[d.nonce.toString()]), res);
    await this.refunds(s, pendingDeposits, res);
    await this.releaseWithdrawals(s, pendingWithdrawals, K, res);
    await this.settleOrders(s, K, res);
    this.advanceCursors(s);
    saveEvmState(this.o.stateFile, this.state);
  }

  private advanceCursors(s: Snapshot): void {
    let d = s.bridge.depositNonce;
    for (const x of s.deposits) {
      const st = s.status.get(x.nonce);
      if (!(st?.processed && st.final) && !x.refunded) {
        d = x.nonce;
        break;
      }
    }
    if (d > this.state.deposit) this.state.deposit = d;
    for (const k of Object.keys(this.state.inflight)) if (BigInt(k) < this.state.deposit) delete this.state.inflight[k];
    for (const k of Object.keys(this.state.signedUnlock)) if (BigInt(k) < this.state.deposit) delete this.state.signedUnlock[k];
    let w = s.withdrawalCount;
    for (let i = 0; i < s.withdrawals.length; i++) {
      if (!s.processed[i]) {
        w = s.withdrawals[i].nonce;
        break;
      }
    }
    if (w > this.state.withdrawal) this.state.withdrawal = w;
    for (const k of Object.keys(this.state.released)) if (BigInt(k) + 1000n < this.state.withdrawal) delete this.state.released[k];
  }

  private isFinal(w: WithdrawalV2Ev, K: number): boolean {
    return w.final && w.confirmations >= K;
  }

  private trackOrder(nonce: bigint): void {
    const k = nonce.toString();
    if (!this.state.orders.includes(k) && !this.state.settled[k]) this.state.orders.push(k);
  }

  // ------------------------------------------------------------------ RH -> chain

  /** The order's PostJob body, checked against the storefront Order. Throws on any mismatch. */
  private async orderBody(d: EvmDeposit, at: bigint): Promise<{ body: Buffer; order: Awaited<ReturnType<EvmIO["order"]>> }> {
    if (!this.o.storefront) throw new Error(`order deposit ${d.nonce}: RH_STOREFRONT not configured`);
    if (!isAddressEqual(d.sender, this.o.storefront)) throw new Error(`order deposit ${d.nonce}: made by ${d.sender}, not the storefront`);
    const order = await this.o.evm.order(d.nonce, at);
    const bh = await this.o.evm.orderBody(d.nonce, d.blockNumber);
    if (!bh) throw new Error(`order deposit ${d.nonce}: no OrderBody event in block ${d.blockNumber}`);
    const body = hexBuf(bh);
    if (!sha256(body).equals(hexBuf(order.bodyHash))) throw new Error(`order ${d.nonce}: sha256(OrderBody) != order.bodyHash`);
    if (order.tokens !== d.amount) throw new Error(`order ${d.nonce}: tokens ${order.tokens} != deposit ${d.amount}`);
    if (order.recipient.toLowerCase() !== d.recipient.toLowerCase()) throw new Error(`order ${d.nonce}: recipient mismatch`);
    if (!hexBuf(order.jobId).equals(orderJobIdV2(this.o.domain, d.nonce))) throw new Error(`order ${d.nonce}: job id mismatch`);
    const f = bodyFees(body);
    if (f.workerPay + f.protocolFee !== d.amount * UNITS_PER_SPL) throw new Error(`order ${d.nonce}: W + P != amount x 1000`);
    if (!isAddressEqual(order.buyer, d.refundTo)) throw new Error(`order ${d.nonce}: buyer != deposit refundTo`);
    return { body, order };
  }

  /** Order deposits that must not be unlocked (state.needsRefund), as on Solana. */
  private async checkOrders(s: Snapshot, pending: EvmDeposit[], res: EvmTick): Promise<Set<bigint>> {
    const hold = new Set<bigint>();
    for (const k of Object.keys(this.state.needsRefund)) {
      if (!pending.some((d) => d.nonce.toString() === k)) delete this.state.needsRefund[k];
    }
    const min = s.params.minProtocolFeeEffective;
    for (const d of pending) {
      if (d.refundTo === ZERO_ADDR) continue;
      const key = d.nonce.toString();
      let reason: EvmHeld["reason"] | null = null;
      let jobId = "";
      let protocolFee = 0n;
      try {
        const { order, body } = await this.orderBody(d, s.final);
        jobId = order.jobId;
        protocolFee = bodyFees(body).protocolFee;
        if (order.status !== ORDER.PENDING) reason = "order_not_pending";
        else if (min !== null && protocolFee < min) reason = "protocol_fee_below_min";
      } catch (e) {
        // A mismatch is a bug, not a refund case: nothing is signed for it (unlockDeposits reports it).
        if (/order_not_pending/.test(String(e))) reason = "order_not_pending";
        else continue;
      }
      if (!reason && this.state.refunds[key]) reason = this.state.refunds[key].reason;
      if (!reason) {
        if (this.state.needsRefund[key]) {
          this.o.log("order unlockable again", { nonce: key });
          delete this.state.needsRefund[key];
        }
        continue;
      }
      hold.add(d.nonce);
      res.needsRefund.push(key);
      const prev = this.state.needsRefund[key];
      if (prev && prev.reason === reason && prev.minProtocolFeeEffective === String(min ?? 0n)) continue;
      this.state.needsRefund[key] = {
        reason, jobId, buyer: d.refundTo, recipient: d.recipient, amountExt: d.amount.toString(), protocolFee: protocolFee.toString(),
        minProtocolFeeEffective: String(min ?? 0n), since: prev?.since ?? this.o.now(),
      };
      this.o.log("order not unlocked: needs refund", { nonce: key, ...this.state.needsRefund[key] });
    }
    return hold;
  }

  private chainKeys(s: Snapshot): Keypair[] | null {
    const set = s.params.attesters.map((a) => new PublicKey(a));
    const mine = this.o.chainSigners.filter((k) => set.some((p) => p.equals(k.publicKey)));
    const t = s.params.attesterThreshold;
    return mine.length >= t && mine.length > 0 ? mine.slice(0, Math.max(t, 1)) : null;
  }

  private async unlockDeposits(s: Snapshot, pending: EvmDeposit[], res: EvmTick): Promise<void> {
    if (pending.length === 0) return;
    const { node } = this.o;
    const keys = this.chainKeys(s);
    if (!keys) {
      res.errors.push(`hold fewer than ${s.params.attesterThreshold} agent-chain attester keys; cannot unlock`);
      return;
    }
    const acct = await node.getAccount(this.o.agentKey.publicKey.toBase58());
    const now = this.o.now();
    let resync = false;
    const live: bigint[] = [];
    for (const d of pending) {
      const f = this.state.inflight[d.nonce.toString()];
      if (!f) continue;
      const st = await node.getTx(f.txHash);
      if (st && st.status === "pending" && BigInt(f.txNonce) >= acct.nonce && now - f.at < this.o.staleMs) live.push(BigInt(f.txNonce));
      else if (!st || st.status === "pending") resync = true;
      else delete this.state.inflight[d.nonce.toString()];
    }
    if (resync) {
      for (const d of pending) delete this.state.inflight[d.nonce.toString()];
      live.length = 0;
    }
    let next = live.reduce((m, n) => (n + 1n > m ? n + 1n : m), acct.nonce);
    let sent = 0;
    for (const d of pending) {
      if (sent >= this.o.maxPerTick) break;
      const k = d.nonce.toString();
      if (this.state.inflight[k] || this.state.refunds[k]) continue;
      const id = `deposit:${k}`;
      try {
        const recipient = hexBuf(d.recipient);
        let msg: Buffer;
        let jobOrder: Buffer | null = null;
        if (d.refundTo !== ZERO_ADDR) {
          const { body } = await this.orderBody(d, s.final);
          jobOrder = body;
          msg = depositOrderMessageV2(s.side, d.nonce, recipient, d.amount, sha256(body));
        } else {
          msg = depositMessageV2(s.side, d.nonce, recipient, d.amount);
        }
        const sigs = keys.map((kp) => ({ pubkey: kp.publicKey, sig: ed25519Sign(kp, msg) }));
        const body = bridgeUnlockV2Body({ domain: this.o.domain, extNonce: d.nonce, recipient: new PublicKey(recipient), amountExt: d.amount, jobOrder, sigs });
        const tx = signTx(this.o.agentKey, { chainId: s.info.chainId, from: this.o.agentKey.publicKey, nonce: next, fee: 0n, kind: KIND.BridgeUnlockV2 }, body);
        this.state.signedUnlock[k] ??= now;
        saveEvmState(this.o.stateFile, this.state);
        const txHash = await node.sendTx(tx);
        this.state.inflight[k] = { txHash, txNonce: next.toString(), at: now };
        next++;
        sent++;
        res.unlocked.push(k);
        this.failures.delete(id);
        this.o.log("unlock submitted", { nonce: k, recipient: d.recipient, amountExt: d.amount.toString(), order: jobOrder !== null, txHash });
      } catch (e) {
        this.fail(id, e, res);
      }
    }
  }

  // ------------------------------------------------------------------ chain -> RH

  private releasePath(b: BridgeView): { mode: "signers"; keys: EvmKey[] } | { mode: "proof" } | { mode: null; why: string } {
    const keys = (b.releaseModes & MODE_SIGNERS) !== 0 ? pickSigners(this.o.attesters, b.signers, b.threshold) : null;
    const proofOn = (b.releaseModes & MODE_PROOF) !== 0 && b.headers !== ZERO_ADDR;
    const want = this.o.releaseMode;
    if ((want === "signers" || want === "auto") && keys) return { mode: "signers", keys };
    if ((want === "proof" || want === "auto") && proofOn) return { mode: "proof" };
    if (want === "signers") return { mode: null, why: (b.releaseModes & MODE_SIGNERS) === 0 ? "bridge signer path off" : `hold fewer than ${b.threshold} bridge signer keys` };
    if (want === "proof") return { mode: null, why: "bridge proof path off" };
    return { mode: null, why: `no release path: signer keys < ${b.threshold} and proof mode off` };
  }

  private async releaseWithdrawals(s: Snapshot, pending: WithdrawalV2Ev[], K: number, res: EvmTick): Promise<void> {
    res.waitingDepth = pending.filter((w) => !this.isFinal(w, K)).map((w) => w.nonce.toString());
    const ready = pending.filter((w) => this.isFinal(w, K));
    if (ready.length === 0) return;
    // Admin settings (modes, signer set, pause) as they are NOW: a release tx runs against
    // the latest state, while the invariant reads stay at the finality block.
    const now = await this.o.evm.bridge("latest");
    if (now.paused) {
      res.errors.push("CadreBridge paused: not releasing");
      return;
    }
    const path = this.releasePath(now);
    if (path.mode === null) {
      res.errors.push(`cannot release ${ready.map((w) => w.nonce).join(",")}: ${path.why}`);
      return;
    }
    const { evm, node, domain } = this.o;
    let oracle: Awaited<ReturnType<EvmIO["oracle"]>> | null = null;
    let sent = 0;
    for (const w0 of ready) {
      if (sent >= this.o.maxPerTick) break;
      const k = w0.nonce.toString();
      const id = `withdrawal:${k}`;
      try {
        if (await evm.isProcessed(w0.nonce, "latest")) continue; // released, not yet at the finality block
        const ev = await node.bridgeEventsV2(domain, w0.nonce, 1);
        const w = ev.withdrawals.find((x) => x.nonce === w0.nonce);
        if (!w) throw new Error("withdrawal vanished from the canonical chain (reorg); not releasing");
        if (w.entry !== w0.entry || w.blockHash !== w0.blockHash) throw new Error("withdrawal changed since the snapshot (reorg); not releasing");
        if (!this.isFinal(w, K)) throw new Error(`withdrawal no longer ${K} deep; not releasing`);
        if (!w.recipientEvm) throw new Error("recipient is not an EVM address");
        const prev = this.state.released[k];
        if (prev && (prev.recipient.toLowerCase() !== w.recipientEvm.toLowerCase() || prev.amountExt !== w.amountExt.toString())) {
          throw new Error(`nonce ${k} was released before with different content: deep reorg, refusing`);
        }
        if (w.nonce < s.bridge.minWithdrawalNonce) throw new Error(`nonce below the bridge's minWithdrawalNonce ${s.bridge.minWithdrawalNonce}`);
        const recipient = getAddress(w.recipientEvm);
        let data: Hex;
        let via: "signers" | "proof";
        if (path.mode === "signers") {
          const sigs = await signBundle(path.keys, releaseDigest(s.chainId, this.o.bridge, w.nonce, recipient, w.amountExt));
          data = encodeFunctionData({ abi: bridgeAbi, functionName: "release", args: [w.nonce, recipient, w.amountExt, sigs] });
          via = "signers";
        } else {
          oracle ??= await evm.oracle("latest");
          if (oracle.latestHash === `0x${"0".repeat(64)}` || oracle.latestHeight < BigInt(w.height)) {
            res.waitingOracle.push(k);
            continue;
          }
          const p = await node.withdrawalProofV2(domain, w.nonce, Number(oracle.latestHeight));
          if (!p || !p.final || !p.proof || p.entry === undefined || p.location === undefined) throw new Error("no final proof from the node");
          if (`0x${p.rootBlockHash}`.toLowerCase() !== oracle.latestHash.toLowerCase()) {
            throw new Error(`node's header at ${oracle.latestHeight} is ${p.rootBlockHash}, the oracle holds ${oracle.latestHash}`);
          }
          const roots = await evm.oracleRoots(oracle.latestHash);
          if (!roots.final || roots.withdrawRoot.toLowerCase() !== `0x${p.withdrawRoot}`.toLowerCase()) {
            throw new Error("oracle withdrawRoot differs from the node's root at that header");
          }
          if (p.entry !== w.entry) throw new Error("proof entry differs from the event entry");
          data = encodeFunctionData({
            abi: bridgeAbi, functionName: "releaseProof",
            args: [toHex(hexBuf(p.entry)), BigInt(p.location), toHex(Buffer.from(p.proof, "base64")), oracle.latestHash],
          });
          via = "proof";
        }
        const r = await evm.send(this.o.bridge, data);
        this.state.released[k] = { recipient, amountExt: w.amountExt.toString(), height: w.height, via, tx: r.hash };
        sent++;
        res.released.push(k);
        if (via === "proof") res.releasedViaProof.push(k);
        this.failures.delete(id);
        this.o.log("released", { nonce: k, recipient, amountExt: w.amountExt.toString(), via, tx: r.hash });
      } catch (e) {
        this.fail(id, e, res);
      }
    }
  }

  // ------------------------------------------------------------------ refunds

  private async refunds(s: Snapshot, pending: EvmDeposit[], res: EvmTick): Promise<void> {
    if (!this.o.storefront) return;
    const { evm } = this.o;
    const now = this.o.now();
    if (this.o.refundGraceMs >= 0) {
      for (const d of pending) {
        const k = d.nonce.toString();
        const rec = this.state.needsRefund[k];
        if (this.state.refunds[k] || !rec || now - rec.since < this.o.refundGraceMs) continue;
        if (s.status.get(d.nonce)?.processed) continue;
        if (this.state.signedUnlock[k] || this.state.inflight[k]) {
          if (!this.failures.has(`refund:${k}`)) {
            this.o.log("not refunding: this relayer signed an unlock for it", { nonce: k });
            this.failures.set(`refund:${k}`, { attempts: 0, lastError: "signed an unlock" });
          }
          continue;
        }
        this.state.refunds[k] = { buyer: d.refundTo, amountExt: d.amount.toString(), reason: rec.reason, at: now };
        saveEvmState(this.o.stateFile, this.state); // the choice is persisted before anything is signed
      }
    }
    const open = Object.entries(this.state.refunds).filter(([, r]) => !r.shareSettled);
    if (open.length === 0) return;
    const bnow = await evm.bridge("latest");
    const bkeys = pickSigners(this.o.attesters, bnow.signers, bnow.threshold);
    for (const [k, r] of open) {
      const nonce = BigInt(k);
      const id = `refund:${k}`;
      try {
        const dep = await evm.deposit(nonce, "latest");
        if (!dep.refunded) {
          const st = await this.o.node.depositV2(this.o.domain, nonce);
          if (st.processed) throw new Error("deposit was unlocked on the chain; never refunding it");
          if (!bkeys) throw new Error(`hold fewer than ${bnow.threshold} bridge signer keys; cannot refund`);
          const sigs = await signBundle(bkeys, refundDigest(s.chainId, this.o.bridge, nonce, getAddress(dep.refundTo), dep.amount));
          const tx = await evm.send(this.o.bridge, encodeFunctionData({ abi: bridgeAbi, functionName: "refundDeposit", args: [nonce, sigs] }));
          r.tx = tx.hash;
          res.refunded.push(k);
          delete this.state.needsRefund[k];
          this.o.log("refunded", { nonce: k, buyer: dep.refundTo, amountExt: dep.amount.toString(), tx: tx.hash });
        }
        r.tx ??= "confirmed-by-record";
        let o = await evm.order(nonce, "latest");
        if (o.status === ORDER.PENDING || o.status === ORDER.EXPIRED) {
          const tx = await evm.send(this.o.storefront, encodeFunctionData({ abi: storefrontAbi, functionName: "markRefunded", args: [nonce] }));
          r.marked = tx.hash;
          o = await evm.order(nonce, "latest");
        }
        if (o.status === ORDER.REFUNDED && !o.shareSettled && o.share > 0n) {
          const tx = await evm.send(this.o.storefront, encodeFunctionData({ abi: storefrontAbi, functionName: "settleShare", args: [nonce] }));
          r.shareSettled = tx.hash;
        } else if (o.status === ORDER.REFUNDED) {
          r.shareSettled = o.shareSettled ? "settled" : "no-share";
        }
        this.state.orders = this.state.orders.filter((x) => x !== k);
        this.failures.delete(id);
      } catch (e) {
        this.fail(id, e, res);
      }
    }
  }

  // ------------------------------------------------------------------ order results

  private async settleOrders(s: Snapshot, K: number, res: EvmTick): Promise<void> {
    if (!this.o.storefront || this.state.orders.length === 0) return;
    const { evm, node } = this.o;
    const sf = await evm.storefront("latest");
    const keys = pickSigners(this.o.attesters, sf.signers, sf.threshold);
    const me = evm.sender;
    const canCall = sf.signers.some((a) => isAddressEqual(a, me)) || isAddressEqual(sf.owner, me);
    const keep: string[] = [];
    let sent = 0;
    for (const k of this.state.orders) {
      const nonce = BigInt(k);
      if (this.state.refunds[k] || this.state.needsRefund[k]) {
        keep.push(k);
        continue;
      }
      const id = `order:${k}`;
      try {
        const o = await evm.order(nonce, "latest");
        if (o.status === ORDER.NONE) {
          keep.push(k);
          continue;
        }
        if (o.status !== ORDER.PENDING) {
          if ((o.status === ORDER.FINALIZED || o.status === ORDER.EXPIRED) && !o.shareSettled && o.share > 0n) {
            const tx = await evm.send(this.o.storefront, encodeFunctionData({ abi: storefrontAbi, functionName: "settleShare", args: [nonce] }));
            this.o.log("order share settled", { nonce: k, tx: tx.hash });
          }
          this.state.settled[k] ??= "closed";
          continue;
        }
        keep.push(k);
        const job: Job | null = await node.getJob(o.jobId.replace(/^0x/, ""));
        if (!job || (job.status !== "completed" && job.status !== "expired")) continue;
        if (s.info.height - job.closedHeight < K) continue;
        if (!keys) {
          res.needsAttester.push(k);
          continue;
        }
        if (!canCall) throw new Error(`RH_SIGNER_KEY ${me} is not a storefront attester or owner; it cannot call finalize/expire`);
        if (sent >= this.o.maxPerTick) continue;
        let data: Hex;
        if (job.status === "completed") {
          if (!job.deliverer || !job.resultHash) throw new Error("completed job without deliverer/result_hash");
          const deliverer = toHex(new PublicKey(job.deliverer).toBuffer());
          const rh = `0x${job.resultHash}` as Hex;
          const sigs = await signBundle(keys, resultDigest(s.chainId, this.o.storefront, o.jobId, deliverer, rh));
          data = encodeFunctionData({ abi: storefrontAbi, functionName: "finalizeOrder", args: [nonce, deliverer, rh, job.resultUri, sigs] });
        } else {
          const sigs = await signBundle(keys, expiredDigest(s.chainId, this.o.storefront, o.jobId));
          data = encodeFunctionData({ abi: storefrontAbi, functionName: "expireOrder", args: [nonce, sigs] });
        }
        const tx = await evm.send(this.o.storefront, data);
        sent++;
        (job.status === "completed" ? res.finalized : res.expired).push(k);
        this.failures.delete(id);
        this.o.log(job.status === "completed" ? "order finalized" : "order expired", { nonce: k, jobId: job.jobId, tx: tx.hash });
      } catch (e) {
        this.fail(id, e, res);
      }
    }
    this.state.orders = keep;
  }

  // ------------------------------------------------------------------ helpers

  invariant(vault: bigint, outstanding: bigint, pd: EvmDeposit[], pw: WithdrawalV2Ev[]): EvmInvariant {
    const k = UNITS_PER_SPL;
    const pendD = pd.reduce((a, d) => a + d.amount, 0n);
    const pendW = pw.reduce((a, w) => a + w.amountExt, 0n);
    const delta = vault * k - outstanding - k * (pendD + pendW);
    return {
      ok: delta === 0n || (delta > 0n && this.o.allowSurplus),
      vaultExt: vault.toString(), outstanding: outstanding.toString(), pendingDepositExt: pendD.toString(), pendingWithdrawalExt: pendW.toString(),
      delta: delta.toString(), strictFormHolds: vault * k === outstanding,
    };
  }

  private fail(id: string, e: unknown, res: EvmTick): void {
    const f = this.failures.get(id) ?? { attempts: 0, lastError: "" };
    f.attempts++;
    f.lastError = errText(e);
    this.failures.set(id, f);
    res.errors.push(`${id}: ${f.lastError}`);
    this.o.log("submit failed", { id, attempts: f.attempts, error: f.lastError });
  }

  health(pollMs: number): Record<string, unknown> {
    const fresh = this.lastOkAt !== null && this.o.now() - this.lastOkAt < Math.max(3 * pollMs, 15_000);
    return {
      ok: fresh && !!this.last && !this.last.refused && this.last.invariant?.ok === true,
      side: "rh",
      domain: this.o.domain,
      ticks: this.ticks,
      lastOkAt: this.lastOkAt ? new Date(this.lastOkAt).toISOString() : null,
      cursors: { deposit: this.state.deposit.toString(), withdrawal: this.state.withdrawal.toString() },
      inflight: this.state.inflight,
      orders: this.state.orders,
      needsRefund: this.state.needsRefund,
      refunds: this.state.refunds,
      last: this.last,
      failures: Object.fromEntries(this.failures),
    };
  }
}
