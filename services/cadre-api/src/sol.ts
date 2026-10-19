// Solana side for cadre-api, no dependencies: account decoders for the bridge, the
// reserve and the pump curve, and UNSIGNED legacy transactions for the user's wallet to
// sign (bridge deposit, x402 payment). The API never holds or asks for a user key; it
// returns the transaction bytes with empty signature slots, the wallet signs, and the
// site either sends it itself or posts the signed bytes to /v1/tx/solana.
//
// Message compile mirrors @solana/web3.js `Transaction.compileMessage` (test/sol.test.ts
// checks it byte for byte against web3.js from services/storefront-x402/node_modules).

import { b58decode, b58encode, findPda, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import { BRIDGE_ID, STOREFRONT_ID } from "../../../apps/explorer/src/decode.ts";

export const SYSTEM = "11111111111111111111111111111111";
export const TOKEN = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const TOKEN_2022 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
export const ATA_PROGRAM = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
export const COMPUTE_BUDGET = "ComputeBudget111111111111111111111111111111";
export const PUMP = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
export const AMM = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
export const RESERVE_ID = process.env.RESERVE_PROGRAM || "4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p";
export { BRIDGE_ID, STOREFRONT_ID };

export interface Meta {
  pubkey: string;
  isSigner: boolean;
  isWritable: boolean;
}
export interface Ix {
  programId: string;
  keys: Meta[];
  data: Uint8Array;
}
const meta = (pubkey: string, isWritable = false, isSigner = false): Meta => ({ pubkey, isSigner, isWritable });

export const disc = (name: string) => sha256(utf8(`global:${name}`)).subarray(0, 8);
export const accountDisc = (name: string) => sha256(utf8(`account:${name}`)).subarray(0, 8);

const u64 = (n: bigint) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(n);
  return Uint8Array.from(b);
};
const u32 = (n: number) => {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return Uint8Array.from(b);
};
const cat = (...p: Uint8Array[]) => Uint8Array.from(Buffer.concat(p.map((x) => Buffer.from(x))));

export function isAddress(s: unknown): s is string {
  if (typeof s !== "string" || s.length < 32 || s.length > 44) return false;
  try {
    return b58decode(s).length === 32;
  } catch {
    return false;
  }
}

export const ata = (owner: string, mint: string, tokenProgram: string) => findPda([b58decode(owner), b58decode(tokenProgram), b58decode(mint)], ATA_PROGRAM)[0];

// ------------------------------------------------------------ instructions

export const systemTransfer = (from: string, to: string, lamports: bigint): Ix => ({
  programId: SYSTEM,
  keys: [meta(from, true, true), meta(to, true)],
  data: cat(u32(2), u64(lamports)),
});

export const computeUnitLimit = (units: number): Ix => ({ programId: COMPUTE_BUDGET, keys: [], data: cat(Uint8Array.of(2), u32(units)) });

export const createAtaIdempotent = (payer: string, owner: string, mint: string, tokenProgram: string): Ix => ({
  programId: ATA_PROGRAM,
  keys: [meta(payer, true, true), meta(ata(owner, mint, tokenProgram), true), meta(owner), meta(mint), meta(SYSTEM), meta(tokenProgram)],
  data: Uint8Array.of(1),
});

// ------------------------------------------------------------ bridge (token/programs/bridge)

export interface BridgeConfig {
  admin: string;
  mint: string;
  vault: string;
  tokenProgram: string;
  depositNonce: bigint;
  threshold: number;
}

export const bridgeConfigPda = () => findPda([utf8("config")], BRIDGE_ID)[0];
export const depositPda = (nonce: bigint) => findPda([utf8("deposit"), u64(nonce)], BRIDGE_ID)[0];

export function decodeBridgeConfig(d: Uint8Array): BridgeConfig | null {
  if (d.length < 145 || !Buffer.from(d.subarray(0, 8)).equals(Buffer.from(accountDisc("Config")))) return null;
  const b = Buffer.from(d);
  return {
    admin: b58encode(d.subarray(8, 40)),
    mint: b58encode(d.subarray(40, 72)),
    vault: b58encode(d.subarray(72, 104)),
    tokenProgram: b58encode(d.subarray(104, 136)),
    depositNonce: b.readBigUInt64LE(136),
    threshold: d[144],
  };
}

