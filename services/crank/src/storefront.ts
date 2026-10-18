// Storefront buyback crank (token/programs/storefront, SPEC 5.1 "Buyback"): a share of
// every storefront order's SOL sits in the storefront's buyback vault PDA; this module
// calls the permissionless crank_buyback (curve) / crank_buyback_amm (PumpSwap) that buys
// the token with it and sends it to the mining reserve or burns it, plus the admin
// set_buyback and the one-time vault setup. Same pump path as the reserve's buyback.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import type { Connection as ConnectionT, Keypair as KeypairT, PublicKey as PublicKeyT, TransactionInstruction } from '@solana/web3.js';
import { bondingCurve, canonicalPool, pda, ata, PUMP, AMM, FEES, WSOL, TOK, T22, ATAP } from './accounts.ts';

const require = createRequire(import.meta.url);
const { PublicKey, SystemProgram, TransactionInstruction: Ix } = require('@solana/web3.js') as typeof import('@solana/web3.js');
type PublicKey = PublicKeyT;
const anchor = require('@coral-xyz/anchor');
const BN = require('bn.js');
const pumpSdk = require('@pump-fun/pump-sdk');
const amm = new (require('@pump-fun/pump-swap-sdk').PumpAmmSdk)();

export const RENT_MIN = 890_880n;
const DEST_NAMES = ['reserve', 'burn'];
const INIT_USER_VOLUME_ACCUMULATOR = Buffer.from([94, 6, 202, 115, 255, 96, 232, 183]);

export class StorefrontKeys {
  readonly program: PublicKey;
  readonly config: PublicKey;
  readonly vault: PublicKey;
  constructor(program: PublicKey) {
    this.program = program;
    this.config = pda(['config'], program);
    this.vault = pda(['buyback'], program);
  }
  reserveAta(reserveProgram: PublicKey, mint: PublicKey) {
    return ata(pda(['creator'], reserveProgram), mint, T22);
  }
  curve(cranker: PublicKey, mint: PublicKey, reserveProgram: PublicKey, buybackFeeRecipient: PublicKey) {
    const bc = bondingCurve(mint);
    return {
      cranker, config: this.config, buybackVault: this.vault, mint, vaultAta: ata(this.vault, mint, T22), vaultWsol: ata(this.vault, WSOL, TOK),
      reserveAta: this.reserveAta(reserveProgram, mint), pumpGlobal: pda(['global'], PUMP), bondingCurve: bc, curveBaseVault: ata(bc, mint, T22),
      curveQuoteVault: ata(bc, WSOL, TOK), userVolumeAccumulator: pda(['user_volume_accumulator', this.vault], PUMP),
      feeConfig: pda(['fee_config', PUMP], FEES), buybackFeeRecipient, wsolMint: WSOL, tokenProgram: TOK, token2022Program: T22,
      associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, pumpEventAuthority: pda(['__event_authority'], PUMP), pumpProgram: PUMP,
    };
  }
  amm(cranker: PublicKey, mint: PublicKey, reserveProgram: PublicKey, poolBaseVault: PublicKey, poolQuoteVault: PublicKey, ammBuybackOwner: PublicKey) {
    return {
      cranker, config: this.config, buybackVault: this.vault, mint, vaultAta: ata(this.vault, mint, T22), vaultWsol: ata(this.vault, WSOL, TOK),
      reserveAta: this.reserveAta(reserveProgram, mint), pool: canonicalPool(mint), poolBaseVault, poolQuoteVault,
      ammGlobalConfig: pda(['global_config'], AMM), ammUserVolumeAccumulator: pda(['user_volume_accumulator', this.vault], AMM),
      ammFeeConfig: pda(['fee_config', AMM], FEES), ammBuybackFeeRecipient: ata(ammBuybackOwner, WSOL, TOK), wsolMint: WSOL,
      tokenProgram: TOK, token2022Program: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId,
      ammEventAuthority: pda(['__event_authority'], AMM), ammProgram: AMM,
    };
  }
}

type Send = (ixs: TransactionInstruction[], extra?: KeypairT[]) => Promise<string>;

export class StorefrontBuyback {
  readonly program: any;
  readonly keys: StorefrontKeys;
  constructor(readonly conn: ConnectionT, readonly signer: KeypairT, idlFile: string, readonly send: Send, readonly minOutBps: bigint, readonly maxSolIn: bigint) {
    const idl = JSON.parse(fs.readFileSync(path.resolve(idlFile), 'utf8'));
    this.program = new anchor.Program(idl, new anchor.AnchorProvider(conn, new anchor.Wallet(signer), { commitment: 'confirmed' }));
    this.keys = new StorefrontKeys(new PublicKey(idl.address));
  }

