// Minimal Solana transaction layer: PDAs, legacy message compile, and a parser for
// legacy and v0 messages (for the approval summary). No @solana/web3.js at runtime;
// test/solana.test.ts checks the output against web3.js byte for byte.

import { Reader, b58, concat, equal, fromB58, u8, utf8 } from "../core/bytes";
import { isOnCurve, sha256, signRaw } from "../core/crypto";

export type Pubkey = Uint8Array;
export const pk = (s: string): Pubkey => {
  const b = fromB58(s);
  if (b.length !== 32) throw new Error(`not a 32-byte address: ${s}`);
  return b;
};

export const SYSTEM_PROGRAM = pk("11111111111111111111111111111111");
export const TOKEN_PROGRAM = pk("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const TOKEN_2022 = pk("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const ATA_PROGRAM = pk("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
export const COMPUTE_BUDGET = pk("ComputeBudget111111111111111111111111111111");
export const IX_SYSVAR = pk("Sysvar1nstructions1111111111111111111111111");
export const WSOL = pk("So11111111111111111111111111111111111111112");

export interface AccountMeta {
  pubkey: Pubkey;
  isSigner: boolean;
  isWritable: boolean;
}
export interface Instruction {
  programId: Pubkey;
  keys: AccountMeta[];
  data: Uint8Array;
}

export const meta = (pubkey: Pubkey, isWritable = false, isSigner = false): AccountMeta => ({ pubkey, isWritable, isSigner });

// ---------------------------------------------------------------- PDAs

export function createProgramAddress(seeds: Uint8Array[], programId: Pubkey): Pubkey {
  for (const s of seeds) if (s.length > 32) throw new Error("seed over 32 bytes");
  const h = sha256(...seeds, programId, utf8("ProgramDerivedAddress"));
  if (isOnCurve(h)) throw new Error("on curve");
  return h;
}

export function findPda(seeds: Uint8Array[], programId: Pubkey): [Pubkey, number] {
  for (let bump = 255; bump >= 0; bump--) {
    try {
      return [createProgramAddress([...seeds, u8(bump)], programId), bump];
    } catch {
      /* next bump */
    }
  }
  throw new Error("no viable bump");
}
export const pda = (seeds: Uint8Array[], programId: Pubkey) => findPda(seeds, programId)[0];

export const ata = (owner: Pubkey, mint: Pubkey, tokenProgram: Pubkey) => pda([owner, tokenProgram, mint], ATA_PROGRAM);

// ---------------------------------------------------------------- compact-u16

export function shortvec(n: number): Uint8Array {
  const out: number[] = [];
  let v = n;
  for (;;) {
    let e = v & 0x7f;
    v >>= 7;
    if (v === 0) {
      out.push(e);
      break;
    }
    e |= 0x80;
    out.push(e);
  }
  return Uint8Array.from(out);
}

function readShortvec(r: Reader): number {
  let len = 0;
  let size = 0;
  for (;;) {
    const e = r.u8();
    len |= (e & 0x7f) << (size * 7);
    size++;
    if ((e & 0x80) === 0) break;
    if (size > 3) throw new Error("bad shortvec");
  }
  return len;
}

// ---------------------------------------------------------------- compile (legacy)

export interface CompiledMessage {
  header: { numRequiredSignatures: number; numReadonlySigned: number; numReadonlyUnsigned: number };
  accountKeys: Pubkey[];
  recentBlockhash: Uint8Array;
  instructions: { programIdIndex: number; accounts: number[]; data: Uint8Array }[];
}

/** Same ordering rules as web3.js `Transaction.compileMessage`: payer first, then signer-writable, signer-readonly, writable, readonly (first-seen order within a class). */
export function compileLegacy(ixs: Instruction[], payer: Pubkey, recentBlockhash: Uint8Array): CompiledMessage {
  type Entry = { key: Pubkey; s: boolean; w: boolean };
  const entries: Entry[] = [];
  const find = (k: Pubkey) => entries.find((e) => equal(e.key, k));
  const add = (k: Pubkey, s: boolean, w: boolean) => {
    const e = find(k);
    if (e) {
      e.s ||= s;
      e.w ||= w;
    } else entries.push({ key: k, s, w });
  };
  for (const ix of ixs) {
    for (const m of ix.keys) add(m.pubkey, m.isSigner, m.isWritable);
    add(ix.programId, false, false);
  }
  // web3.js sorts: signers first, then writable, then by base58 string... (see test). We mirror it exactly.
  const sorted = entries.slice().sort((x, y) => {
    if (x.s !== y.s) return x.s ? -1 : 1;
    if (x.w !== y.w) return x.w ? -1 : 1;
    return b58(x.key).localeCompare(b58(y.key), "en", { localeMatcher: "best fit", usage: "sort", sensitivity: "variant", ignorePunctuation: false, numeric: false, caseFirst: "lower" });
  });
  const payerIdx = sorted.findIndex((e) => equal(e.key, payer));
  let payerEntry: Entry;
  if (payerIdx >= 0) {
    payerEntry = sorted.splice(payerIdx, 1)[0];
    payerEntry.s = true;
    payerEntry.w = true;
  } else payerEntry = { key: payer, s: true, w: true };
  const keys = [payerEntry, ...sorted];
  const numRequiredSignatures = keys.filter((e) => e.s).length;
  const numReadonlySigned = keys.filter((e) => e.s && !e.w).length;
  const numReadonlyUnsigned = keys.filter((e) => !e.s && !e.w).length;
  const index = (k: Pubkey) => keys.findIndex((e) => equal(e.key, k));
  return {
    header: { numRequiredSignatures, numReadonlySigned, numReadonlyUnsigned },
    accountKeys: keys.map((e) => e.key),
    recentBlockhash,
    instructions: ixs.map((ix) => ({ programIdIndex: index(ix.programId), accounts: ix.keys.map((m) => index(m.pubkey)), data: ix.data })),
  };
}

export function serializeMessage(m: CompiledMessage): Uint8Array {
  return concat(
    Uint8Array.of(m.header.numRequiredSignatures, m.header.numReadonlySigned, m.header.numReadonlyUnsigned),
    shortvec(m.accountKeys.length), ...m.accountKeys,
    m.recentBlockhash,
    shortvec(m.instructions.length),
    ...m.instructions.map((ix) => concat(u8(ix.programIdIndex), shortvec(ix.accounts.length), Uint8Array.from(ix.accounts), shortvec(ix.data.length), ix.data)),
  );
}

export function serializeTransaction(sigs: Uint8Array[], message: Uint8Array): Uint8Array {
  return concat(shortvec(sigs.length), ...sigs, message);
}

/** Build, sign (single signer = payer) and serialize a legacy tx. */
export function buildSigned(ixs: Instruction[], payerSeed: Uint8Array, payer: Pubkey, blockhash: string) {
  const msg = serializeMessage(compileLegacy(ixs, payer, fromB58(blockhash)));
  const sig = signRaw(payerSeed, msg);
  return { tx: serializeTransaction([sig], msg), signature: b58(sig), message: msg };
}

// ---------------------------------------------------------------- parse (legacy + v0)

export interface ParsedMessage {
  version: "legacy" | 0;
  header: CompiledMessage["header"];
  staticKeys: Pubkey[];
  recentBlockhash: Uint8Array;
  instructions: { programIdIndex: number; accounts: number[]; data: Uint8Array }[];
  lookups: { table: Pubkey; writable: number[]; readonly: number[] }[];
}

export function parseMessage(b: Uint8Array): ParsedMessage {
  const r = new Reader(b);
  let version: "legacy" | 0 = "legacy";
  const first = b[0];
  if (first & 0x80) {
    const v = first & 0x7f;
    if (v !== 0) throw new Error(`unsupported message version ${v}`);
    version = 0;
    r.u8();
  }
  const header = { numRequiredSignatures: r.u8(), numReadonlySigned: r.u8(), numReadonlyUnsigned: r.u8() };
  const nk = readShortvec(r);
  const staticKeys = Array.from({ length: nk }, () => r.take(32).slice());
  const recentBlockhash = r.take(32).slice();
  const ni = readShortvec(r);
  const instructions = Array.from({ length: ni }, () => {
    const programIdIndex = r.u8();
    const na = readShortvec(r);
    const accounts = Array.from(r.take(na));
    const nd = readShortvec(r);
    return { programIdIndex, accounts, data: r.take(nd).slice() };
  });
  const lookups: ParsedMessage["lookups"] = [];
  if (version === 0) {
    const nl = readShortvec(r);
    for (let i = 0; i < nl; i++) {
      const table = r.take(32).slice();
      const writable = Array.from(r.take(readShortvec(r)));
      const readonly = Array.from(r.take(readShortvec(r)));
      lookups.push({ table, writable, readonly });
    }
  }
  r.finish();
  return { version, header, staticKeys, recentBlockhash, instructions, lookups };
}

export function parseTransaction(b: Uint8Array): { signatures: Uint8Array[]; message: Uint8Array; parsed: ParsedMessage } {
  const r = new Reader(b);
  const ns = readShortvec(r);
  const signatures = Array.from({ length: ns }, () => r.take(64).slice());
  const message = b.subarray(r.o);
  return { signatures, message, parsed: parseMessage(message) };
}

/** Put `sig` at the slot of `signer` in a serialized tx (dApp signTransaction). */
export function insertSignature(txBytes: Uint8Array, signer: Pubkey, sig: Uint8Array): Uint8Array {
  const { signatures, message, parsed } = parseTransaction(txBytes);
  const idx = parsed.staticKeys.findIndex((k) => equal(k, signer));
  if (idx < 0 || idx >= parsed.header.numRequiredSignatures) throw new Error("this account is not a required signer of the transaction");
  const sigs = signatures.length === parsed.header.numRequiredSignatures
    ? signatures.map((s) => s.slice())
    : Array.from({ length: parsed.header.numRequiredSignatures }, () => new Uint8Array(64));
  sigs[idx] = sig;
  return serializeTransaction(sigs, message);
}

export const isWritableIndex = (m: ParsedMessage, i: number) => {
  const { numRequiredSignatures: s, numReadonlySigned: rs, numReadonlyUnsigned: ru } = m.header;
  if (i < s) return i < s - rs;
  if (i < m.staticKeys.length) return i < m.staticKeys.length - ru;
  return false;
};