export interface BridgeDeposit {
  address: string;
  nonce: bigint;
  sender: string;
  recipient: string;
  amount: bigint;
  slot: bigint;
}
export function decodeBridgeDeposit(address: string, d: Uint8Array): BridgeDeposit | null {
  if (d.length < 96 || !Buffer.from(d.subarray(0, 8)).equals(Buffer.from(accountDisc("Deposit")))) return null;
  const b = Buffer.from(d);
  return { address, nonce: b.readBigUInt64LE(8), sender: b58encode(d.subarray(16, 48)), recipient: b58encode(d.subarray(48, 80)), amount: b.readBigUInt64LE(80), slot: b.readBigUInt64LE(88) };
}

/** bridge.deposit(amount, agent_recipient): locks `amount` SPL base units in the vault for `recipient` on the agent chain. */
export function bridgeDepositIx(cfg: BridgeConfig, owner: string, amount: bigint, agentRecipient: string): Ix {
  return {
    programId: BRIDGE_ID,
    keys: [
      meta(owner, true, true),
      meta(owner, false, true),
      meta(bridgeConfigPda(), true),
      meta(cfg.mint),
      meta(ata(owner, cfg.mint, cfg.tokenProgram), true),
      meta(cfg.vault, true),
      meta(depositPda(cfg.depositNonce), true),
      meta(cfg.tokenProgram),
      meta(SYSTEM),
    ],
    data: cat(disc("deposit"), u64(amount), b58decode(agentRecipient)),
  };
}

// ------------------------------------------------------------ token accounts, mints

export const tokenAccountAmount = (d: Uint8Array) => (d.length >= 72 ? Buffer.from(d).readBigUInt64LE(64) : 0n);
export const tokenAccountOwner = (d: Uint8Array) => b58encode(d.subarray(32, 64));
export const mintSupply = (d: Uint8Array) => Buffer.from(d).readBigUInt64LE(36);
export const mintDecimals = (d: Uint8Array) => d[44];

// ------------------------------------------------------------ reserve (token/programs/reserve)

export const reserveConfigPda = () => findPda([utf8("config")], RESERVE_ID)[0];
export const reserveCreatorPda = () => findPda([utf8("creator")], RESERVE_ID)[0];
/** The reserve's token account: the creator PDA's Token-2022 ATA (services/storefront-x402 reserveAta). */
export const reserveAta = (mint: string) => ata(reserveCreatorPda(), mint, TOKEN_2022);

export interface ReserveConfig {
  admin: string;
  mint: string;
  cranker: string;
  bridgeProgram: string;
  miningPool: string;
  maxBuyPerCall: bigint;
  minBuy: bigint;
  minIntervalSecs: bigint;
  maxSlippageBps: number;
  solFloor: bigint;
  maxFundPerCall: bigint;
  lastCrankTs: bigint;
  totalBought: bigint;
  totalSpent: bigint;
  totalFunded: bigint;
  /** Bytes after the known layout (fields appended by later program versions). */
  tailLen: number;
}

/** Reserve Config, layout as of token/programs/reserve 2026-10-19 (fields up to total_funded). */
export function decodeReserveConfig(d: Uint8Array): ReserveConfig | null {
  if (d.length < 8 + 32 * 6 + 8 * 3 + 2 + 8 * 6 || !Buffer.from(d.subarray(0, 8)).equals(Buffer.from(accountDisc("Config")))) return null;
  const b = Buffer.from(d);
  let o = 8;
  const key = () => {
    const k = b58encode(d.subarray(o, o + 32));
    o += 32;
    return k;
  };
  const r64 = () => {
    const v = b.readBigUInt64LE(o);
    o += 8;
    return v;
  };
  const i64 = () => {
    const v = b.readBigInt64LE(o);
    o += 8;
    return v;
  };
  const admin = key(), mint = key(), cranker = key();
  key(); // funder (unused since 2026-10-19)
  const bridgeProgram = key(), miningPool = key();
  const maxBuyPerCall = r64(), minBuy = r64(), minIntervalSecs = i64();
  const maxSlippageBps = b.readUInt16LE(o);
  o += 2;
  const solFloor = r64(), maxFundPerCall = r64(), lastCrankTs = i64(), totalBought = r64(), totalSpent = r64(), totalFunded = r64();
  return { admin, mint, cranker, bridgeProgram, miningPool, maxBuyPerCall, minBuy, minIntervalSecs, maxSlippageBps, solFloor, maxFundPerCall, lastCrankTs, totalBought, totalSpent, totalFunded, tailLen: d.length - o };
}