  async config() {
    const a = await this.conn.getAccountInfo(this.keys.config);
    return a ? this.program.coder.accounts.decode('config', a.data) : null;
  }

  async status() {
    const c = await this.config();
    if (!c) return { initialized: false, config: this.keys.config.toBase58() };
    const vaultLamports = BigInt(await this.conn.getBalance(this.keys.vault));
    const dest = Number(c.buybackDest);
    const s = (v: any) => v.toString();
    return {
      initialized: true, vault: this.keys.vault.toBase58(), vaultLamports: s(vaultLamports),
      availableLamports: s(vaultLamports > RENT_MIN ? vaultLamports - RENT_MIN : 0n), buybackBps: c.buybackBps,
      destination: DEST_NAMES[dest] ?? `unknown(${dest})`,
      destinationAccount: dest === 0 ? this.keys.reserveAta(c.reserveProgram, c.mint).toBase58() : c.mint.toBase58(),
      reserveProgram: c.reserveProgram.toBase58(), cranker: c.buybackCranker.equals(PublicKey.default) ? null : c.buybackCranker.toBase58(),
      maxPerCall: s(c.buybackMaxPerCall), minBuy: s(c.buybackMinBuy), minIntervalSecs: s(c.buybackMinIntervalSecs), lastBuyTs: s(c.bbLastTs),
      accrued: s(c.bbAccrued), spent: s(c.bbSpent), tokens: s(c.bbTokens), toReserve: s(c.bbToReserve), burned: s(c.bbBurned), buys: s(c.bbBuys),
      pendingOrders: s(c.bbPendingOrders), pendingFirstNonce: s(c.bbPendingFirstNonce), pendingLastNonce: s(c.bbPendingLastNonce),
    };
  }

  /** One-time (anyone pays): pump + PumpSwap user volume accumulators for the vault PDA. */
  async setup(): Promise<string[]> {
    const out: string[] = [];
    for (const prog of [PUMP, AMM]) {
      const acc = pda(['user_volume_accumulator', this.keys.vault], prog);
      if (await this.conn.getAccountInfo(acc)) continue;
      out.push(await this.send([new Ix({
        programId: prog,
        keys: [
          { pubkey: this.signer.publicKey, isSigner: true, isWritable: true }, { pubkey: this.keys.vault, isSigner: false, isWritable: false },
          { pubkey: acc, isSigner: false, isWritable: true }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
          { pubkey: pda(['__event_authority'], prog), isSigner: false, isWritable: false }, { pubkey: prog, isSigner: false, isWritable: false },
        ],
        data: INIT_USER_VOLUME_ACCUMULATOR,
      })]));
    }
    return out;
  }

  /** Admin set_buyback; unset flags stay as they are. dest: reserve | burn. */
  async set(f: Record<string, string>) {
    const dest = f.dest === undefined ? null : f.dest === 'burn' ? 1 : f.dest === 'reserve' ? 0 : Number(f.dest);
    const p = {
      buybackBps: f.bps ? Number(f.bps) : null, destination: dest, reserveProgram: f.reserveProgram ? new PublicKey(f.reserveProgram) : null,
      cranker: f.cranker ? new PublicKey(f.cranker === 'none' ? PublicKey.default : f.cranker) : null,
      maxPerCall: f.maxPerCall ? new BN(f.maxPerCall) : null, minBuy: f.minBuy ? new BN(f.minBuy) : null,
      minIntervalSecs: f.minIntervalSecs ? new BN(f.minIntervalSecs) : null,
    };
    const ix = await this.program.methods.setBuyback(p).accountsStrict({ admin: this.signer.publicKey, config: this.keys.config }).instruction();
    return this.send([ix]);
  }

  /** Decoded storefront events of a confirmed tx. */
  async events(sig: string) {
    const tx = await this.conn.getTransaction(sig, { commitment: 'confirmed', maxSupportedTransactionVersion: 0 });
    const out: { name: string; data: any }[] = [];
    for (const l of tx?.meta?.logMessages ?? []) {
      const m = l.match(/^Program data: (.+)$/);
      const e = m && this.program.coder.events.decode(m[1]);
      if (e) out.push(e);
    }
    return out;
  }

