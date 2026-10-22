// PumpSwap (pump_amm) on Solana mainnet, read-only: pool and token-account decoding, the fee
// tier from the program's FeeConfig account, and constant-product quotes with that fee. These are
// the numbers a practice fill uses. They are computed here from the program's own accounts; they
// are not the program's code, so integer rounding can differ from a real fill by a few base units.
import { b58decode, b58encode, findPda, sha256, utf8 } from "../../../../apps/explorer/src/codec.ts";

export const AMM = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
export const PUMP = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
export const PUMP_FEES = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";
export const WSOL = "So11111111111111111111111111111111111111112";
export const TOKEN = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const TOKEN_2022 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
export const ATA_PROGRAM = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
/** ["fee_config", pump_amm] @ pump_fees (research/NOTES-pump.md: 5PHirr...). */
export const AMM_FEE_CONFIG = findPda([utf8("fee_config"), b58decode(AMM)], PUMP_FEES)[0];

export const disc = (name: string) => Buffer.from(sha256(utf8(`global:${name}`)).subarray(0, 8)).toString("hex");
/** Trade instructions whose account 0 is the pool, 3 the base mint and 4 the quote mint. */
export const TRADE_DISCS = new Set(["buy", "sell", "buy_exact_quote_in"].map(disc));

export function canonicalPool(mint: string): string {
  const authority = findPda([utf8("pool-authority"), b58decode(mint)], PUMP)[0];
  return findPda([utf8("pool"), Uint8Array.from([0, 0]), b58decode(authority), b58decode(mint), b58decode(WSOL)], AMM)[0];
}
export const ata = (owner: string, mint: string, tokenProgram: string) => findPda([b58decode(owner), b58decode(tokenProgram), b58decode(mint)], ATA_PROGRAM)[0];

export interface Pool {
  address: string;
  creator: string;
  baseMint: string;
  quoteMint: string;
  lpMint: string;
  baseVault: string;
  quoteVault: string;
  lpSupply: bigint;
  coinCreator: string;
  virtualQuote: bigint;
  protocolFees: bigint;
  creatorFees: bigint;
}

/** pump_amm Pool: bump 8, index 9, creator 11, base_mint 43, quote_mint 75, lp_mint 107, vaults 139/171, lp_supply 203, coin_creator 211, virtual quote 245 (i128), protocol/creator fees 271/279. */
export function decodePool(address: string, d: Buffer): Pool {
  if (d.length < 243) throw new Error(`pool ${address}: ${d.length} bytes, not a PumpSwap pool`);
  const vq = d.length >= 261 ? d.readBigUInt64LE(245) + (d.readBigInt64LE(253) << 64n) : 0n;
  return {
    address,
    creator: b58encode(d.subarray(11, 43)),
    baseMint: b58encode(d.subarray(43, 75)),
    quoteMint: b58encode(d.subarray(75, 107)),
    lpMint: b58encode(d.subarray(107, 139)),
    baseVault: b58encode(d.subarray(139, 171)),
    quoteVault: b58encode(d.subarray(171, 203)),
    lpSupply: d.readBigUInt64LE(203),
    coinCreator: b58encode(d.subarray(211, 243)),
    virtualQuote: vq < 0n ? 0n : vq,
    protocolFees: d.length >= 287 ? d.readBigUInt64LE(271) : 0n,
    creatorFees: d.length >= 287 ? d.readBigUInt64LE(279) : 0n,
  };
}

export const tokenAmount = (d: Buffer) => d.readBigUInt64LE(64);
export const tokenOwner = (d: Buffer) => b58encode(d.subarray(32, 64));
export const mintSupply = (d: Buffer) => d.readBigUInt64LE(36);
export const mintDecimals = (d: Buffer) => d[44];

/** Tradeable reserves as the program prices them: quote vault less fees held for sweeping, plus virtual quote. */
export function reserves(p: Pool, baseVault: Buffer, quoteVault: Buffer): { base: bigint; quote: bigint } {
  let q = tokenAmount(quoteVault) - p.protocolFees - p.creatorFees;
  if (q < 0n) q = 0n;
  return { base: tokenAmount(baseVault), quote: q + p.virtualQuote };
}

export interface Fees { lp: number; protocol: number; creator: number }
export interface FeeConfig { flat: Fees; tiers: { threshold: bigint; fees: Fees }[] }

/** pump_fees FeeConfig: disc 8, bump 1, admin 32, flat_fees (lp, protocol, creator u64), fee_tiers Vec<(u128 threshold, Fees)>. */
export function decodeFeeConfig(d: Buffer): FeeConfig {
  let o = 8 + 1 + 32;
  const fees = (): Fees => {
    const f = { lp: Number(d.readBigUInt64LE(o)), protocol: Number(d.readBigUInt64LE(o + 8)), creator: Number(d.readBigUInt64LE(o + 16)) };
    o += 24;
    return f;
  };
  const flat = fees();
  const n = d.readUInt32LE(o);
  o += 4;
  if (n > 64) throw new Error(`fee config: ${n} tiers, not a FeeConfig`);
  const tiers: FeeConfig["tiers"] = [];
  for (let i = 0; i < n; i++) {
    const threshold = d.readBigUInt64LE(o) + (d.readBigUInt64LE(o + 8) << 64n);
    o += 16;
    tiers.push({ threshold, fees: fees() });
  }
  return { flat, tiers };
}

/** Market cap in lamports as pump prices tiers: quote x supply / base. */
export const mcapLamports = (r: { base: bigint; quote: bigint }, supply: bigint) => (r.base === 0n ? 0n : (r.quote * supply) / r.base);

/** The tier whose threshold is the largest one at or below the market cap (canonical SOL pools). */
export function feesFor(cfg: FeeConfig, mcap: bigint, canonical: boolean): Fees {
  if (!canonical || cfg.tiers.length === 0) return { ...cfg.flat, creator: canonical ? cfg.flat.creator : 0 };
  let f = cfg.tiers[0].fees;
  for (const t of cfg.tiers) if (mcap >= t.threshold) f = t.fees;
  return f;
}
export const totalBps = (f: Fees) => f.lp + f.protocol + f.creator;

const ceilDiv = (a: bigint, b: bigint) => (a + b - 1n) / b;

/** Buy with exactly `solIn` lamports (fees taken from the input): base units out. */
export function buyOut(r: { base: bigint; quote: bigint }, f: Fees, solIn: bigint): bigint {
  if (solIn <= 0n) return 0n;
  const eff = (solIn * 10_000n) / (10_000n + BigInt(totalBps(f)));
  return (r.base * eff) / (r.quote + eff);
}

/** Sell `baseIn` base units: lamports out after each fee (rounded up, as the program charges them). */
export function sellOut(r: { base: bigint; quote: bigint }, f: Fees, baseIn: bigint): bigint {
  if (baseIn <= 0n) return 0n;
  const gross = (r.quote * baseIn) / (r.base + baseIn);
  const fee = ceilDiv(gross * BigInt(f.lp), 10_000n) + ceilDiv(gross * BigInt(f.protocol), 10_000n) + ceilDiv(gross * BigInt(f.creator), 10_000n);
  return gross > fee ? gross - fee : 0n;
}

/** Lamports per base unit x 1e12 (the price unit committed on Cadre). */
export const priceE12 = (r: { base: bigint; quote: bigint }) => (r.base === 0n ? 0n : (r.quote * 1_000_000_000_000n) / r.base);
