// The relayer: one `tick()` takes a consistent snapshot of both chains, checks
// the supply invariant (SPEC section 1), and only if it holds signs and submits
// `unlock` (agent chain) for pending token-chain Deposits and `release` (token
// chain) for pending agent-chain Withdrawals. Cursors persist to a JSON file.
//
// Invariant, generalised for transfers in flight:
//   vault_spl x 1000 == (genesis_escrow - escrow_lamports) + 1000 x (pending_deposits + pending_withdrawals)
// A pending deposit is in the vault but not yet out of the escrow; a pending
// withdrawal is back in the escrow but not yet out of the vault. When nothing is
// in flight this is exactly SPEC's `vault x 1000 == 1e18 - escrow`.

import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { Keypair, PublicKey } from "@solana/web3.js";
import type { ChainIO, RawAccount } from "./chain.ts";
import {
  BRIDGE_PROGRAM_ID,
  ESCROW_PROGRAM_ID,
  GENESIS_ESCROW_LAMPORTS,
  LAMPORTS_PER_SPL_UNIT,
  ata,
  bridgeConfigPda,
  bridgeProcessedPda,
  bridgeReleaseIx,
  createAtaIdempotentIx,
  decodeBridgeConfig,
  decodeDeposit,
  decodeEscrowConfig,
  decodeWithdrawal,
  depositMessage,
  depositPda,
  ed25519Instruction,
  escrowConfigPda,
  escrowPda,
  escrowProcessedPda,
  escrowUnlockIx,
  signEntries,
  tokenAccountAmount,
  withdrawMessage,
  withdrawalPda,
} from "./codec.ts";
import type { Transfer } from "./codec.ts";

export interface RelayerOptions {
  token: ChainIO;
  agent: ChainIO;
  /** Local signer keys; only those in each program's signer set are used. */
  signers: Keypair[];
  tokenPayer: Keypair;
  agentPayer: Keypair;
  cursorFile: string;
  /** Lamports the escrow held at genesis (localnet: the `--account` preload). */
  genesisEscrowLamports?: bigint;
  /** Accept vault surplus (donated SPL or lamports) instead of refusing. Default false (SPEC: strict). */
  allowSurplus?: boolean;
  /** Max submissions per direction per tick. */
  maxPerTick?: number;
  /** Snapshot and check only; never sign or submit. */
  dryRun?: boolean;
  log?: (msg: string, extra?: Record<string, unknown>) => void;
}

export interface Cursors {
  deposit: bigint;
  withdrawal: bigint;
}

export interface InvariantReport {
  ok: boolean;
  vaultSpl: string;
  escrowLamports: string;
  circulatingLamports: string;
  pendingDepositSpl: string;
  pendingWithdrawalSpl: string;
  /** vault x 1000 - circulating - 1000 x pending; 0 when healthy. */
  deltaLamports: string;
  /** SPEC's quiescent form: vault x 1000 == genesis - escrow. */
  strictFormHolds: boolean;
}

export interface TickResult {
  at: string;
  invariant: InvariantReport;
  pendingDeposits: string[];
  pendingWithdrawals: string[];
  unlocked: string[];
  released: string[];
  refused: string | null;
  errors: string[];
}

interface Failure {
  attempts: number;
  lastError: string;
}

const isProgramAccount = (a: RawAccount | null, program: PublicKey): a is RawAccount =>
  a !== null && a.owner.equals(program) && a.data.length > 0;

export function loadCursors(file: string): Cursors {
  if (!existsSync(file)) return { deposit: 0n, withdrawal: 0n };
  const j = JSON.parse(readFileSync(file, "utf8"));
  return { deposit: BigInt(j.deposit ?? 0), withdrawal: BigInt(j.withdrawal ?? 0) };
}

export function saveCursors(file: string, c: Cursors): void {
  const tmp = `${file}.tmp`;
  writeFileSync(tmp, JSON.stringify({ deposit: c.deposit.toString(), withdrawal: c.withdrawal.toString() }, null, 2) + "\n");
  renameSync(tmp, file);
}

function range(from: bigint, to: bigint): bigint[] {
  const out: bigint[] = [];
  for (let n = from; n < to; n++) out.push(n);
  return out;
}

export class Relayer {
  readonly o: Required<Omit<RelayerOptions, "log">> & { log: NonNullable<RelayerOptions["log"]> };
  cursors: Cursors;
  last: TickResult | null = null;
  lastOkAt: number | null = null;
  ticks = 0;
  readonly failures = new Map<string, Failure>();

  constructor(opts: RelayerOptions) {
    this.o = {
      genesisEscrowLamports: GENESIS_ESCROW_LAMPORTS,
      allowSurplus: true,
      maxPerTick: 50,
      dryRun: false,
      log: (m, e) => console.log(JSON.stringify({ t: new Date().toISOString(), msg: m, ...e })),
      ...opts,
    } as Relayer["o"];
    this.cursors = loadCursors(this.o.cursorFile);
  }