// ------------------------------------------------------------ pump curve / PumpSwap (read-only quotes)

export const bondingCurvePda = (mint: string) => findPda([utf8("bonding-curve"), b58decode(mint)], PUMP)[0];
export function canonicalPool(mint: string): string {
  const poolAuthority = findPda([utf8("pool-authority"), b58decode(mint)], PUMP)[0];
  return findPda([utf8("pool"), Uint8Array.from([0, 0]), b58decode(poolAuthority), b58decode(mint), b58decode("So11111111111111111111111111111111111111112")], AMM)[0];
}
export function decodeCurve(d: Uint8Array) {
  const b = Buffer.from(d);
  return { virtualToken: b.readBigUInt64LE(8), virtualQuote: b.readBigUInt64LE(16), realToken: b.readBigUInt64LE(24), realQuote: b.readBigUInt64LE(32), tokenTotalSupply: b.readBigUInt64LE(40), complete: d[48] === 1 };
}
export function decodePool(d: Uint8Array) {
  // same offsets as services/storefront-x402 decodePool (pump_amm Pool)
  const b = Buffer.from(d);
  const vq = d.length >= 261 ? (b.readBigInt64LE(253) << 64n) + b.readBigUInt64LE(245) : 0n;
  return {
    baseVault: b58encode(d.subarray(139, 171)),
    quoteVault: b58encode(d.subarray(171, 203)),
    virtualQuote: vq < 0n ? 0n : vq,
    protocolFees: d.length >= 287 ? b.readBigUInt64LE(271) : 0n,
    creatorFees: d.length >= 287 ? b.readBigUInt64LE(279) : 0n,
  };
}
/** Constant product out for `quoteIn`, before fees (the same quote storefront-x402 uses). */
export const cpQuote = (baseReserve: bigint, quoteReserve: bigint, quoteIn: bigint) => (quoteIn <= 0n ? 0n : (baseReserve * quoteIn) / (quoteReserve + quoteIn));

// ------------------------------------------------------------ legacy message compile

function shortvec(n: number): number[] {
  const out: number[] = [];
  let v = n;
  for (;;) {
    let e = v & 0x7f;
    v >>= 7;
    if (v === 0) {
      out.push(e);
      return out;
    }
    e |= 0x80;
    out.push(e);
  }
}

export interface Compiled {
  message: Uint8Array;
  accountKeys: string[];
  numRequiredSignatures: number;
}

/** Same ordering as web3.js Transaction.compileMessage: fee payer first, then signer-writable, signer-readonly, writable, readonly. */
export function compileLegacy(ixs: Ix[], feePayer: string, recentBlockhash: string): Compiled {
  type E = { key: string; s: boolean; w: boolean };
  const metas: E[] = [];
  for (const ix of ixs) {
    for (const k of ix.keys) metas.push({ key: k.pubkey, s: k.isSigner, w: k.isWritable });
  }
  for (const ix of ixs) metas.push({ key: ix.programId, s: false, w: false });
  const uniq: E[] = [];
  for (const m of metas) {
    const e = uniq.find((x) => x.key === m.key);
    if (e) {
      e.w ||= m.w;
      e.s ||= m.s;
    } else uniq.push({ ...m });
  }
  uniq.sort((x, y) => {
    if (x.s !== y.s) return x.s ? -1 : 1;
    if (x.w !== y.w) return x.w ? -1 : 1;
    return x.key.localeCompare(y.key, "en", { localeMatcher: "best fit", usage: "sort", sensitivity: "variant", ignorePunctuation: false, numeric: false, caseFirst: "lower" });
  });
  const pi = uniq.findIndex((x) => x.key === feePayer);
  if (pi > -1) {
    const [p] = uniq.splice(pi, 1);
    p.s = true;
    p.w = true;
    uniq.unshift(p);
  } else uniq.unshift({ key: feePayer, s: true, w: true });

  const signedW = uniq.filter((x) => x.s && x.w), signedR = uniq.filter((x) => x.s && !x.w);
  const unsignedW = uniq.filter((x) => !x.s && x.w), unsignedR = uniq.filter((x) => !x.s && !x.w);
  const keys = [...signedW, ...signedR, ...unsignedW, ...unsignedR].map((x) => x.key);
  const idx = (k: string) => keys.indexOf(k);
  const out: number[] = [signedW.length + signedR.length, signedR.length, unsignedR.length, ...shortvec(keys.length)];
  for (const k of keys) out.push(...b58decode(k));
  const bh = b58decode(recentBlockhash);
  if (bh.length !== 32) throw new Error("bad recent blockhash");
  out.push(...bh, ...shortvec(ixs.length));
  for (const ix of ixs) {
    out.push(idx(ix.programId), ...shortvec(ix.keys.length), ...ix.keys.map((k) => idx(k.pubkey)), ...shortvec(ix.data.length), ...ix.data);
  }
  return { message: Uint8Array.from(out), accountKeys: keys, numRequiredSignatures: signedW.length + signedR.length };
}

