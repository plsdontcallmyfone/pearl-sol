// Raw readers for the accounts and transactions the explorer shows. Offsets come from
// the programs' published layouts (token/programs/storefront `layout`, lightclient
// `layout` / `acc_layout`, Config field order) and docs/RPC.md (chain tx bytes).

import { b58decode, findPda, hex, pk, rd16, rd32, rd64, rdi64, rd8, sha256, slice, u64le, utf8 } from "./codec.ts";

export const STOREFRONT_ID = "DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr";
export const LIGHTCLIENT_ID = "BUDUW2bGexRGRHhEkBUj8dAvffRuXQQacredxKtRMHK5";
export const BRIDGE_ID = "7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8";

export const accountDisc = (name: string) => slice(sha256(utf8(`account:${name}`)), 0, 8);
const discIs = (d: Uint8Array, name: string) => d.length >= 8 && Buffer.compare(Buffer.from(slice(d, 0, 8)), Buffer.from(accountDisc(name))) === 0;

// ------------------------------------------------------------ storefront Order

export const ORDER_STATUS: Record<number, string> = { 1: "pending", 2: "finalized", 3: "expired", 4: "refunded" };
export const ORDER_MIN_LEN = 1070;

export interface Order {
  address: string;
  status: string;
  venue: "curve" | "pumpswap";
  nonce: string;
  jobId: string;
  buyer: string;
  recipient: string;
  bodyHash: string;
  solSpent: string; // lamports
  tokens: string; // SPL base units (6 dp)
  workerPay: string; // chain units (9 dp)
  protocolFee: string; // chain units
  createdSlot: number;
  createdTs: number;
  deliverer: string | null;
  resultHash: string | null;
  closedSlot: number;
  resultUri: string;
  m: number;
  n: number;
  k: number;
  deadlineBlocks: number;
  maxBlocks: number;
  dataUri: string;
}

const ZERO32 = new Uint8Array(32);
const nonZero = (b: Uint8Array) => Buffer.compare(Buffer.from(b), Buffer.from(ZERO32)) !== 0;

export function decodeOrder(address: string, d: Uint8Array): Order | null {
  if (d.length < ORDER_MIN_LEN || !discIs(d, "Order")) return null;
  const SPEC = 470;
  const uriLen = Math.min(rd16(d, 268), 200);
  const dataUriLen = Math.min(rd16(d, SPEC + 142 + 256), 200);
  const deliverer = slice(d, 196, 32);
  const result = slice(d, 228, 32);
  return {
    address,
    status: ORDER_STATUS[rd8(d, 9)] ?? `status ${rd8(d, 9)}`,
    venue: rd8(d, 10) === 1 ? "pumpswap" : "curve",
    nonce: rd64(d, 12).toString(),
    jobId: hex(slice(d, 20, 32)),
    buyer: pk(d, 52),
    recipient: pk(d, 84),
    bodyHash: hex(slice(d, 116, 32)),
    solSpent: rd64(d, 148).toString(),
    tokens: rd64(d, 156).toString(),
    workerPay: rd64(d, 164).toString(),
    protocolFee: rd64(d, 172).toString(),
    createdSlot: Number(rd64(d, 180)),
    createdTs: Number(rdi64(d, 188)),
    deliverer: nonZero(deliverer) ? pk(d, 196) : null,
    resultHash: nonZero(result) ? hex(result) : null,
    closedSlot: Number(rd64(d, 260)),
    resultUri: Buffer.from(slice(d, 270, uriLen)).toString("utf8"),
    m: rd32(d, SPEC),
    n: rd32(d, SPEC + 4),
    k: rd32(d, SPEC + 8),
    deadlineBlocks: Number(rd64(d, SPEC + 128)),
    maxBlocks: rd32(d, SPEC + 136),
    dataUri: Buffer.from(slice(d, SPEC + 144 + 256, dataUriLen)).toString("utf8"),
  };
}

export const orderPda = (nonce: bigint | number | string) => findPda([utf8("order"), u64le(BigInt(nonce))], STOREFRONT_ID)[0];
export const orderJobId = (nonce: bigint | number | string) => hex(sha256(utf8("pearl-sol:order-job:v1"), u64le(BigInt(nonce))));

// ------------------------------------------------------------ light client

export interface LcConfig {
  checkpointHash: string;
  checkpointHeight: number;
  tipHash: string;
  tipHeight: number;
  finalFloor: number;
  finalHeight: number;
  finalHash: string;
  finalityK: number;
  chainId: number;
  retainFinal: number;
}

