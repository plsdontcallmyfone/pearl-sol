// The relayer (SPEC 1, 4.3, 5.1). One `tick()`:
//   1. takes a consistent snapshot: the agent-chain node (tip, escrow, withdrawal
//      log, deposit status) and the Solana bridge (config, vault, Processed PDAs),
//      re-reading the chain tip at the end and retrying if it moved;
//   2. checks the supply invariant and refuses to sign anything on a deficit;
//   3. Solana -> chain: submits a BridgeUnlock (kind 6) for each Solana Deposit
//      the chain has not processed. A Deposit whose sender is the storefront
//      trader PDA is an order: the PostJob body is rebuilt from the Order account
//      and must hash to order.body_hash, else nothing is signed for it;
//   4. chain -> Solana: calls bridge.release for each withdrawal (BridgeLock)
//      that is final (>= K deep), re-checking the event and its depth right
//      before signing. Releases are keyed by withdrawal nonce (the bridge's
//      Processed PDA makes a second release of a nonce impossible);
//   5. order results: when an order's job is Completed (or Expired) on the chain
//      and that is >= K deep, submits storefront finalize_order (expire_order)
//      if the relayer holds enough storefront attester keys; otherwise it reports
//      that an attester must finalize (health `needsAttester`).
//
// Invariant, generalised for transfers in flight (all amounts in chain units):
//   vault_spl x 1000 == (total_supply - escrow) + 1000 x (pending_deposits + pending_withdrawals)
// pending deposit = in the vault, not yet unlocked on the chain (as of the chain
// snapshot); pending withdrawal = locked back into the escrow (canonical chain,
// final or not) and not yet released from the vault. With nothing in flight this
// is SPEC's `vault x 1000 == 1e18 - escrow`. A deficit always refuses; a surplus
// (someone donated SPL to the vault) is accepted unless allowSurplus is false.
//
// BridgeUnlock txs are sent from one relayer account with fee 0. If one is
// dropped or stays pending past staleMs, every pending unlock is re-signed from
// the account nonce (deterministic, so an unchanged tx is byte-identical); a
// double unlock is impossible because the chain keeps a replay set by Solana nonce.
// Run one relayer per agentKey (two processes sharing a key would fight over nonces).
//
// Orders the relayer will not unlock (state.needsRefund, tick `needsRefund`,
// `cli.ts status`): before signing an order BridgeUnlock the relayer compares the
// order's P with getParams.min_protocol_fee_effective. The chain refuses a whole
// order unlock with P below it (docs/RPC.md), so retrying would loop forever;
// instead the deposit is held back, logged once and reported. It is re-checked
// every tick, so if the chain floor is lowered (AdminConfig) the order unlocks
// normally. An order whose Solana Order is no longer Pending (an attester closed
// it) is held back the same way.
// Refunds (owner decision 2026-10-17, SPEC 5.1): a held-back order still held back
// `refundGraceMs` after it was first seen is refunded on Solana: bridge
// `refund_deposit` (bridge signers over `pearl-sol:refund:v1 || nonce || buyer ||
// amount_spl`) returns the deposit's tokens to the Order's buyer and writes the refund
// record Processed["processed", [0], nonce]; storefront `mark_refunded` in the same tx
// flips the Order to Refunded. The chain cannot see Solana, so "never both unlocked and
// refunded" rests on each signer key signing at most one of the two per nonce. This
// relayer enforces it for its keys with persisted state, saved BEFORE signing:
// state.signedUnlock (it never refunds those) and state.refunds (it never unlocks
// those), and it reads the refund records every tick (it never unlocks a refunded
// deposit, whoever refunded it). It refunds only a deposit the chain reports as not
// processed. Refunded deposits leave the vault, so they are not pending: the
// invariant is exact again and the deposit cursor passes them.

import { Keypair, PublicKey } from "@solana/web3.js";
import type { ChainIO, RawAccount } from "./chain.ts";
import {
  BRIDGE_PROGRAM_ID,
  MODE_SIGNERS,
  ORDER_STATUS_EXPIRED,
  ORDER_STATUS_PENDING,
  STOREFRONT_PROGRAM_ID,
  TOTAL_SUPPLY,
  UNITS_PER_SPL,
  ata,
  bridgeConfigPda,
  bridgeProcessedPda,
  bridgeRefundIx,
  bridgeRefundPda,
  bridgeReleaseIx,
  createAtaIdempotentIx,
  decodeBridgeConfig,
  decodeDeposit,
  decodeOrder,
  decodeStorefrontConfig,
  depositMessage,
  depositOrderMessage,
  depositPda,
  ed25519Instruction,
  ed25519Sign,
  encodePostJobBody,
  expiredMessage,
  expireOrderIx,
  finalizeOrderIx,
  markRefundedIx,
  orderJobId,
  orderPda,
  refundMessage,
  resultMessage,
  sha256,
  signEntries,
  storefrontConfigPda,
  tokenAccountAmount,
  traderPda,
  withdrawMessage,
} from "./codec.ts";
import type { BridgeConfig, Deposit } from "./codec.ts";
import { KIND, bridgeUnlockBody, signTx } from "./agent-tx.ts";
import type { ChainInfo, DepositStatus, NodeIO, NodeParams, Withdrawal } from "./node.ts";
import { loadState, saveState } from "./state.ts";
import type { RefundRecord, RelayerState } from "./state.ts";

