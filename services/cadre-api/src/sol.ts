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