export function decodeLcConfig(d: Uint8Array): LcConfig | null {
  if (d.length < 318 || !discIs(d, "Config")) return null;
  return {
    chainId: rd32(d, 40),
    finalityK: rd32(d, 44),
    retainFinal: Number(rd64(d, 148)),
    checkpointHash: hex(slice(d, 156, 32)),
    checkpointHeight: Number(rd64(d, 188)),
    tipHash: hex(slice(d, 196, 32)),
    tipHeight: Number(rd64(d, 228)),
    finalFloor: Number(rd64(d, 268)),
    finalHeight: Number(rd64(d, 276)),
    finalHash: hex(slice(d, 284, 32)),
  };
}

export interface LcHeader {
  address: string;
  hash: string;
  height: number;
  verified: boolean;
  final: boolean;
  restored: boolean;
  verifiedSlot: number;
  relayer: string;
}

export function decodeHeaderRecord(address: string, d: Uint8Array): LcHeader | null {
  if (d.length < 472 || !discIs(d, "HeaderRecord")) return null;
  return {
    address,
    hash: hex(slice(d, 8, 32)),
    height: Number(rd64(d, 328)),
    relayer: pk(d, 424),
    verifiedSlot: Number(rd64(d, 456)),
    verified: rd8(d, 465) === 2,
    final: rd8(d, 466) === 1,
    restored: rd8(d, 468) === 1,
  };
}

export interface LcAccumulator {
  startHeight: number;
  count: number;
  lastHash: string;
}

export function decodeAccumulator(d: Uint8Array): LcAccumulator | null {
  if (d.length < 56 || !discIs(d, "Accumulator")) return null;
  return { startHeight: Number(rd64(d, 8)), count: Number(rd64(d, 16)), lastHash: hex(slice(d, 24, 32)) };
}

export const lcConfigPda = () => findPda([utf8("config")], LIGHTCLIENT_ID)[0];
export const lcAccPda = () => findPda([utf8("acc")], LIGHTCLIENT_ID)[0];
export const headerPda = (hashHex: string) => findPda([utf8("header"), Buffer.from(hashHex, "hex")], LIGHTCLIENT_ID)[0];
/** Bridge replay record of an agent-chain withdrawal (source chain 1). */
export const processedWithdrawalPda = (nonce: number | bigint) => findPda([utf8("processed"), Uint8Array.of(1), u64le(BigInt(nonce))], BRIDGE_ID)[0];

// ------------------------------------------------------------ chain txs (docs/RPC.md)

const HDR = 1 + 4 + 32 + 8 + 8 + 1; // version, chain_id, from, nonce, fee, kind

export interface TxFacts {
  jobId?: string; // SubmitResult / CompleteJob / ExpireJob
  resultHash?: string;
  postHashA?: string; // PostJob
  workerPay?: string;
  protocolFee?: string;
  solanaNonce?: string; // BridgeUnlock
  recipient?: string;
  amountSpl?: string;
  jobOrder?: boolean;
  orderJobId?: string;
  solanaRecipient?: string; // BridgeLock
  amount?: string;
}

/** The fields the explorer links on, read from a tx's bytes (base64). Unknown kinds give {}. */
export function txFacts(kind: string, txB64: string): TxFacts {
  const d = Uint8Array.from(Buffer.from(txB64, "base64"));
  if (d.length < HDR) return {};
  try {
    switch (kind) {
      case "SubmitResult":
        return { jobId: hex(slice(d, HDR, 32)), resultHash: hex(slice(d, HDR + 32, 32)) };
      case "CompleteJob":
      case "ExpireJob":
        return { jobId: hex(slice(d, HDR, 32)) };
      case "PostJob": {
        const o = HDR + 12 + 52;
        const uriLen = rd16(d, o + 64);
        const w = o + 66 + uriLen;
        return { postHashA: hex(slice(d, o, 32)), workerPay: rd64(d, w).toString(), protocolFee: rd64(d, w + 8).toString() };
      }
      case "BridgeUnlock": {
        const nonce = rd64(d, HDR);
        const jobOrder = rd8(d, HDR + 48) === 1;
        return {
          solanaNonce: nonce.toString(),
          recipient: pk(d, HDR + 8),
          amountSpl: rd64(d, HDR + 40).toString(),
          jobOrder,
          ...(jobOrder ? { orderJobId: orderJobId(nonce) } : {}),
        };
      }
      case "BridgeLock":
        return { solanaRecipient: pk(d, HDR), amount: rd64(d, HDR + 32).toString() };
      default:
        return {};
    }
  } catch {
    return {};
  }
}