/** Wire transaction with every signature slot zeroed (the wallet fills its slot). */
export function unsignedTx(c: Compiled): Uint8Array {
  return Uint8Array.from([...shortvec(c.numRequiredSignatures), ...new Uint8Array(64 * c.numRequiredSignatures), ...c.message]);
}

/** Parses a wire tx (legacy or v0) far enough to check its signatures: signatures, message bytes, signer keys. */
export function parseWireTx(tx: Uint8Array): { sigs: Uint8Array[]; message: Uint8Array; signers: string[]; version: "legacy" | 0 } {
  const rdShort = (buf: Uint8Array, at: { o: number }) => {
    let len = 0, size = 0;
    for (;;) {
      if (at.o >= buf.length) throw new Error("truncated tx");
      const e = buf[at.o++];
      len |= (e & 0x7f) << (size * 7);
      size++;
      if ((e & 0x80) === 0) return len;
      if (size > 3) throw new Error("bad shortvec");
    }
  };
  const at = { o: 0 };
  const n = rdShort(tx, at);
  if (n < 1 || n > 32) throw new Error("bad signature count");
  const sigs: Uint8Array[] = [];
  for (let i = 0; i < n; i++, at.o += 64) sigs.push(tx.subarray(at.o, at.o + 64));
  if (at.o > tx.length) throw new Error("truncated tx");
  const message = tx.subarray(at.o);
  const m = { o: 0 };
  let version: "legacy" | 0 = "legacy";
  if (message[0] & 0x80) {
    if ((message[0] & 0x7f) !== 0) throw new Error("unsupported tx version");
    version = 0;
    m.o = 1;
  }
  const nreq = message[m.o];
  m.o += 3;
  const nkeys = rdShort(message, m);
  if (nreq !== n) throw new Error(`message needs ${nreq} signatures, tx carries ${n}`);
  if (nkeys < nreq || m.o + 32 * nkeys > message.length) throw new Error("bad account keys");
  const signers: string[] = [];
  for (let i = 0; i < nreq; i++) signers.push(b58encode(message.subarray(m.o + 32 * i, m.o + 32 * i + 32)));
  return { sigs, message, signers, version };
}

// ------------------------------------------------------------ market txs (pump buy, crank buyback, fund_mining)
//
// The account lists below mirror what services/crank already sends live (scripts/demo):
// pump `buy_exact_quote_in_v3` as @pump-fun/pump-sdk 4.1.0 builds it (same 17 accounts the
// storefront's crank_buyback CPI passes), PumpSwap `buy_exact_quote_in_v2` as
// @pump-fun/pump-swap-sdk 2.1.0 builds it, and the storefront / reserve instructions with the
// account sets of services/crank/src/{storefront,accounts}.ts (StorefrontKeys, ReserveKeys.fund).
// test/market-tx.test.ts compares them with those builders.

export const FEES = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";
export const WSOL = "So11111111111111111111111111111111111111112";
export const CLOCK = "SysvarC1ock11111111111111111111111111111111";
/** The storefront buyback vault keeps this much (services/crank RENT_MIN, the program's RENT_MIN). */
export const STORE_RENT_MIN = 890_880n;

const seedKey = (k: string) => b58decode(k);
const pumpPda = (...s: Uint8Array[]) => findPda(s, PUMP)[0];
const ammPda = (...s: Uint8Array[]) => findPda(s, AMM)[0];