export interface RelayerOptions {
  /** Solana (token chain). */
  token: ChainIO;
  /** Agent-chain node RPC. */
  node: NodeIO;
  /** Local attester / bridge signer keys; each set uses only its own members. */
  signers: Keypair[];
  /** Pays Solana fees (release, finalize). */
  tokenPayer: Keypair;
  /** Agent-chain account that sends BridgeUnlock txs (fee 0, needs no balance). */
  agentKey: Keypair;
  stateFile: string;
  storefrontProgram?: PublicKey;
  totalSupply?: bigint;
  /** Accept a vault surplus (donation). Default true; a deficit always refuses. */
  allowSurplus?: boolean;
  /** Max submissions per direction per tick. */
  maxPerTick?: number;
  /** A BridgeUnlock still pending after this long is re-signed from the account nonce. */
  staleMs?: number;
  /**
   * A held-back order deposit is refunded on Solana this long after it was first held
   * back (state.needsRefund `since`). Negative: never refund. Default 1 h.
   */
  refundGraceMs?: number;
  /** Snapshot and check only; never sign or submit. */
  dryRun?: boolean;
  log?: (msg: string, extra?: Record<string, unknown>) => void;
  now?: () => number;
}

export interface InvariantReport {
  ok: boolean;
  vaultSpl: string;
  escrow: string;
  /** total_supply - escrow (chain units). */
  outsideEscrow: string;
  pendingDepositSpl: string;
  pendingWithdrawalSpl: string;
  /** vault x 1000 - outside - 1000 x pending; 0 when healthy. */
  delta: string;
  /** SPEC's quiescent form: vault x 1000 == total - escrow. */
  strictFormHolds: boolean;
}

export interface TickResult {
  at: string;
  chainHeight: number | null;
  finalityK: number | null;
  invariant: InvariantReport | null;
  pendingDeposits: string[];
  pendingWithdrawals: string[];
  /** Withdrawals seen but not yet K deep. */
  waitingDepth: string[];
  /** Solana deposit nonces a BridgeUnlock was submitted for this tick. */
  unlocked: string[];
  released: string[];
  finalized: string[];
  expired: string[];
  /** Order nonces whose result is final on the chain but need an attester to finalize. */
  needsAttester: string[];
  /**
   * Final withdrawals / order results this relayer leaves to the proof path because
   * the bridge (release_modes) or storefront (finalize_modes) has the signer path off.
   * services/header-relay `run --settle` submits those by light-client proof.
   */
  proofMode: string[];
  /** Order deposit nonces held back for a refund (state.needsRefund has why). */
  needsRefund: string[];
  /** Deposit nonces refunded on Solana this tick (bridge refund_deposit). */
  refunded: string[];
  refused: string | null;
  errors: string[];
}

interface Failure {
  attempts: number;
  lastError: string;
}


const isProgramAccount = (a: RawAccount | null, program: PublicKey): a is RawAccount =>
  a !== null && a.owner.equals(program) && a.data.length > 0;

const errText = (e: unknown): string => String((e as Error)?.message ?? e).slice(0, 500);

export class Relayer {
  readonly o: Required<Omit<RelayerOptions, "log" | "now">> & {
    log: NonNullable<RelayerOptions["log"]>;
    now: NonNullable<RelayerOptions["now"]>;
  };
  state: RelayerState;
  last: TickResult | null = null;
  lastOkAt: number | null = null;
  ticks = 0;
  readonly failures = new Map<string, Failure>();
  private readonly trader: PublicKey;

  constructor(opts: RelayerOptions) {
    this.o = {
      storefrontProgram: STOREFRONT_PROGRAM_ID,
      totalSupply: TOTAL_SUPPLY,
      allowSurplus: true,
      maxPerTick: 50,
      staleMs: 120_000,
      refundGraceMs: 3_600_000,
      dryRun: false,
      log: (m, e) => console.log(JSON.stringify({ t: new Date().toISOString(), msg: m, ...e })),
      now: () => Date.now(),
      ...opts,
    } as Relayer["o"];
    this.trader = traderPda(this.o.storefrontProgram);
    this.state = loadState(this.o.stateFile);
  }