/** Anchor instruction names from a Solana tx's log messages, in order, for one program. */
export function anchorInstructions(logs: string[] | null | undefined, programId: string): string[] {
  const out: string[] = [];
  let depth: string[] = [];
  for (const l of logs ?? []) {
    let m = l.match(/^Program (\w+) invoke \[\d+\]$/);
    if (m) {
      depth.push(m[1]);
      continue;
    }
    m = l.match(/^Program (\w+) (success|failed)/);
    if (m) {
      depth.pop();
      continue;
    }
    m = l.match(/^Program log: Instruction: (\w+)$/);
    if (m && depth[depth.length - 1] === programId) out.push(m[1]);
  }
  return out;
}

export { b58decode };

// ------------------------------------------------------------ storefront buyback (2026-10-18)

export interface StoreBuyback {
  bps: number;
  dest: "reserve" | "burn";
  accrued: string;
  spent: string;
  tokens: string;
  toReserve: string;
  burned: string;
  buys: number;
  pendingOrders: number;
  lastTs: number;
}

/** The buyback tail of the storefront Config, or null on a program without it. */
export function decodeStoreBuyback(d: Uint8Array): StoreBuyback | null {
  if (d.length < 146 || !discIs(d, "Config")) return null;
  const n = rd32(d, 142);
  const at = 146 + 32 * n;
  const b = at + 59;
  if (d.length < at + 230) return null;
  return {
    bps: rd16(d, b),
    dest: d[b + 2] === 1 ? "burn" : "reserve",
    lastTs: Number(rdi64(d, b + 91)),
    accrued: rd64(d, b + 99).toString(),
    spent: rd64(d, b + 107).toString(),
    tokens: rd64(d, b + 115).toString(),
    toReserve: rd64(d, b + 123).toString(),
    burned: rd64(d, b + 131).toString(),
    buys: Number(rd64(d, b + 139)),
    pendingOrders: Number(rd64(d, b + 147)),
  };
}

export const storeConfigPda = () => findPda([utf8("config")], STOREFRONT_ID)[0];
export const buybackVaultPda = () => findPda([utf8("buyback")], STOREFRONT_ID)[0];

export const eventDisc = (name: string) => slice(sha256(utf8(`event:${name}`)), 0, 8);

export interface BuybackExecuted {
  buy: number;
  venue: "curve" | "pumpswap";
  solIn: string;
  tokensOut: string;
  routed: string;
  destination: "reserve" | "burn";
  destinationAccount: string;
  firstNonce: string;
  lastNonce: string;
  orders: number;
  carry: string;
}

export interface BuybackAccrued {
  order: string;
  nonce: string;
  lamports: string;
}

/** Anchor `emit!` events in a tx's logs ("Program data: <base64>"), the two buyback kinds only. */
export function buybackEvents(logs: string[] | null | undefined): { executed: BuybackExecuted[]; accrued: BuybackAccrued[] } {
  const executed: BuybackExecuted[] = [];
  const accrued: BuybackAccrued[] = [];
  const ex = Buffer.from(eventDisc("BuybackExecuted"));
  const ac = Buffer.from(eventDisc("BuybackAccrued"));
  for (const l of logs ?? []) {
    const m = l.match(/^Program data: (.+)$/);
    if (!m) continue;
    const d = Uint8Array.from(Buffer.from(m[1], "base64"));
    if (d.length < 8) continue;
    const disc = Buffer.from(d.subarray(0, 8));
    if (disc.equals(ex) && d.length >= 8 + 8 + 1 + 24 + 1 + 32 + 8 * 11) {
      let o = 8;
      const buy = Number(rd64(d, o)); o += 8;
      const venue = d[o] === 1 ? "pumpswap" : "curve"; o += 1;
      const solIn = rd64(d, o).toString(); o += 8;
      const tokensOut = rd64(d, o).toString(); o += 8;
      const routed = rd64(d, o).toString(); o += 8;
      const destination = d[o] === 1 ? "burn" : "reserve"; o += 1;
      const destinationAccount = pk(d, o); o += 32;
      o += 8; // quote
      const firstNonce = rd64(d, o).toString(); o += 8;
      const lastNonce = rd64(d, o).toString(); o += 8;
      const orders = Number(rd64(d, o)); o += 8;
      o += 8; // available_before
      const carry = rd64(d, o).toString();
      executed.push({ buy, venue, solIn, tokensOut, routed, destination, destinationAccount, firstNonce, lastNonce, orders, carry });
    } else if (disc.equals(ac) && d.length >= 8 + 32 + 16) {
      accrued.push({ order: pk(d, 8), nonce: rd64(d, 40).toString(), lamports: rd64(d, 48).toString() });
    }
  }
  return { executed, accrued };
}