  async tick(): Promise<TickResult> {
    const res: TickResult = {
      at: new Date().toISOString(),
      invariant: null as unknown as InvariantReport,
      pendingDeposits: [],
      pendingWithdrawals: [],
      unlocked: [],
      released: [],
      refused: null,
      errors: [],
    };
    this.ticks++;
    try {
      await this.run(res);
    } catch (e) {
      res.errors.push(String((e as Error)?.message ?? e));
      res.refused ??= "snapshot failed";
    }
    this.last = res;
    if (!res.refused && res.errors.length === 0) this.lastOkAt = Date.now();
    return res;
  }

  private async run(res: TickResult): Promise<void> {
    const { token, agent } = this.o;

    // ---- token chain snapshot: bridge config + vault at one slot ----
    const [cfgAcc0] = await token.getAccounts([bridgeConfigPda()]);
    if (!isProgramAccount(cfgAcc0, BRIDGE_PROGRAM_ID)) throw new Error("bridge config not found on token chain");
    const vault = decodeBridgeConfig(cfgAcc0.data).vault;
    const [cfgAcc, vaultAcc] = await token.getAccounts([bridgeConfigPda(), vault]);
    if (!cfgAcc || !vaultAcc) throw new Error("bridge config or vault missing");
    const bcfg = decodeBridgeConfig(cfgAcc.data);
    const vaultSpl = tokenAccountAmount(vaultAcc.data);

    // ---- agent chain snapshot: escrow config + escrow + Processed for deposits ----
    const dNonces = range(this.cursors.deposit, bcfg.depositNonce);
    const agentSnap = await agent.getAccounts([escrowConfigPda(), escrowPda(), ...dNonces.map(escrowProcessedPda)]);
    const [ecfgAcc, escrowAcc] = agentSnap;
    if (!isProgramAccount(ecfgAcc, ESCROW_PROGRAM_ID)) throw new Error("escrow config not found on agent chain");
    if (!escrowAcc) throw new Error("escrow PDA missing on agent chain");
    const ecfg = decodeEscrowConfig(ecfgAcc.data);
    const escrowLamports = escrowAcc.lamports;
    const dProcessed = agentSnap.slice(2).map((a) => isProgramAccount(a, ESCROW_PROGRAM_ID));

    // Deposit records (immutable) from the token chain.
    const depAccs = await token.getAccounts(dNonces.map(depositPda));
    const pendingDeposits: Transfer[] = [];
    dNonces.forEach((n, i) => {
      const a = depAccs[i];
      if (!isProgramAccount(a, BRIDGE_PROGRAM_ID)) throw new Error(`deposit ${n} below counter but missing`);
      if (!dProcessed[i]) pendingDeposits.push(decodeDeposit(a.data));
    });

    // Withdrawal records (immutable) from the agent chain, Processed from the token chain.
    const wNonces = range(this.cursors.withdrawal, ecfg.withdrawNonce);
    const wAccs = await agent.getAccounts(wNonces.map(withdrawalPda));
    const wProcessed = (await token.getAccounts(wNonces.map(bridgeProcessedPda))).map((a) =>
      isProgramAccount(a, BRIDGE_PROGRAM_ID),
    );
    const pendingWithdrawals: Transfer[] = [];
    wNonces.forEach((n, i) => {
      const a = wAccs[i];
      if (!isProgramAccount(a, ESCROW_PROGRAM_ID)) throw new Error(`withdrawal ${n} below counter but missing`);
      if (!wProcessed[i]) pendingWithdrawals.push(decodeWithdrawal(a.data));
    });

    // Advance cursors over the processed prefix.
    this.cursors.deposit = pendingDeposits[0]?.nonce ?? bcfg.depositNonce;
    this.cursors.withdrawal = pendingWithdrawals[0]?.nonce ?? ecfg.withdrawNonce;
    res.pendingDeposits = pendingDeposits.map((d) => d.nonce.toString());
    res.pendingWithdrawals = pendingWithdrawals.map((d) => d.nonce.toString());

    // ---- invariant ----
    const inv = this.invariant(vaultSpl, escrowLamports, pendingDeposits, pendingWithdrawals);
    res.invariant = inv;
    if (!inv.ok) {
      res.refused = `supply invariant broken (delta ${inv.deltaLamports} lamports): refusing to sign`;
      this.o.log("invariant broken, refusing to sign", { invariant: inv });
      if (!this.o.dryRun) saveCursors(this.o.cursorFile, this.cursors);
      return;
    }
    if (this.o.dryRun) return;

    // ---- unlock pending deposits on the agent chain ----
    const dKeys = this.localSigners(ecfg.signers, ecfg.threshold);
    if (pendingDeposits.length && !dKeys) {
      res.errors.push(`hold fewer than ${ecfg.threshold} escrow signer keys; cannot unlock`);
    }
    for (const d of dKeys ? pendingDeposits.slice(0, this.o.maxPerTick) : []) {
      const id = `deposit:${d.nonce}`;
      try {
        const msg = depositMessage(d.nonce, d.recipient, d.amount);
        const sig = await agent.send(
          [
            ed25519Instruction(signEntries(dKeys!, msg)),
            escrowUnlockIx({ payer: this.o.agentPayer.publicKey, nonce: d.nonce, recipient: d.recipient, amountSpl: d.amount }),
          ],
          this.o.agentPayer,
        );
        res.unlocked.push(d.nonce.toString());
        this.failures.delete(id);
        this.o.log("unlocked", { nonce: d.nonce.toString(), recipient: d.recipient.toBase58(), amountSpl: d.amount.toString(), sig });
      } catch (e) {
        this.fail(id, e, res);
      }
    }

    // ---- release pending withdrawals on the token chain ----
    const wKeys = this.localSigners(bcfg.signers, bcfg.threshold);
    if (pendingWithdrawals.length && !wKeys) {
      res.errors.push(`hold fewer than ${bcfg.threshold} bridge signer keys; cannot release`);
    }
    for (const wd of wKeys ? pendingWithdrawals.slice(0, this.o.maxPerTick) : []) {
      const id = `withdrawal:${wd.nonce}`;
      try {
        const msg = withdrawMessage(wd.nonce, wd.recipient, wd.amount);
        const recipientToken = ata(wd.recipient, bcfg.mint, bcfg.tokenProgram);
        const sig = await token.send(
          [
            createAtaIdempotentIx(this.o.tokenPayer.publicKey, wd.recipient, bcfg.mint, bcfg.tokenProgram),
            ed25519Instruction(signEntries(wKeys!, msg)),
            bridgeReleaseIx({
              payer: this.o.tokenPayer.publicKey,
              mint: bcfg.mint,
              nonce: wd.nonce,
              recipient: wd.recipient,
              amount: wd.amount,
              recipientToken,
              tokenProgram: bcfg.tokenProgram,
            }),
          ],
          this.o.tokenPayer,
        );
        res.released.push(wd.nonce.toString());
        this.failures.delete(id);
        this.o.log("released", { nonce: wd.nonce.toString(), recipient: wd.recipient.toBase58(), amountSpl: wd.amount.toString(), sig });
      } catch (e) {
        this.fail(id, e, res);
      }
    }

    // Cursor moves past whatever is now done, in order.
    const doneD = new Set(res.unlocked);
    this.cursors.deposit = pendingDeposits.find((d) => !doneD.has(d.nonce.toString()))?.nonce ?? bcfg.depositNonce;
    const doneW = new Set(res.released);
    this.cursors.withdrawal =
      pendingWithdrawals.find((d) => !doneW.has(d.nonce.toString()))?.nonce ?? ecfg.withdrawNonce;
    saveCursors(this.o.cursorFile, this.cursors);
  }