export const pumpGlobalPda = () => pumpPda(utf8("global"));
export const pumpEventAuthority = () => pumpPda(utf8("__event_authority"));
export const pumpUserVolumeAccumulator = (user: string) => pumpPda(utf8("user_volume_accumulator"), seedKey(user));
export const pumpFeeConfig = () => findPda([utf8("fee_config"), seedKey(PUMP)], FEES)[0];
export const ammGlobalConfigPda = () => ammPda(utf8("global_config"));
export const ammEventAuthority = () => ammPda(utf8("__event_authority"));
export const ammUserVolumeAccumulator = (user: string) => ammPda(utf8("user_volume_accumulator"), seedKey(user));
export const ammFeeConfig = () => findPda([utf8("fee_config"), seedKey(AMM)], FEES)[0];

const IX_PUMP_BUY_EXACT_QUOTE_IN_V3 = Uint8Array.from([225, 247, 80, 30, 213, 179, 132, 136]);
const IX_AMM_BUY_EXACT_QUOTE_IN_V2 = Uint8Array.from([194, 171, 28, 70, 104, 77, 91, 47]);

function keyList(d: Uint8Array, at: number, n: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < n; i++) out.push(b58encode(d.subarray(at + 32 * i, at + 32 * i + 32)));
  return out;
}
/** pump Global.buyback_fee_recipients (8 keys at 741, the layout of the fixtures/pump Global and pump-sdk 4.1.0). */
export function pumpBuybackRecipients(d: Uint8Array): string[] {
  if (d.length < 741 + 256) throw new Error("pump Global too short for buyback_fee_recipients");
  return keyList(d, 741, 8);
}
/** PumpSwap GlobalConfig.buyback_fee_recipients (8 keys at 643, pump-swap-sdk 2.1.0 layout). */
export function ammBuybackRecipients(d: Uint8Array): string[] {
  if (d.length < 643 + 256) throw new Error("PumpSwap GlobalConfig too short for buyback_fee_recipients");
  return keyList(d, 643, 8);
}
/** Pool keys the PumpSwap buy needs (pump_amm Pool: base_mint 43, quote_mint 75, vaults 139 / 171, coin_creator 211). */
export function decodePoolKeys(d: Uint8Array) {
  return { baseMint: b58encode(d.subarray(43, 75)), quoteMint: b58encode(d.subarray(75, 107)), baseVault: b58encode(d.subarray(139, 171)), quoteVault: b58encode(d.subarray(171, 203)), coinCreator: b58encode(d.subarray(211, 243)) };
}

export const syncNative = (account: string): Ix => ({ programId: TOKEN, keys: [meta(account, true)], data: Uint8Array.of(17) });
export const closeAccount = (account: string, dest: string, owner: string): Ix => ({ programId: TOKEN, keys: [meta(account, true), meta(dest, true), meta(owner, false, true)], data: Uint8Array.of(9) });

/** pump `buy_exact_quote_in_v3(spendable_quote_in, min_tokens_out, partial_fill = false)` on a SOL-quoted curve. */
export function pumpBuyExactQuoteInIx(user: string, mint: string, spendableQuoteIn: bigint, minTokensOut: bigint, buybackFeeRecipient: string, tokenProgram = TOKEN_2022): Ix {
  const bc = bondingCurvePda(mint);
  return {
    programId: PUMP,
    keys: [
      meta(pumpGlobalPda()),
      meta(mint),
      meta(WSOL),
      meta(tokenProgram),
      meta(TOKEN),
      meta(bc, true),
      meta(ata(bc, mint, tokenProgram), true),
      meta(ata(bc, WSOL, TOKEN), true),
      meta(user, true, true),
      meta(ata(user, mint, tokenProgram), true),
      meta(ata(user, WSOL, TOKEN), true),
      meta(pumpUserVolumeAccumulator(user), true),
      meta(pumpFeeConfig()),
      meta(buybackFeeRecipient, true),
      meta(SYSTEM),
      meta(pumpEventAuthority()),
      meta(PUMP),
    ],
    data: cat(IX_PUMP_BUY_EXACT_QUOTE_IN_V3, u64(spendableQuoteIn), u64(minTokensOut), Uint8Array.of(0)),
  };
}