  async tick(): Promise<TickResult> {
    const res: TickResult = {
      at: new Date(this.o.now()).toISOString(),
      chainHeight: null,
      finalityK: null,
      invariant: null,
      pendingDeposits: [],
      pendingWithdrawals: [],
      waitingDepth: [],
      unlocked: [],
      released: [],
      finalized: [],
      expired: [],
      needsAttester: [],
      needsRefund: [],
      refunded: [],
      proofMode: [],
      refused: null,
      errors: [],
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

  /**
   * Chain reads, then one Solana read (config + vault + Processed PDAs at one
   * slot), then the chain's deposit statuses, all under one chain tip.
   */
  private async snapshot(): Promise<Snapshot> {
    const { node, token } = this.o;
    for (let attempt = 0; attempt < 3; attempt++) {
      const info = await node.getChainInfo();
      const params = await node.getParams();
      const esc = await node.getEscrow();
      const withdrawals: Withdrawal[] = [];
      let withdrawalCount = 0n;
      for (let from = this.state.withdrawal; ; ) {
        const ev = await node.getBridgeEvents(from, 100);
        withdrawalCount = ev.withdrawalCount;
        withdrawals.push(...ev.withdrawals.filter((w) => w.nonce >= from));
        if (ev.withdrawals.length === 0) break;
        from = ev.withdrawals[ev.withdrawals.length - 1].nonce + 1n;
        if (from >= withdrawalCount) break;
      }

      const [cfg0] = await token.getAccounts([bridgeConfigPda()]);
      if (!isProgramAccount(cfg0, BRIDGE_PROGRAM_ID)) throw new Error("bridge config not found on Solana");
      const cfg0d = decodeBridgeConfig(cfg0.data);
      const vault = cfg0d.vault;
      // Refund records of the scanned deposits, read at the same slot as the vault.
      const refundNonces: bigint[] = [];
      for (let n = this.state.deposit; n < cfg0d.depositNonce; n++) refundNonces.push(n);
      const nw = withdrawals.length;
      const accs = await token.getAccounts([bridgeConfigPda(), vault, ...withdrawals.map((w) => bridgeProcessedPda(w.nonce)),
        ...refundNonces.map(bridgeRefundPda)]);
      const [cfgAcc, vaultAcc] = accs;
      if (!isProgramAccount(cfgAcc, BRIDGE_PROGRAM_ID) || !vaultAcc) throw new Error("bridge config or vault missing");
      const bcfg = decodeBridgeConfig(cfgAcc.data);
      const released = accs.slice(2, 2 + nw).map((a) => isProgramAccount(a, BRIDGE_PROGRAM_ID));
      const refunded = new Set<bigint>();
      accs.slice(2 + nw).forEach((a, i) => {
        if (isProgramAccount(a, BRIDGE_PROGRAM_ID)) refunded.add(refundNonces[i]);
      });

      const deposits = new Map<bigint, DepositStatus>();
      for (let n = this.state.deposit; n < bcfg.depositNonce; n++) deposits.set(n, await node.getDeposit(n));

      const after = await node.getChainInfo();
      if (after.tip === info.tip && esc.height === info.height) {
        return {
          info, params, escrow: esc.escrowBalance, withdrawals, withdrawalCount, released, bcfg,
          vaultSpl: tokenAccountAmount(vaultAcc.data), deposits, refunded,
        };
      }
    }
    throw new Error("agent chain tip moved during three snapshot attempts");
  }

  private async run(res: TickResult): Promise<void> {
    const { token } = this.o;
    const s = await this.snapshot();
    const K = s.info.finalityK || s.params.finalityK;
    res.chainHeight = s.info.height;
    res.finalityK = K;

    // Solana Deposit records (immutable) for the scanned range.
    const dNonces = [...s.deposits.keys()];
    const depAccs = await token.getAccounts(dNonces.map(depositPda));
    const deposits = new Map<bigint, Deposit>();
    dNonces.forEach((n, i) => {
      const a = depAccs[i];
      if (!isProgramAccount(a, BRIDGE_PROGRAM_ID)) throw new Error(`deposit ${n} below the counter but missing`);
      deposits.set(n, decodeDeposit(a.data));
    });

    // Pending relative to the chain snapshot height. A deposit refunded on Solana left
    // the vault and is never unlocked: not pending.
    const processedAt = (st: DepositStatus) => st.processed && (st.height ?? 0) <= s.info.height;
    for (const n of s.refunded) {
      if (s.deposits.get(n)?.processed) {
        res.errors.push(`deposit ${n} is BOTH refunded on Solana and unlocked on the chain: signers signed both`);
        this.o.log("deposit both refunded and unlocked", { nonce: n.toString() });
      }
    }
    const pendingDeposits = dNonces
      .filter((n) => !processedAt(s.deposits.get(n)!) && !s.refunded.has(n))
      .map((n) => deposits.get(n)!);
    const pendingWithdrawals = s.withdrawals.filter((_, i) => !s.released[i]);
    res.pendingDeposits = pendingDeposits.map((d) => d.nonce.toString());
    res.pendingWithdrawals = pendingWithdrawals.map((w) => w.nonce.toString());

    // Every order deposit in range is tracked for its result, whoever unlocks it.
    for (const d of deposits.values()) if (d.sender.equals(this.trader)) this.trackOrder(d.nonce);

    // ---- invariant ----
    const inv = this.invariant(s.vaultSpl, s.escrow, pendingDeposits, pendingWithdrawals);
    res.invariant = inv;
    if (!inv.ok) {
      res.refused = `supply invariant broken (delta ${inv.delta} chain units): refusing to sign`;
      this.o.log("invariant broken, refusing to sign", { invariant: inv });
      this.advanceCursors(s, new Set());
      if (!this.o.dryRun) saveState(this.o.stateFile, this.state);
      return;
    }

    const holdBack = await this.checkOrderFees(s, pendingDeposits, res);

    if (this.o.dryRun) {
      for (const w of pendingWithdrawals) if (!this.isFinal(w, K)) res.waitingDepth.push(w.nonce.toString());
      return;
    }

    if (s.params.bridgePaused) {
      res.errors.push("bridge paused on the agent chain: not submitting unlocks");
    } else {
      await this.unlockDeposits(s, pendingDeposits.filter((d) => !holdBack.has(d.nonce) && !this.state.refunds[d.nonce.toString()]), res);
    }
    await this.refundDeposits(s, pendingDeposits, deposits, res);
    const releasedNow = await this.releaseWithdrawals(s, pendingWithdrawals, K, res);
    await this.settleOrders(s, K, res);

    this.advanceCursors(s, releasedNow);
    saveState(this.o.stateFile, this.state);
  }

  private advanceCursors(s: Snapshot, releasedNow: Set<bigint>): void {
    // Deposits: past the prefix that is processed AND final on the chain.
    let d = s.bcfg.depositNonce;
    for (const [n, st] of s.deposits) {
      if (!(st.processed && st.final) && !s.refunded.has(n)) {
        d = n;
        break;
      }
    }
    this.state.deposit = d;
    for (const k of Object.keys(this.state.inflight)) if (BigInt(k) < d) delete this.state.inflight[k];
    // An unlock below the cursor is final on the chain: a refund of it is impossible anyway.
    for (const k of Object.keys(this.state.signedUnlock)) if (BigInt(k) < d && !s.refunded.has(BigInt(k))) delete this.state.signedUnlock[k];

    // Withdrawals: past the prefix released on Solana.
    let w = s.withdrawalCount;
    for (let i = 0; i < s.withdrawals.length; i++) {
      if (!s.released[i] && !releasedNow.has(s.withdrawals[i].nonce)) {
        w = s.withdrawals[i].nonce;
        break;
      }
    }
    if (w > this.state.withdrawal) this.state.withdrawal = w;
    for (const k of Object.keys(this.state.released)) if (BigInt(k) + 1000n < this.state.withdrawal) delete this.state.released[k];
  }

  private isFinal(w: Withdrawal, K: number): boolean {
    return w.final && w.confirmations >= K;
  }

  private trackOrder(nonce: bigint): void {
    const k = nonce.toString();
    if (!this.state.orders.includes(k)) this.state.orders.push(k);
  }

  // ------------------------------------------------------------------ Solana -> chain

  /**
   * Order deposits that must not be unlocked: P below the chain's
   * min_protocol_fee_effective, or the Order no longer Pending on Solana. Records
   * them in state.needsRefund (logged once) and returns their nonces. A record is
   * dropped once the order becomes unlockable again (floor lowered).
   */
  private async checkOrderFees(s: Snapshot, pending: Deposit[], res: TickResult): Promise<Set<bigint>> {
    const hold = new Set<bigint>();
    const orders = pending.filter((d) => d.sender.equals(this.trader));
    for (const k of Object.keys(this.state.needsRefund)) {
      if (!pending.some((d) => d.nonce.toString() === k)) delete this.state.needsRefund[k]; // processed after all
    }
    if (orders.length === 0) return hold;
    const min = s.params.minProtocolFeeEffective;
    const accs = await this.o.token.getAccounts(orders.map((d) => orderPda(d.nonce, this.o.storefrontProgram)));
    orders.forEach((d, i) => {
      const acc = accs[i];
      if (!isProgramAccount(acc, this.o.storefrontProgram)) return; // unlockDeposits reports it
      let o;
      try {
        o = decodeOrder(acc.data);
      } catch {
        return; // unlockDeposits reports it
      }
      const key = d.nonce.toString();
      let reason: RefundRecord["reason"] | null = null;
      if (o.status !== ORDER_STATUS_PENDING) reason = "order_not_pending";
      else if (min !== null && o.body.protocolFee < min) reason = "protocol_fee_below_min";
      if (!reason && this.state.refunds[key]) reason = this.state.refunds[key].reason; // refund chosen: stays held
      if (!reason) {
        if (this.state.needsRefund[key]) {
          this.o.log("order unlockable again", { nonce: key, protocolFee: o.body.protocolFee.toString(), min: String(min) });
          delete this.state.needsRefund[key];
        }
        return;
      }
      hold.add(d.nonce);
      res.needsRefund.push(key);
      const prev = this.state.needsRefund[key];
      if (prev && prev.reason === reason && prev.minProtocolFeeEffective === String(min ?? 0n)) return;
      this.state.needsRefund[key] = {
        reason, jobId: o.jobId.toString("hex"), buyer: o.buyer.toBase58(), recipient: o.recipient.toBase58(),
        amountSpl: d.amount.toString(), protocolFee: o.body.protocolFee.toString(), minProtocolFeeEffective: String(min ?? 0n),
        since: prev?.since ?? this.o.now(),
      };
      this.o.log("order not unlocked: needs refund", { nonce: key, ...this.state.needsRefund[key] });
    });
    return hold;
  }

  private async unlockDeposits(s: Snapshot, pending: Deposit[], res: TickResult): Promise<void> {
    if (pending.length === 0) return;
    const { node } = this.o;
    const attesters = s.params.attesters.map((a) => new PublicKey(a));
    const keys = this.localSigners(attesters, s.params.attesterThreshold);
    if (!keys) {
      res.errors.push(`hold fewer than ${s.params.attesterThreshold} agent-chain attester keys; cannot unlock`);
      return;
    }
    const me = this.o.agentKey.publicKey.toBase58();
    const acct = await node.getAccount(me);

    // Reconcile in-flight unlocks: keep the ones still pending and fresh; if any
    // was dropped or went stale, re-sign everything pending from the account nonce
    // (same nonce + same content = byte-identical tx, so a resend is harmless).
    const now = this.o.now();
    let resync = false;
    const live: bigint[] = [];
    for (const d of pending) {
      const f = this.state.inflight[d.nonce.toString()];
      if (!f) continue;
      const st = await node.getTx(f.txHash);
      if (st && st.status === "pending" && BigInt(f.txNonce) >= acct.nonce && now - f.at < this.o.staleMs) {
        live.push(BigInt(f.txNonce));
      } else if (!st || st.status === "pending") {
        resync = true; // dropped, or stuck behind a gap
      } else {
        // Included, but the snapshot says unprocessed: the block was reorged out.
        delete this.state.inflight[d.nonce.toString()];
      }
    }
    if (resync) {
      for (const d of pending) delete this.state.inflight[d.nonce.toString()];
      live.length = 0;
    }
    let next = live.reduce((m, n) => (n + 1n > m ? n + 1n : m), acct.nonce);

    let sent = 0;
    for (const d of pending) {
      if (sent >= this.o.maxPerTick) break;
      if (this.state.inflight[d.nonce.toString()]) continue;
      if (this.state.refunds[d.nonce.toString()] || s.refunded.has(d.nonce)) continue; // refund chosen: never unlock
      const id = `deposit:${d.nonce}`;
      try {
        const tx = await this.buildUnlock(s, d, keys, next);
        // Persist the choice before the signed tx leaves this process.
        this.state.signedUnlock[d.nonce.toString()] ??= now;
        saveState(this.o.stateFile, this.state);
        const txHash = await node.sendTx(tx);
        this.state.inflight[d.nonce.toString()] = { txHash, txNonce: next.toString(), at: now };
        next++;
        sent++;
        res.unlocked.push(d.nonce.toString());
        this.failures.delete(id);
        this.o.log("unlock submitted", { nonce: d.nonce.toString(), recipient: d.recipient.toBase58(),
          amountSpl: d.amount.toString(), order: d.sender.equals(this.trader), txHash });
      } catch (e) {
        this.fail(id, e, res);
      }
    }
  }

  /**
   * Refunds held-back order deposits on Solana once their grace period is over
   * (see "Refunds" at the top). Each refund tx: create the buyer's ATA (idempotent),
   * the ed25519 precompile with the bridge signers' refund signatures, bridge
   * refund_deposit, and storefront mark_refunded when the Order is Pending or Expired.
   */
  private async refundDeposits(s: Snapshot, pending: Deposit[], deposits: Map<bigint, Deposit>, res: TickResult): Promise<void> {
    const now = this.o.now();
    // Orders refunded (by anyone) but not yet marked Refunded on the storefront.
    const marks: bigint[] = [];
    for (const n of s.refunded) if (deposits.get(n)?.sender.equals(this.trader)) marks.push(n);
    if (marks.length) {
      const accs = await this.o.token.getAccounts(marks.map((n) => orderPda(n, this.o.storefrontProgram)));
      for (let i = 0; i < marks.length; i++) {
        const a = accs[i];
        if (!isProgramAccount(a, this.o.storefrontProgram)) continue;
        const st = decodeOrder(a.data).status;
        if (st !== ORDER_STATUS_PENDING && st !== ORDER_STATUS_EXPIRED) continue;
        try {
          await this.o.token.send([markRefundedIx(marks[i], this.o.storefrontProgram)], this.o.tokenPayer);
          this.o.log("order marked refunded", { nonce: marks[i].toString() });
        } catch (e) {
          this.fail(`mark_refunded:${marks[i]}`, e, res);
        }
      }
    }
    for (const [k, r] of Object.entries(this.state.refunds)) {
      if (!r.sig && s.refunded.has(BigInt(k))) r.sig = "confirmed-by-record";
    }

    if (this.o.refundGraceMs < 0) return;
    const due = pending.filter((d) => {
      const k = d.nonce.toString();
      if (this.state.refunds[k]) return true; // decided earlier: final, retry until it lands
      const rec = this.state.needsRefund[k];
      return !!rec && now - rec.since >= this.o.refundGraceMs;
    });
    if (due.length === 0) return;
    const keys = this.localSigners(s.bcfg.signers, s.bcfg.threshold);
    if (!keys) {
      res.errors.push(`hold fewer than ${s.bcfg.threshold} bridge signer keys; cannot refund ${due.map((d) => d.nonce).join(",")}`);
      return;
    }
    const orderAccs = await this.o.token.getAccounts(due.map((d) => orderPda(d.nonce, this.o.storefrontProgram)));
    let sent = 0;
    for (let i = 0; i < due.length; i++) {
      if (sent >= this.o.maxPerTick) break;
      const d = due[i];
      const k = d.nonce.toString();
      const id = `refund:${k}`;
      const st = s.deposits.get(d.nonce);
      if (st?.processed) continue; // unlocked on the chain after all: never refund
      if (this.state.signedUnlock[k] || this.state.inflight[k]) {
        if (!this.failures.has(id)) {
          this.o.log("not refunding: this relayer signed an unlock for it", { nonce: k });
          this.failures.set(id, { attempts: 0, lastError: "signed an unlock" });
        }
        continue;
      }
      try {
        const oa = orderAccs[i];
        if (!isProgramAccount(oa, this.o.storefrontProgram)) throw new Error("Order account missing");
        const order = decodeOrder(oa.data);
        if (order.depositNonce !== d.nonce || order.tokens !== d.amount) throw new Error("Order does not match the deposit");
        const buyer = order.buyer;
        this.state.refunds[k] ??= { buyer: buyer.toBase58(), amountSpl: d.amount.toString(), reason: this.state.needsRefund[k]?.reason ?? "order_not_pending", at: now };
        saveState(this.o.stateFile, this.state); // the choice is persisted before anything is signed
        const ixs = [
          createAtaIdempotentIx(this.o.tokenPayer.publicKey, buyer, s.bcfg.mint, s.bcfg.tokenProgram),
          ed25519Instruction(signEntries(keys, refundMessage(d.nonce, buyer, d.amount))),
          bridgeRefundIx({ payer: this.o.tokenPayer.publicKey, mint: s.bcfg.mint, nonce: d.nonce, buyer,
            buyerToken: ata(buyer, s.bcfg.mint, s.bcfg.tokenProgram), tokenProgram: s.bcfg.tokenProgram }),
        ];
        if (order.status === ORDER_STATUS_PENDING || order.status === ORDER_STATUS_EXPIRED) ixs.push(markRefundedIx(d.nonce, this.o.storefrontProgram));
        const sig = await this.o.token.send(ixs, this.o.tokenPayer);
        this.state.refunds[k].sig = sig;
        delete this.state.needsRefund[k];
        sent++;
        res.refunded.push(k);
        this.failures.delete(id);
        this.o.log("refunded", { nonce: k, buyer: buyer.toBase58(), amountSpl: d.amount.toString(), sig });
      } catch (e) {
        this.fail(id, e, res);
      }
    }
  }

  /** Signs the attester message (plain or order) and the BridgeUnlock tx. */
  private async buildUnlock(s: Snapshot, d: Deposit, keys: Keypair[], txNonce: bigint): Promise<Buffer> {
    let msg: Buffer;
    let jobOrder: Buffer | null = null;
    if (d.sender.equals(this.trader)) {
      const body = await this.orderBody(d);
      jobOrder = body;
      msg = depositOrderMessage(d.nonce, d.recipient, d.amount, sha256(body));
    } else {
      msg = depositMessage(d.nonce, d.recipient, d.amount);
    }
    const sigs = keys.map((k) => ({ pubkey: k.publicKey, sig: ed25519Sign(k, msg) }));
    const body = bridgeUnlockBody({ solanaNonce: d.nonce, recipient: d.recipient, amountSpl: d.amount, jobOrder, sigs });
    return signTx(this.o.agentKey, {
      chainId: s.info.chainId, from: this.o.agentKey.publicKey, nonce: txNonce, fee: 0n, kind: KIND.BridgeUnlock,
    }, body);
  }

  /**
   * Reads the Order for an order deposit and rebuilds its PostJob body exactly
   * as storefront `post_job_body`. Throws (nothing gets signed) on any mismatch.
   */
  private async orderBody(d: Deposit): Promise<Buffer> {
    const [acc] = await this.o.token.getAccounts([orderPda(d.nonce, this.o.storefrontProgram)]);
    if (!isProgramAccount(acc, this.o.storefrontProgram)) throw new Error(`order deposit ${d.nonce}: Order account missing`);
    const o = decodeOrder(acc.data);
    if (o.depositNonce !== d.nonce) throw new Error(`order ${d.nonce}: deposit_nonce ${o.depositNonce} mismatch`);
    if (!o.recipient.equals(d.recipient)) throw new Error(`order ${d.nonce}: recipient mismatch`);
    if (o.tokens !== d.amount) throw new Error(`order ${d.nonce}: tokens ${o.tokens} != deposit ${d.amount}`);
    if (o.body.workerPay + o.body.protocolFee !== d.amount * UNITS_PER_SPL) throw new Error(`order ${d.nonce}: W + P != amount x 1000`);
    if (!o.jobId.equals(orderJobId(d.nonce))) throw new Error(`order ${d.nonce}: job_id mismatch`);
    const body = encodePostJobBody(o.body);
    if (!sha256(body).equals(o.bodyHash)) throw new Error(`order ${d.nonce}: rebuilt PostJob body does not match order.body_hash`);
    return body;
  }

  // ------------------------------------------------------------------ chain -> Solana

  private async releaseWithdrawals(s: Snapshot, pending: Withdrawal[], K: number, res: TickResult): Promise<Set<bigint>> {
    const done = new Set<bigint>();
    const waiting = pending.filter((w) => !this.isFinal(w, K));
    res.waitingDepth = waiting.map((w) => w.nonce.toString());
    const ready = pending.filter((w) => this.isFinal(w, K));
    if (ready.length === 0) return done;
    if ((s.bcfg.releaseModes & MODE_SIGNERS) === 0) {
      for (const w of ready) res.proofMode.push(`withdrawal:${w.nonce}`);
      return done; // signer path off: released by light-client proof (header-relay --settle)
    }
    const keys = this.localSigners(s.bcfg.signers, s.bcfg.threshold);
    if (!keys) {
      res.errors.push(`hold fewer than ${s.bcfg.threshold} bridge signer keys; cannot release`);
      return done;
    }
    for (const w0 of ready.slice(0, this.o.maxPerTick)) {
      const id = `withdrawal:${w0.nonce}`;
      try {
        // Re-check right before signing: same event, still canonical and K deep.
        const ev = await this.o.node.getBridgeEvents(w0.nonce, 1);
        const w = ev.withdrawals.find((x) => x.nonce === w0.nonce);
        if (!w) throw new Error("withdrawal vanished from the canonical chain (reorg); not releasing");
        if (w.solanaRecipient !== w0.solanaRecipient || w.amountSpl !== w0.amountSpl || w.blockHash !== w0.blockHash) {
          throw new Error("withdrawal changed since the snapshot (reorg); not releasing");
        }
        if (!this.isFinal(w, K)) throw new Error(`withdrawal no longer ${K} deep; not releasing`);
        const prev = this.state.released[w.nonce.toString()];
        if (prev && (prev.recipient !== w.solanaRecipient || prev.amountSpl !== w.amountSpl.toString())) {
          throw new Error(`nonce ${w.nonce} was released before with different content: deep reorg, refusing`);
        }
        const recipient = new PublicKey(w.solanaRecipient);
        const sig = await this.o.token.send(
          [
            createAtaIdempotentIx(this.o.tokenPayer.publicKey, recipient, s.bcfg.mint, s.bcfg.tokenProgram),
            ed25519Instruction(signEntries(keys, withdrawMessage(w.nonce, recipient, w.amountSpl))),
            bridgeReleaseIx({
              payer: this.o.tokenPayer.publicKey, mint: s.bcfg.mint, nonce: w.nonce, recipient, amount: w.amountSpl,
              recipientToken: ata(recipient, s.bcfg.mint, s.bcfg.tokenProgram), tokenProgram: s.bcfg.tokenProgram,
            }),
          ],
          this.o.tokenPayer,
        );
        this.state.released[w.nonce.toString()] = {
          recipient: w.solanaRecipient, amountSpl: w.amountSpl.toString(), height: w.height, blockHash: w.blockHash, sig,
        };
        done.add(w.nonce);
        res.released.push(w.nonce.toString());
        this.failures.delete(id);
        this.o.log("released", { nonce: w.nonce.toString(), recipient: w.solanaRecipient, amountSpl: w.amountSpl.toString(), sig });
      } catch (e) {
        this.fail(id, e, res);
      }
    }
    return done;
  }

  // ------------------------------------------------------------------ order results

  private async settleOrders(s: Snapshot, K: number, res: TickResult): Promise<void> {
    if (this.state.orders.length === 0) return;
    const prog = this.o.storefrontProgram;
    const nonces = this.state.orders.map((n) => BigInt(n));
    const accs = await this.o.token.getAccounts([storefrontConfigPda(prog), ...nonces.map((n) => orderPda(n, prog))]);
    const [cfgAcc] = accs;
    if (!isProgramAccount(cfgAcc, prog)) {
      res.errors.push("storefront config not found on Solana; cannot settle orders");
      return;
    }
    const sf = decodeStorefrontConfig(cfgAcc.data);
    const keys = this.localSigners(sf.attesters, sf.threshold);
    const keep: string[] = [];
    let sent = 0;
    for (let i = 0; i < nonces.length; i++) {
      const nonce = nonces[i];
      const acc = accs[i + 1];
      if (!isProgramAccount(acc, prog)) {
        keep.push(nonce.toString()); // created in the same tx as the deposit; not visible yet
        continue;
      }
      const order = decodeOrder(acc.data);
      if (order.status !== ORDER_STATUS_PENDING) continue; // finalized or expired: done
      keep.push(nonce.toString());
      const id = `order:${nonce}`;
      try {
        const job = await this.o.node.getJob(order.jobId.toString("hex"));
        if (!job || (job.status !== "completed" && job.status !== "expired")) continue;
        if (s.info.height - job.closedHeight < K) continue; // not final yet
        if ((sf.finalizeModes & MODE_SIGNERS) === 0) {
          res.proofMode.push(`order:${nonce}`); // finalized / expired by light-client proof
          continue;
        }
        if (!keys) {
          res.needsAttester.push(nonce.toString());
          if (!this.failures.has(id)) {
            this.o.log("order result final on the chain; a storefront attester must finalize", {
              nonce: nonce.toString(), jobId: job.jobId, status: job.status, deliverer: job.deliverer, resultHash: job.resultHash,
            });
            this.failures.set(id, { attempts: 0, lastError: "needs attester" });
          }
          continue;
        }
        if (sent >= this.o.maxPerTick) continue;
        const caller = keys[0];
        let ixs;
        if (job.status === "completed") {
          if (!job.deliverer || !job.resultHash) throw new Error("completed job without deliverer/result_hash");
          const deliverer = new PublicKey(job.deliverer);
          const rh = Buffer.from(job.resultHash, "hex");
          ixs = [
            ed25519Instruction(signEntries(keys, resultMessage(order.jobId, deliverer, rh))),
            finalizeOrderIx({ caller: caller.publicKey, nonce, deliverer, resultHash: rh, resultUri: job.resultUri, program: prog }),
          ];
        } else {
          ixs = [
            ed25519Instruction(signEntries(keys, expiredMessage(order.jobId))),
            expireOrderIx({ caller: caller.publicKey, nonce, program: prog }),
          ];
        }
        const extra = caller.publicKey.equals(this.o.tokenPayer.publicKey) ? [] : [caller];
        const sig = await this.o.token.send(ixs, this.o.tokenPayer, extra);
        sent++;
        keep.pop();
        (job.status === "completed" ? res.finalized : res.expired).push(nonce.toString());
        this.failures.delete(id);
        this.o.log(job.status === "completed" ? "order finalized" : "order expired", { nonce: nonce.toString(), jobId: job.jobId, sig });
      } catch (e) {
        this.fail(id, e, res);
      }
    }
    this.state.orders = keep;
  }

  // ------------------------------------------------------------------ helpers

  invariant(vaultSpl: bigint, escrow: bigint, pd: Deposit[], pw: Withdrawal[]): InvariantReport {
    const k = UNITS_PER_SPL;
    const pendD = pd.reduce((a, d) => a + d.amount, 0n);
    const pendW = pw.reduce((a, w) => a + w.amountSpl, 0n);
    const outside = this.o.totalSupply - escrow;
    const delta = vaultSpl * k - outside - k * (pendD + pendW);
    return {
      ok: delta === 0n || (delta > 0n && this.o.allowSurplus),
      vaultSpl: vaultSpl.toString(),
      escrow: escrow.toString(),
      outsideEscrow: outside.toString(),
      pendingDepositSpl: pendD.toString(),
      pendingWithdrawalSpl: pendW.toString(),
      delta: delta.toString(),
      strictFormHolds: vaultSpl * k === outside,
    };
  }

  /** Local keys that are in `set`, or null if they cannot reach `threshold`. */
  private localSigners(set: PublicKey[], threshold: number): Keypair[] | null {
    const mine = this.o.signers.filter((k) => set.some((s) => s.equals(k.publicKey)));
    return mine.length >= threshold && mine.length > 0 ? mine.slice(0, Math.max(threshold, 1)) : null;
  }

  private fail(id: string, e: unknown, res: TickResult): void {
    const f = this.failures.get(id) ?? { attempts: 0, lastError: "" };
    f.attempts++;
    f.lastError = errText(e);
    this.failures.set(id, f);
    res.errors.push(`${id}: ${f.lastError}`);
    this.o.log("submit failed", { id, attempts: f.attempts, error: f.lastError });
  }

  health(pollMs: number): Record<string, unknown> {
    const fresh = this.lastOkAt !== null && this.o.now() - this.lastOkAt < Math.max(3 * pollMs, 10_000);
    return {
      ok: fresh && !!this.last && !this.last.refused && this.last.invariant?.ok === true,
      ticks: this.ticks,
      lastOkAt: this.lastOkAt ? new Date(this.lastOkAt).toISOString() : null,
      cursors: { deposit: this.state.deposit.toString(), withdrawal: this.state.withdrawal.toString() },
      inflight: this.state.inflight,
      orders: this.state.orders,
      needsRefund: this.state.needsRefund,
      last: this.last,
      failures: Object.fromEntries(this.failures),
    };
  }
}

interface Snapshot {
  info: ChainInfo;
  params: NodeParams;
  escrow: bigint;
  /** Canonical withdrawals from the cursor on, by nonce. */
  withdrawals: Withdrawal[];
  withdrawalCount: bigint;
  /** Per withdrawal: bridge Processed PDA exists on Solana. */
  released: boolean[];
  bcfg: BridgeConfig;
  vaultSpl: bigint;
  /** Chain status of every Solana deposit from the cursor to the counter. */
  deposits: Map<bigint, DepositStatus>;
  /** Deposits (from the cursor) with a bridge refund record on Solana. */
  refunded: Set<bigint>;
}