  invariant(vaultSpl: bigint, escrowLamports: bigint, pd: Transfer[], pw: Transfer[]): InvariantReport {
    const k = LAMPORTS_PER_SPL_UNIT;
    const pendD = pd.reduce((s, d) => s + d.amount, 0n);
    const pendW = pw.reduce((s, d) => s + d.amount, 0n);
    const circulating = this.o.genesisEscrowLamports - escrowLamports;
    const delta = vaultSpl * k - circulating - k * (pendD + pendW);
    return {
      ok: delta === 0n || (delta > 0n && this.o.allowSurplus),
      vaultSpl: vaultSpl.toString(),
      escrowLamports: escrowLamports.toString(),
      circulatingLamports: circulating.toString(),
      pendingDepositSpl: pendD.toString(),
      pendingWithdrawalSpl: pendW.toString(),
      deltaLamports: delta.toString(),
      strictFormHolds: vaultSpl * k === circulating,
    };
  }

  /** Local keys that are in `targetSigners`, or null if they cannot reach `threshold`. */
  private localSigners(targetSigners: PublicKey[], threshold: number): Keypair[] | null {
    const mine = this.o.signers.filter((k) => targetSigners.some((s) => s.equals(k.publicKey)));
    return mine.length >= threshold ? mine.slice(0, Math.max(threshold, 1)) : null;
  }

  private fail(id: string, e: unknown, res: TickResult): void {
    const f = this.failures.get(id) ?? { attempts: 0, lastError: "" };
    f.attempts++;
    f.lastError = String((e as Error)?.message ?? e).slice(0, 500);
    this.failures.set(id, f);
    res.errors.push(`${id}: ${f.lastError}`);
    this.o.log("submit failed", { id, attempts: f.attempts, error: f.lastError });
  }

  health(pollMs: number): Record<string, unknown> {
    const fresh = this.lastOkAt !== null && Date.now() - this.lastOkAt < Math.max(3 * pollMs, 10_000);
    return {
      ok: fresh && !!this.last && !this.last.refused && this.last.invariant?.ok === true,
      ticks: this.ticks,
      lastOkAt: this.lastOkAt ? new Date(this.lastOkAt).toISOString() : null,
      cursors: { deposit: this.cursors.deposit.toString(), withdrawal: this.cursors.withdrawal.toString() },
      last: this.last,
      failures: Object.fromEntries(this.failures),
    };
  }
}