export interface PoolKeys {
  pool: string;
  baseMint: string;
  quoteMint: string;
  baseVault: string;
  quoteVault: string;
}
/** PumpSwap `buy_exact_quote_in_v2(spendable_quote_in, min_base_amount_out)`; `ammBuybackOwner` is a GlobalConfig buyback recipient (its wSOL ATA is passed). */
export function ammBuyExactQuoteInIx(user: string, p: PoolKeys, spendableQuoteIn: bigint, minBaseOut: bigint, ammBuybackOwner: string, baseTokenProgram = TOKEN_2022): Ix {
  return {
    programId: AMM,
    keys: [
      meta(p.pool, true),
      meta(user, true, true),
      meta(ammGlobalConfigPda()),
      meta(p.baseMint),
      meta(p.quoteMint),
      meta(ata(user, p.baseMint, baseTokenProgram), true),
      meta(ata(user, p.quoteMint, TOKEN), true),
      meta(p.baseVault, true),
      meta(p.quoteVault, true),
      meta(baseTokenProgram),
      meta(TOKEN),
      meta(SYSTEM),
      meta(ammUserVolumeAccumulator(user), true),
      meta(ammFeeConfig()),
      meta(ata(ammBuybackOwner, p.quoteMint, TOKEN), true),
      meta(ammEventAuthority()),
      meta(AMM),
    ],
    data: cat(IX_AMM_BUY_EXACT_QUOTE_IN_V2, u64(spendableQuoteIn), u64(minBaseOut)),
  };
}

// ---- storefront buyback crank (token/programs/storefront crank_buyback / crank_buyback_amm)

export interface StoreCrankConfig {
  mint: string;
  reserveProgram: string;
  /** zero key = permissionless */
  cranker: string;
  maxPerCall: bigint;
  minBuy: bigint;
  minIntervalSecs: bigint;
  lastTs: bigint;
  locked: bigint;
  reserveCap: bigint;
}
/** Storefront Config buyback fields (offsets as apps/explorer decodeStoreBuyback: tail after the attesters vec). */
export function decodeStoreCrankConfig(d: Uint8Array): StoreCrankConfig | null {
  if (d.length < 146 || !Buffer.from(d.subarray(0, 8)).equals(Buffer.from(accountDisc("Config")))) return null;
  const b0 = Buffer.from(d);
  const n = b0.readUInt32LE(142);
  const at = 146 + 32 * n;
  const b = at + 59;
  if (d.length < at + 246) return null; // a program before held shares (bb_locked): not the crank this API builds for
  return {
    mint: b58encode(d.subarray(40, 72)),
    reserveProgram: b58encode(d.subarray(b + 3, b + 35)),
    cranker: b58encode(d.subarray(b + 35, b + 67)),
    maxPerCall: b0.readBigUInt64LE(b + 67),
    minBuy: b0.readBigUInt64LE(b + 75),
    minIntervalSecs: b0.readBigInt64LE(b + 83),
    lastTs: b0.readBigInt64LE(b + 91),
    locked: b0.readBigUInt64LE(b + 171),
    reserveCap: d.length >= at + 254 ? b0.readBigUInt64LE(b + 187) : 0n,
  };
}

export const storeConfigAddress = () => findPda([utf8("config")], STOREFRONT_ID)[0];
export const storeBuybackVault = () => findPda([utf8("buyback")], STOREFRONT_ID)[0];

/** storefront.crank_buyback(max_sol_in, min_tokens_out): accounts as services/crank StorefrontKeys.curve. */
export function storeCrankBuybackIx(cranker: string, c: StoreCrankConfig, maxSolIn: bigint, minTokensOut: bigint, buybackFeeRecipient: string): Ix {
  const vault = storeBuybackVault();
  const bc = bondingCurvePda(c.mint);
  const reserveAtaAddr = ata(findPda([utf8("creator")], c.reserveProgram)[0], c.mint, TOKEN_2022);
  return {
    programId: STOREFRONT_ID,
    keys: [
      meta(cranker, true, true),
      meta(storeConfigAddress(), true),
      meta(vault, true),
      meta(c.mint, true),
      meta(ata(vault, c.mint, TOKEN_2022), true),
      meta(ata(vault, WSOL, TOKEN), true),
      meta(reserveAtaAddr, true),
      meta(pumpGlobalPda()),
      meta(bc, true),
      meta(ata(bc, c.mint, TOKEN_2022), true),
      meta(ata(bc, WSOL, TOKEN), true),
      meta(pumpUserVolumeAccumulator(vault), true),
      meta(pumpFeeConfig()),
      meta(buybackFeeRecipient, true),
      meta(WSOL),
      meta(TOKEN),
      meta(TOKEN_2022),
      meta(ATA_PROGRAM),
      meta(SYSTEM),
      meta(pumpEventAuthority()),
      meta(PUMP),
    ],
    data: cat(disc("crank_buyback"), u64(maxSolIn), u64(minTokensOut)),
  };
}