  /** One buyback attempt: curve before graduation, PumpSwap after. Returns a JSON-able result. */
  async once(now: number): Promise<Record<string, unknown>> {
    const c = await this.config();
    if (!c) return { ok: false, skipped: 'storefront config not initialized' };
    const last = Number(c.bbLastTs);
    if (last !== 0 && now < last + Number(c.buybackMinIntervalSecs)) return { ok: false, skipped: `wait ${last + Number(c.buybackMinIntervalSecs) - now}s` };
    const vault = BigInt(await this.conn.getBalance(this.keys.vault));
    const cap = BigInt(c.buybackMaxPerCall.toString());
    let spend = vault > RENT_MIN ? vault - RENT_MIN : 0n;
    if (cap > 0n && spend > cap) spend = cap;
    if (spend > this.maxSolIn) spend = this.maxSolIn;
    const minBuy = BigInt(c.buybackMinBuy.toString());
    if (spend < (minBuy > 0n ? minBuy : 1n)) return { ok: false, skipped: `vault has ${spend} lamports to spend (min_buy ${minBuy})` };
    const mint: PublicKey = c.mint;
    const curve = pumpSdk.PUMP_SDK.decodeBondingCurve(await this.conn.getAccountInfo(bondingCurve(mint)));
    let ix: TransactionInstruction;
    let venue: string;
    if (!curve.complete) {
      venue = 'curve';
      const vt = BigInt(curve.virtualTokenReserves.toString());
      const vq = BigInt((curve.virtualQuoteReserves ?? curve.virtualSolReserves).toString());
      const q = (vt * spend) / (vq + spend);
      const g = pumpSdk.PUMP_SDK.decodeGlobal(await this.conn.getAccountInfo(pda(['global'], PUMP)));
      ix = await this.program.methods
        .crankBuyback(new BN(this.maxSolIn.toString()), new BN(((q * (10_000n - this.minOutBps)) / 10_000n).toString()))
        .accountsStrict(this.keys.curve(this.signer.publicKey, mint, c.reserveProgram, g.buybackFeeRecipients[Math.floor(Math.random() * 8)]))
        .instruction();
    } else {
      venue = 'pumpswap';
      const pool = amm.decodePool(await this.conn.getAccountInfo(canonicalPool(mint)));
      const gc = amm.decodeGlobalConfig(await this.conn.getAccountInfo(pda(['global_config'], AMM)));
      const amt = async (k: PublicKey) => {
        const a = await this.conn.getAccountInfo(k);
        return a && a.data.length >= 72 ? a.data.readBigUInt64LE(64) : 0n;
      };
      const vq = BigInt(pool.virtualQuoteReserves?.toString() ?? '0');
      const quoteRes = (await amt(pool.poolQuoteTokenAccount)) - BigInt(pool.protocolFees?.toString() ?? '0') - BigInt(pool.creatorFees?.toString() ?? '0') + (vq > 0n ? vq : 0n);
      const q = ((await amt(pool.poolBaseTokenAccount)) * spend) / (quoteRes + spend);
      ix = await this.program.methods
        .crankBuybackAmm(new BN(this.maxSolIn.toString()), new BN(((q * (10_000n - this.minOutBps)) / 10_000n).toString()))
        .accountsStrict(this.keys.amm(this.signer.publicKey, mint, c.reserveProgram, pool.poolBaseTokenAccount, pool.poolQuoteTokenAccount, gc.buybackFeeRecipients[0]))
        .instruction();
    }
    const sig = await this.send([ix]);
    const ev = (await this.events(sig)).find((e) => e.name.toLowerCase() === 'buybackexecuted');
    const d = ev?.data ?? {};
    const g = (k: string, alt: string) => (d[k] ?? d[alt])?.toString();
    return {
      ok: true, venue, sig, buy: g('buy', 'buy'), solIn: g('solIn', 'sol_in'), tokensOut: g('tokensOut', 'tokens_out'), routed: g('routed', 'routed'),
      destination: DEST_NAMES[Number(d.destination)] ?? String(d.destination), destinationAccount: (d.destinationAccount ?? d.destination_account)?.toBase58?.(),
      firstNonce: g('firstNonce', 'first_nonce'), lastNonce: g('lastNonce', 'last_nonce'), orders: g('orders', 'orders'),
      availableBefore: g('availableBefore', 'available_before'), carry: g('carry', 'carry'),
    };
  }
}
