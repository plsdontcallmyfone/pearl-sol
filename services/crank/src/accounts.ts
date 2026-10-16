// Account sets for the reserve program's instructions (pump.fun + PumpSwap
// PDAs). Mirrors the order and names in token/programs/reserve.
// CommonJS builds throughout (anchor and the pump SDKs are CJS; mixing in the
// ESM web3.js build would give two PublicKey classes).
import { createRequire } from 'node:module';
import type { PublicKey as PublicKeyT } from '@solana/web3.js';
const require = createRequire(import.meta.url);
const { PublicKey, SystemProgram } = require('@solana/web3.js') as typeof import('@solana/web3.js');
type PublicKey = PublicKeyT;
const { NATIVE_MINT, TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID, getAssociatedTokenAddressSync } =
  require('@solana/spl-token') as typeof import('@solana/spl-token');

export const PUMP = new PublicKey('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P');
export const AMM = new PublicKey('pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA');
export const FEES = new PublicKey('pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ');
export const MAYHEM = new PublicKey('MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e');
export const WSOL = NATIVE_MINT;
export const TOK = TOKEN_PROGRAM_ID;
export const T22 = TOKEN_2022_PROGRAM_ID;
export const ATAP = ASSOCIATED_TOKEN_PROGRAM_ID;

type Seed = string | Buffer | PublicKey;
export const pda = (seeds: Seed[], prog: PublicKey): PublicKey =>
  PublicKey.findProgramAddressSync(
    seeds.map((s) => (typeof s === 'string' ? Buffer.from(s) : Buffer.isBuffer(s) ? s : s.toBuffer())),
    prog,
  )[0];
export const ata = (owner: PublicKey, mint: PublicKey, tp: PublicKey): PublicKey => getAssociatedTokenAddressSync(mint, owner, true, tp);

export const bondingCurve = (mint: PublicKey) => pda(['bonding-curve', mint], PUMP);
export const canonicalPool = (mint: PublicKey) => {
  const auth = pda(['pool-authority', mint], PUMP);
  const idx = Buffer.alloc(2);
  return pda([Buffer.from('pool'), idx, auth, mint, WSOL], AMM);
};

export class ReserveKeys {
  readonly program: PublicKey;
  readonly config: PublicKey;
  readonly creator: PublicKey;
  constructor(program: PublicKey) {
    this.program = program;
    this.config = pda(['config'], program);
    this.creator = pda(['creator'], program);
  }
  reserveAta(mint: PublicKey) {
    return ata(this.creator, mint, T22);
  }

  launch(admin: PublicKey, mint: PublicKey, buybackFeeRecipient: PublicKey) {
    const bc = bondingCurve(mint);
    const solVault = pda(['sol-vault'], MAYHEM);
    return {
      admin, config: this.config, creator: this.creator, mint, mintAuthority: pda(['mint-authority'], PUMP), bondingCurve: bc,
      curveBaseVault: ata(bc, mint, T22), curveQuoteVault: ata(bc, WSOL, TOK), pumpGlobal: pda(['global'], PUMP),
      reserveAta: this.reserveAta(mint), creatorWsol: ata(this.creator, WSOL, TOK),
      userVolumeAccumulator: pda(['user_volume_accumulator', this.creator], PUMP), feeConfig: pda(['fee_config', PUMP], FEES),
      buybackFeeRecipient, mayhemProgram: MAYHEM, mayhemGlobalParams: pda(['global-params'], MAYHEM), mayhemSolVault: solVault,
      mayhemState: pda(['mayhem-state', mint], MAYHEM), mayhemTokenVault: ata(solVault, mint, T22), wsolMint: WSOL,
      tokenProgram: TOK, token2022Program: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId,
      pumpEventAuthority: pda(['__event_authority'], PUMP), pumpProgram: PUMP,
    };
  }

  curve(cranker: PublicKey, mint: PublicKey, buybackFeeRecipient: PublicKey) {
    const bc = bondingCurve(mint);
    const cv = pda(['creator-vault', this.creator], PUMP);
    return {
      cranker, config: this.config, creator: this.creator, mint, reserveAta: this.reserveAta(mint), creatorWsol: ata(this.creator, WSOL, TOK),
      pumpGlobal: pda(['global'], PUMP), bondingCurve: bc, curveBaseVault: ata(bc, mint, T22), curveQuoteVault: ata(bc, WSOL, TOK),
      creatorVault: cv, creatorVaultWsol: ata(cv, WSOL, TOK), userVolumeAccumulator: pda(['user_volume_accumulator', this.creator], PUMP),
      feeConfig: pda(['fee_config', PUMP], FEES), buybackFeeRecipient, wsolMint: WSOL, tokenProgram: TOK, token2022Program: T22,
      associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, pumpEventAuthority: pda(['__event_authority'], PUMP), pumpProgram: PUMP,
    };
  }

  /** `pool` vault keys come from the decoded Pool; `ammBuybackOwner` is a GlobalConfig.buyback_fee_recipients entry. */
  amm(cranker: PublicKey, mint: PublicKey, poolBaseVault: PublicKey, poolQuoteVault: PublicKey, ammBuybackOwner: PublicKey) {
    const vaultAuth = pda(['creator_vault', this.creator], AMM);
    return {
      cranker, config: this.config, creator: this.creator, mint, reserveAta: this.reserveAta(mint), creatorWsol: ata(this.creator, WSOL, TOK),
      bondingCurve: bondingCurve(mint), pool: canonicalPool(mint), poolBaseVault, poolQuoteVault, ammGlobalConfig: pda(['global_config'], AMM),
      coinCreatorVaultAuthority: vaultAuth, coinCreatorVaultAta: ata(vaultAuth, WSOL, TOK),
      ammUserVolumeAccumulator: pda(['user_volume_accumulator', this.creator], AMM), ammFeeConfig: pda(['fee_config', AMM], FEES),
      ammBuybackFeeRecipient: ata(ammBuybackOwner, WSOL, TOK), wsolMint: WSOL, tokenProgram: TOK, token2022Program: T22,
      associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, ammEventAuthority: pda(['__event_authority'], AMM), ammProgram: AMM,
    };
  }

  fund(authority: PublicKey, mint: PublicKey, bridgeProgram: PublicKey, depositNonce: bigint) {
    const bcfg = pda(['config'], bridgeProgram);
    const n = Buffer.alloc(8);
    n.writeBigUInt64LE(depositNonce);
    return {
      authority, config: this.config, creator: this.creator, mint, reserveAta: this.reserveAta(mint), bridgeConfig: bcfg,
      bridgeVault: ata(bcfg, mint, T22), bridgeDeposit: pda([Buffer.from('deposit'), n], bridgeProgram), token2022Program: T22,
      systemProgram: SystemProgram.programId, bridgeProgram,
    };
  }
}