/** storefront.crank_buyback_amm(max_sol_in, min_tokens_out): accounts as services/crank StorefrontKeys.amm. */
export function storeCrankBuybackAmmIx(cranker: string, c: StoreCrankConfig, p: PoolKeys, maxSolIn: bigint, minTokensOut: bigint, ammBuybackOwner: string): Ix {
  const vault = storeBuybackVault();
  const reserveAtaAddr = ata(findPda([utf8("creator")], c.reserveProgram)[0], c.mint, TOKEN_2022);
  return {
    programId: STOREFRONT_ID,
    keys: [
      meta(cranker, true, true),
      meta(storeConfigAddress(), true),
      meta(vault, true),
      meta(c.mint, true),
      meta(ata(vault, c.mint, TOKEN_2022), true),
      meta(ata(vault, WSOL, TOKEN), true),
      meta(reserveAtaAddr, true),
      meta(p.pool, true),
      meta(p.baseVault, true),
      meta(p.quoteVault, true),
      meta(ammGlobalConfigPda()),
      meta(ammUserVolumeAccumulator(vault), true),
      meta(ammFeeConfig()),
      meta(ata(ammBuybackOwner, WSOL, TOKEN), true),
      meta(WSOL),
      meta(TOKEN),
      meta(TOKEN_2022),
      meta(ATA_PROGRAM),
      meta(SYSTEM),
      meta(ammEventAuthority()),
      meta(AMM),
    ],
    data: cat(disc("crank_buyback_amm"), u64(maxSolIn), u64(minTokensOut)),
  };
}

// ---- reserve fund_mining (token/programs/reserve, permissionless + rate limited since 2026-10-19)

export interface ReserveFundLimits {
  layout: number;
  paused: boolean;
  minIntervalSecs: bigint;
  windowSecs: bigint;
  maxPerWindow: bigint;
  lastTs: bigint;
  windowStart: bigint;
  windowUsed: bigint;
  calls: bigint;
}
/** The fund-limit tail of the reserve Config (after bump, creator_bump), or null on an unmigrated config. */
export function decodeReserveFundLimits(d: Uint8Array): ReserveFundLimits | null {
  const o = 8 + 32 * 6 + 8 * 3 + 2 + 8 * 6 + 2; // through total_funded, then bump + creator_bump
  if (d.length < o + 2 + 8 * 7) return null;
  const b = Buffer.from(d);
  return {
    layout: d[o],
    paused: d[o + 1] !== 0,
    minIntervalSecs: b.readBigInt64LE(o + 2),
    windowSecs: b.readBigInt64LE(o + 10),
    maxPerWindow: b.readBigUInt64LE(o + 18),
    lastTs: b.readBigInt64LE(o + 26),
    windowStart: b.readBigInt64LE(o + 34),
    windowUsed: b.readBigUInt64LE(o + 42),
    calls: b.readBigUInt64LE(o + 50),
  };
}

/** reserve.fund_mining(amount): accounts as services/crank ReserveKeys.fund (bridge Deposit PDA keyed by the bridge's deposit nonce now). */
export function reserveFundMiningIx(authority: string, mint: string, bridgeProgram: string, depositNonce: bigint, amount: bigint): Ix {
  const bcfg = findPda([utf8("config")], bridgeProgram)[0];
  return {
    programId: RESERVE_ID,
    keys: [
      meta(authority, true, true),
      meta(reserveConfigPda(), true),
      meta(reserveCreatorPda()),
      meta(mint),
      meta(reserveAta(mint), true),
      meta(bcfg, true),
      meta(ata(bcfg, mint, TOKEN_2022), true),
      meta(findPda([utf8("deposit"), u64(depositNonce)], bridgeProgram)[0], true),
      meta(TOKEN_2022),
      meta(SYSTEM),
      meta(bridgeProgram),
    ],
    data: cat(disc("fund_mining"), u64(amount)),
  };
}

/** Clock sysvar unix_timestamp (i64 at 32). */
export const clockUnixTs = (d: Uint8Array) => Number(Buffer.from(d).readBigInt64LE(32));
