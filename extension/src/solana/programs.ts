// Instruction builders and account readers for the token-chain programs:
// bridge (deposit), storefront (open_order, buy_job, buy_job_amm, cancel_draft),
// plus System / Token-2022 / ATA / ComputeBudget. Hand-encoded Borsh against
// token/target/idl/*.json (checked by test/solana.test.ts with Anchor's coder).
// Account layouts follow services/storefront-x402/src/storefront.ts.

import { concat, u16, u32, u64, u8, utf8 } from "../core/bytes";
import { sha256 } from "../core/crypto";
import {
  ATA_PROGRAM, COMPUTE_BUDGET, Instruction, Pubkey, SYSTEM_PROGRAM, TOKEN_2022, TOKEN_PROGRAM, WSOL, ata, meta, pda, pk,
} from "./tx";

export const PUMP = pk("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
export const AMM = pk("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
export const PUMP_FEES = pk("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");

export const disc = (name: string) => sha256(utf8(`global:${name}`)).subarray(0, 8);
export const accountDisc = (name: string) => sha256(utf8(`account:${name}`)).subarray(0, 8);
const borshString = (s: string) => {
  const b = utf8(s);
  return concat(u32(b.length), b);
};

// ---------------------------------------------------------------- system / token

export function systemTransfer(from: Pubkey, to: Pubkey, lamports: bigint): Instruction {
  return { programId: SYSTEM_PROGRAM, keys: [meta(from, true, true), meta(to, true)], data: concat(u32(2), u64(lamports)) };
}

export function computeUnitLimit(units: number): Instruction {
  return { programId: COMPUTE_BUDGET, keys: [], data: concat(u8(2), u32(units)) };
}

/** AssociatedTokenAccount CreateIdempotent (instruction 1). */
export function createAtaIdempotent(payer: Pubkey, owner: Pubkey, mint: Pubkey, tokenProgram: Pubkey): Instruction {
  return {
    programId: ATA_PROGRAM,
    keys: [meta(payer, true, true), meta(ata(owner, mint, tokenProgram), true), meta(owner), meta(mint), meta(SYSTEM_PROGRAM), meta(tokenProgram)],
    data: u8(1),
  };
}

/** SPL Token / Token-2022 TransferChecked (12). */
export function transferChecked(source: Pubkey, mint: Pubkey, dest: Pubkey, owner: Pubkey, amount: bigint, decimals: number, tokenProgram: Pubkey): Instruction {
  return {
    programId: tokenProgram,
    keys: [meta(source, true), meta(mint), meta(dest, true), meta(owner, false, true)],
    data: concat(u8(12), u64(amount), u8(decimals)),
  };
}

// ---------------------------------------------------------------- bridge

export interface BridgeConfig {
  admin: Pubkey;
  mint: Pubkey;
  vault: Pubkey;
  tokenProgram: Pubkey;
  depositNonce: bigint;
  threshold: number;
}

export const bridgeConfigPda = (bridge: Pubkey) => pda([utf8("config")], bridge);
export const depositPda = (bridge: Pubkey, nonce: bigint) => pda([utf8("deposit"), u64(nonce)], bridge);
/** Processed marker for an agent-chain withdrawal (source chain 1 = agent). */
export const processedPda = (bridge: Pubkey, withdrawalNonce: bigint) => pda([utf8("processed"), u8(1), u64(withdrawalNonce)], bridge);

export function decodeBridgeConfig(d: Uint8Array): BridgeConfig {
  checkDisc(d, "Config");
  const dv = new DataView(d.buffer, d.byteOffset, d.byteLength);
  return {
    admin: d.slice(8, 40), mint: d.slice(40, 72), vault: d.slice(72, 104), tokenProgram: d.slice(104, 136),
    depositNonce: dv.getBigUint64(136, true), threshold: d[144],
  };
}

export interface BridgeDeposit {
  nonce: bigint;
  sender: Pubkey;
  recipient: Pubkey;
  amount: bigint;
  slot: bigint;
}
export function decodeBridgeDeposit(d: Uint8Array): BridgeDeposit {
  checkDisc(d, "Deposit");
  const dv = new DataView(d.buffer, d.byteOffset, d.byteLength);
  return { nonce: dv.getBigUint64(8, true), sender: d.slice(16, 48), recipient: d.slice(48, 80), amount: dv.getBigUint64(80, true), slot: dv.getBigUint64(88, true) };
}

/** bridge.deposit(amount, agent_recipient): locks SPL in the vault for `recipient` on our chain. */
export function bridgeDepositIx(bridge: Pubkey, cfg: BridgeConfig, payer: Pubkey, amount: bigint, agentRecipient: Pubkey): Instruction {
  return {
    programId: bridge,
    keys: [
      meta(payer, true, true), meta(payer, false, true), meta(bridgeConfigPda(bridge), true), meta(cfg.mint),
      meta(ata(payer, cfg.mint, cfg.tokenProgram), true), meta(cfg.vault, true), meta(depositPda(bridge, cfg.depositNonce), true),
      meta(cfg.tokenProgram), meta(SYSTEM_PROGRAM),
    ],
    data: concat(disc("deposit"), u64(amount), agentRecipient),
  };
}

// ---------------------------------------------------------------- storefront

export interface StoreJobSpec {
  m: number;
  n: number;
  k: number;
  config: Uint8Array;
  hashA: Uint8Array;
  hashB: Uint8Array;
  dataUri: string;
  deadlineBlocks: bigint;
  maxBlocks: number;
  evaluators: Pubkey[];
  threshold: number;
}

export function encodeJobSpecArgs(s: StoreJobSpec): Uint8Array {
  if (s.config.length !== 52 || s.hashA.length !== 32 || s.hashB.length !== 32) throw new Error("bad spec byte lengths");
  return concat(
    u32(s.m), u32(s.n), u32(s.k), s.config, s.hashA, s.hashB, borshString(s.dataUri),
    u64(s.deadlineBlocks), u32(s.maxBlocks), u32(s.evaluators.length), ...s.evaluators, u8(s.threshold),
  );
}

export const storeConfigPda = (sf: Pubkey) => pda([utf8("config")], sf);
export const traderPda = (sf: Pubkey) => pda([utf8("trader")], sf);
export const draftPda = (sf: Pubkey, buyer: Pubkey, draftId: bigint) => pda([utf8("draft"), buyer, u64(draftId)], sf);
export const orderPda = (sf: Pubkey, nonce: bigint) => pda([utf8("order"), u64(nonce)], sf);
export const bondingCurvePda = (mint: Pubkey) => pda([utf8("bonding-curve"), mint], PUMP);
export function canonicalPool(mint: Pubkey): Pubkey {
  const auth = pda([utf8("pool-authority"), mint], PUMP);
  return pda([utf8("pool"), u16(0), auth, mint, WSOL], AMM);
}

export function openOrderIx(sf: Pubkey, buyer: Pubkey, draftId: bigint, recipient: Pubkey, spec: StoreJobSpec): Instruction {
  return {
    programId: sf,
    keys: [meta(buyer, true, true), meta(storeConfigPda(sf)), meta(draftPda(sf, buyer, draftId), true), meta(SYSTEM_PROGRAM)],
    data: concat(disc("open_order"), u64(draftId), recipient, encodeJobSpecArgs(spec)),
  };
}

export function cancelDraftIx(sf: Pubkey, buyer: Pubkey, draftId: bigint): Instruction {
  return { programId: sf, keys: [meta(buyer, true, true), meta(draftPda(sf, buyer, draftId), true)], data: disc("cancel_draft") };
}

export interface PumpAccounts {
  mint: Pubkey;
  curveBuyback: Pubkey;
  ammBuyback: Pubkey;
  poolBaseVault?: Pubkey;
  poolQuoteVault?: Pubkey;
}

function bridgeTail(bridge: Pubkey, mint: Pubkey, nonce: bigint) {
  const bc = bridgeConfigPda(bridge);
  return [meta(bc, true), meta(ata(bc, mint, TOKEN_2022), true), meta(depositPda(bridge, nonce), true), meta(bridge)];
}

export function buyJobIx(sf: Pubkey, bridge: Pubkey, buyer: Pubkey, draftId: bigint, nonce: bigint, solIn: bigint, minOut: bigint, p: PumpAccounts): Instruction {
  const trader = traderPda(sf);
  const bc = bondingCurvePda(p.mint);
  return {
    programId: sf,
    keys: [
      meta(buyer, true, true), meta(storeConfigPda(sf), true), meta(draftPda(sf, buyer, draftId), true), meta(orderPda(sf, nonce), true),
      meta(trader, true), meta(p.mint), meta(ata(trader, p.mint, TOKEN_2022), true), meta(ata(trader, WSOL, TOKEN_PROGRAM), true),
      meta(pda([utf8("global")], PUMP)), meta(bc, true), meta(ata(bc, p.mint, TOKEN_2022), true), meta(ata(bc, WSOL, TOKEN_PROGRAM), true),
      meta(pda([utf8("user_volume_accumulator"), trader], PUMP), true), meta(pda([utf8("fee_config"), PUMP], PUMP_FEES)),
      meta(p.curveBuyback, true), meta(WSOL), meta(TOKEN_PROGRAM), meta(TOKEN_2022), meta(ATA_PROGRAM), meta(SYSTEM_PROGRAM),
      meta(pda([utf8("__event_authority")], PUMP)), meta(PUMP), ...bridgeTail(bridge, p.mint, nonce),
    ],
    data: concat(disc("buy_job"), u64(nonce), u64(solIn), u64(minOut)),
  };
}

export function buyJobAmmIx(sf: Pubkey, bridge: Pubkey, buyer: Pubkey, draftId: bigint, nonce: bigint, solIn: bigint, minOut: bigint, p: PumpAccounts): Instruction {
  if (!p.poolBaseVault || !p.poolQuoteVault) throw new Error("pool vaults required");
  const trader = traderPda(sf);
  return {
    programId: sf,
    keys: [
      meta(buyer, true, true), meta(storeConfigPda(sf), true), meta(draftPda(sf, buyer, draftId), true), meta(orderPda(sf, nonce), true),
      meta(trader, true), meta(p.mint), meta(ata(trader, p.mint, TOKEN_2022), true), meta(ata(trader, WSOL, TOKEN_PROGRAM), true),
      meta(canonicalPool(p.mint), true), meta(p.poolBaseVault, true), meta(p.poolQuoteVault, true), meta(pda([utf8("global_config")], AMM)),
      meta(pda([utf8("user_volume_accumulator"), trader], AMM), true), meta(pda([utf8("fee_config"), AMM], PUMP_FEES)),
      meta(ata(p.ammBuyback, WSOL, TOKEN_PROGRAM), true), meta(WSOL), meta(TOKEN_PROGRAM), meta(TOKEN_2022), meta(ATA_PROGRAM), meta(SYSTEM_PROGRAM),
      meta(pda([utf8("__event_authority")], AMM)), meta(AMM), ...bridgeTail(bridge, p.mint, nonce),
    ],
    data: concat(disc("buy_job_amm"), u64(nonce), u64(solIn), u64(minOut)),
  };
}

// ---------------------------------------------------------------- readers

function checkDisc(d: Uint8Array, name: string) {
  const want = accountDisc(name);
  for (let i = 0; i < 8; i++) if (d[i] !== want[i]) throw new Error(`not a ${name} account`);
}
const dvOf = (d: Uint8Array) => new DataView(d.buffer, d.byteOffset, d.byteLength);

export interface StoreConfig {
  admin: Pubkey;
  mint: Pubkey;
  bridgeProgram: Pubkey;
  paused: boolean;
  protocolFeeBps: number;
  maxSlippageBps: number;
  minSolIn: bigint;
  maxSolIn: bigint;
  maxDim: number;
  maxBlocksCap: number;
  maxDeadlineBlocks: bigint;
  threshold: number;
  attesters: Pubkey[];
}

export function decodeStoreConfig(d: Uint8Array): StoreConfig {
  checkDisc(d, "Config");
  const v = dvOf(d);
  const n = v.getUint32(142, true);
  return {
    admin: d.slice(8, 40), mint: d.slice(40, 72), bridgeProgram: d.slice(72, 104), paused: d[104] !== 0,
    protocolFeeBps: v.getUint16(105, true), maxSlippageBps: v.getUint16(107, true), minSolIn: v.getBigUint64(109, true), maxSolIn: v.getBigUint64(117, true),
    maxDim: v.getUint32(125, true), maxBlocksCap: v.getUint32(129, true), maxDeadlineBlocks: v.getBigUint64(133, true), threshold: d[141],
    attesters: Array.from({ length: n }, (_, i) => d.slice(146 + 32 * i, 178 + 32 * i)),
  };
}

export const ORDER_STATUS: Record<number, string> = { 1: "pending", 2: "finalized", 3: "expired" };
export interface OrderView {
  status: string;
  venue: "curve" | "pumpswap";
  nonce: bigint;
  jobId: Uint8Array;
  buyer: Pubkey;
  recipient: Pubkey;
  solSpent: bigint;
  tokens: bigint;
  workerPay: bigint;
  protocolFee: bigint;
  resultHash: Uint8Array | null;
}
export function decodeOrder(d: Uint8Array): OrderView {
  checkDisc(d, "Order");
  const v = dvOf(d);
  return {
    status: ORDER_STATUS[d[9]] ?? `unknown(${d[9]})`, venue: d[10] === 1 ? "pumpswap" : "curve", nonce: v.getBigUint64(12, true),
    jobId: d.slice(20, 52), buyer: d.slice(52, 84), recipient: d.slice(84, 116),
    solSpent: v.getBigUint64(148, true), tokens: v.getBigUint64(156, true), workerPay: v.getBigUint64(164, true), protocolFee: v.getBigUint64(172, true),
    resultHash: d[9] === 2 ? d.slice(228, 260) : null,
  };
}

export const decodeCurve = (d: Uint8Array) => {
  const v = dvOf(d);
  return { virtualToken: v.getBigUint64(8, true), virtualQuote: v.getBigUint64(16, true), complete: d[48] !== 0 };
};
export function decodePool(d: Uint8Array) {
  const v = dvOf(d);
  const vq = d.length >= 261 ? (v.getBigInt64(253, true) << 64n) + v.getBigUint64(245, true) : 0n;
  return { baseVault: d.slice(139, 171), quoteVault: d.slice(171, 203), virtualQuote: vq < 0n ? 0n : vq, protocolFees: v.getBigUint64(271, true), creatorFees: v.getBigUint64(279, true) };
}
export const pumpGlobalBuyback = (d: Uint8Array, i = 0) => d.slice(741 + 32 * i, 773 + 32 * i);
export const ammGlobalBuyback = (d: Uint8Array, i = 0) => d.slice(643 + 32 * i, 675 + 32 * i);
/** SPL token account amount (offset 64), same for Token and Token-2022. */
export const tokenAccountAmount = (d: Uint8Array) => dvOf(d).getBigUint64(64, true);
/** Mint decimals (offset 44). */
export const mintDecimals = (d: Uint8Array) => d[44];

export function cpQuote(baseReserve: bigint, quoteReserve: bigint, quoteIn: bigint): bigint {
  const den = quoteReserve + quoteIn;
  return den === 0n ? 0n : (baseReserve * quoteIn) / den;
}
