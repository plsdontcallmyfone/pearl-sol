// Contract tx kinds 30-34 (chain/agent-types/src/contracts.rs, docs/CONTRACTS.md): address
// rule, call ABI, fee floor, receipts / events / record, window.pearl JSON form and approval
// wording. Byte layout of the bodies in ./tx; everything here is checked against
// chain/agent-types/vectors/contracts.json (test/contracts.test.ts).

import { Reader, b58, equal, fromB64, fromHex, fromUtf8, hex, key32, u64, utf8 } from "../core/bytes";
import { blake3 } from "../core/crypto";
import { describeFields, encodeAbiFields, encodeCallArgs, hexArg, int, parseCallArgs, shortHex, shortKey, uint } from "./abi";
import { MAX_CODE_SIZE, MAX_CONTRACT_ARGS, MAX_CONTRACT_LABEL, TxBody, UnsignedTx, signingBytes } from "./tx";

export const CONTRACT_KINDS = ["DeployCode", "Instantiate", "Call", "Migrate", "SetContractAdmin"] as const;
export type ContractKind = (typeof CONTRACT_KINDS)[number];
export const isContractKind = (k: string): k is ContractKind => (CONTRACT_KINDS as readonly string[]).includes(k);

const ZERO32 = new Uint8Array(32);
const isZero = (b: Uint8Array) => b.length === 32 && equal(b, ZERO32);

// ---------------------------------------------------------------- addresses and hashes

/** code_hash = blake3(code). */
export const codeHash = (code: Uint8Array) => blake3(code);

/** blake3("pearl-sol:contract:v1" || creator || nonce u64 LE): the Instantiate sender and nonce. */
export const contractAddress = (creator: Uint8Array, nonce: bigint) => blake3(utf8("pearl-sol:contract:v1"), creator, u64(nonce));

/** Topic of a named event (the SDK's `event!`): blake3("pearl-sol:event:v1" || name). */
export const eventTopic = (name: string) => blake3(utf8("pearl-sol:event:v1"), utf8(name));

export { describeFields, encodeAbiField, encodeAbiFields, encodeCallArgs, parseCallArgs } from "./abi";
export type { AbiField } from "./abi";

// ---------------------------------------------------------------- fee floor

/** getParams fields of the contract fee rule (decimal strings on the node), as bigints. */
export interface ContractFeeParams {
  minFee: bigint;
  gasPrice: bigint;
  gasPriceUnit: bigint;
  baseFee: bigint;
  byteFee: bigint;
  /** chain base units per stored byte (code and contract storage), when the node reports it */
  storageByteDeposit?: bigint;
}

/** Reads the fee rule from a getParams result; null when the node does not report all of it. */
export function contractFeeParams(p: Record<string, unknown> | null | undefined): ContractFeeParams | null {
  if (!p) return null;
  const g = (k: string) => {
    const v = p[k];
    return typeof v === "string" || typeof v === "number" ? (/^\d+$/.test(String(v)) ? BigInt(String(v)) : null) : null;
  };
  const minFee = g("min_fee"), gasPrice = g("gas_price"), gasPriceUnit = g("gas_price_unit"), baseFee = g("contract_base_fee"), byteFee = g("contract_byte_fee");
  if (minFee === null || gasPrice === null || gasPriceUnit === null || baseFee === null || byteFee === null || gasPriceUnit === 0n) return null;
  const sd = g("storage_byte_deposit");
  return { minFee, gasPrice, gasPriceUnit, baseFee, byteFee, ...(sd !== null ? { storageByteDeposit: sd } : {}) };
}

/** Gas limit the tx asks for (0 for SetContractAdmin and non-contract kinds). */
export function gasLimitOf(b: TxBody): bigint {
  switch (b.kind) {
    case "DeployCode":
    case "Instantiate":
    case "Call":
    case "Migrate":
      return b.gasLimit;
  }
  return 0n;
}

/** Length of the encoded signed tx (signing bytes + 64-byte sig): the `len` of the fee rule. */
export const encodedTxLen = (t: UnsignedTx) => signingBytes(t).length + 64;

/**
 * `max(min_fee, contract_base_fee + contract_byte_fee x len + ceil(gas_limit x gas_price / gas_price_unit))`
 * (chain/executor/src/contracts.rs contract_fee_floor). `gas` is the part paid back in
 * proportion to unused gas.
 */
export function contractFeeFloor(p: ContractFeeParams, len: number, gasLimit: bigint): { floor: bigint; fixed: bigint; gas: bigint } {
  const gas = (gasLimit * p.gasPrice + p.gasPriceUnit - 1n) / p.gasPriceUnit;
  const fixed = p.baseFee + p.byteFee * BigInt(len);
  const sum = fixed + gas;
  return { floor: sum > p.minFee ? sum : p.minFee, fixed, gas };
}

/** The fee floor of `t` (its fee field does not change its length). */
export const contractFeeFor = (t: UnsignedTx, p: ContractFeeParams) => contractFeeFloor(p, encodedTxLen(t), gasLimitOf(t.body)).floor;

// ---------------------------------------------------------------- receipts, events, record

export const RECEIPT_STATUS = ["ok", "reverted", "out_of_gas", "trapped", "refused"] as const;

export interface ContractReceipt {
  txHash: Uint8Array;
  kind: number;
  status: (typeof RECEIPT_STATUS)[number];
  gasLimit: bigint;
  gasUsed: bigint;
  feeCharged: bigint;
  contract: Uint8Array; // zero for DeployCode
  codeHash: Uint8Array;
  eventCount: number;
  ret: Uint8Array;
  error: string;
}

export function decodeReceipt(b: Uint8Array): ContractReceipt {
  const r = new Reader(b);
  const v = r.u8();
  if (v !== 1) throw new Error(`unsupported receipt version ${v}`);
  const txHash = r.take(32).slice(), kind = r.u8(), st = r.u8();
  const status = RECEIPT_STATUS[st];
  if (!status) throw new Error(`bad receipt status ${st}`);
  const gasLimit = r.u64(), gasUsed = r.u64(), feeCharged = r.u64();
  const contract = r.take(32).slice(), code = r.take(32).slice(), eventCount = r.u16();
  const ret = r.take(r.u16()).slice();
  const error = fromUtf8(r.take(r.u8()));
  r.finish();
  return { txHash, kind, status, gasLimit, gasUsed, feeCharged, contract, codeHash: code, eventCount, ret, error };
}

export interface ContractEvent {
  contract: Uint8Array;
  txHash: Uint8Array;
  index: number;
  topics: Uint8Array[];
  data: Uint8Array;
}

export function decodeEvent(b: Uint8Array): ContractEvent {
  const r = new Reader(b);
  const contract = r.take(32).slice(), txHash = r.take(32).slice(), index = r.u32();
  const n = r.u8();
  if (n > 4) throw new Error("more than 4 topics");
  const topics = Array.from({ length: n }, () => r.take(32).slice());
  const data = r.take(r.u16()).slice();
  r.finish();
  return { contract, txHash, index, topics, data };
}

export interface ContractsRecord {
  height: bigint;
  receiptCount: number;
  receiptsRoot: Uint8Array;
  eventCount: number;
  eventsRoot: Uint8Array;
  gasUsed: bigint;
}

/** The per-block contracts record (version 0x02, 89 bytes), a tx_root leaf before the coinbase receipt. */
export function decodeContractsRecord(b: Uint8Array): ContractsRecord {
  if (b.length !== 89) throw new Error(`contracts record is 89 bytes, got ${b.length}`);
  const r = new Reader(b);
  const v = r.u8();
  if (v !== 2) throw new Error(`unsupported contracts record version ${v}`);
  const x = { height: r.u64(), receiptCount: r.u32(), receiptsRoot: r.take(32).slice(), eventCount: r.u32(), eventsRoot: r.take(32).slice(), gasUsed: r.u64() };
  r.finish();
  return x;
}

/** Leaf value of a receipt, event or record in its tree: blake3 of its bytes. */
export const leafHash = (b: Uint8Array) => blake3(b);

// chain/agent-types/src/merkle.rs: leaf = blake3(0x00 || value), node = blake3(0x01 || l || r),
// an odd node is carried up, empty root = 32 zero bytes.
const mLeaf = (v: Uint8Array) => blake3(Uint8Array.of(0), v);
const mNode = (l: Uint8Array, r: Uint8Array) => blake3(Uint8Array.of(1), l, r);

/** Root over leaf values (tx hashes, record / receipt / event hashes). */
export function merkleRoot(values: Uint8Array[]): Uint8Array {
  if (!values.length) return new Uint8Array(32);
  let level = values.map(mLeaf);
  while (level.length > 1) {
    const next: Uint8Array[] = [];
    for (let i = 0; i < level.length; i += 2) next.push(i + 1 < level.length ? mNode(level[i], level[i + 1]) : level[i]);
    level = next;
  }
  return level[0];
}

/** Checks a path of 33-byte entries (sibling [32] || sibling_is_left u8) for `value` against `root`. */
export function verifyPath(value: Uint8Array, path: Uint8Array, root: Uint8Array): boolean {
  if (path.length % 33 !== 0) return false;
  let h = mLeaf(value);
  for (let o = 0; o < path.length; o += 33) {
    const sib = path.subarray(o, o + 32);
    const left = path[o + 32];
    if (left > 1) return false;
    h = left === 1 ? mNode(sib, h) : mNode(h, sib);
  }
  return equal(h, root);
}

// ---------------------------------------------------------------- window.pearl JSON form

const amount = (v: unknown, what: string): bigint => {
  if (v === undefined || v === null || v === "") return 0n;
  return uint(v, 64, what);
};
const gas = (v: unknown): bigint => {
  if (v === undefined || v === null || v === "") throw new Error("gasLimit is required");
  return uint(v, 64, "gasLimit");
};
const acct = (s: unknown, what: string): Uint8Array => {
  try {
    return key32(String(s ?? ""));
  } catch {
    throw new Error(`${what} must be a base58 account`);
  }
};
/** admin: base58, or null / "" / "none" for no admin (immutable). */
const adminKey = (s: unknown): Uint8Array => (s === null || s === undefined || s === "" || s === "none" ? new Uint8Array(32) : acct(s, "admin"));
const hash32 = (s: unknown, what: string): Uint8Array => {
  const b = hexArg(s, what);
  if (b.length !== 32) throw new Error(`${what} must be 64 hex`);
  return b;
};
const bytesArg = (v: unknown, what: string): Uint8Array => {
  if (v instanceof Uint8Array) return v;
  if (typeof v !== "string") throw new Error(`${what} must be hex or base64`);
  const t = v.trim();
  if (/^(0x)?([0-9a-fA-F]{2})*$/.test(t)) return fromHex(t);
  try {
    return fromB64(t);
  } catch {
    throw new Error(`${what} must be hex or base64`);
  }
};
/** args as hex/base64 bytes, or built from { method, fields } (call) / { fields } (instantiate, migrate). */
function argsOf(t: any, withMethod: boolean): Uint8Array {
  if (t.args !== undefined && t.args !== null && t.args !== "") {
    if (t.method !== undefined || t.fields !== undefined) throw new Error("pass args bytes or method/fields, not both");
    const a = bytesArg(t.args, "args");
    if (a.length > MAX_CONTRACT_ARGS) throw new Error(`args over ${MAX_CONTRACT_ARGS} bytes`);
    return a;
  }
  const fields = t.fields ?? [];
  if (!Array.isArray(fields)) throw new Error("fields must be an array");
  if (withMethod) {
    if (typeof t.method !== "string" || !t.method) throw new Error("method is required (or pass args bytes)");
    return encodeCallArgs(t.method, fields);
  }
  if (t.method !== undefined) throw new Error("instantiate and migrate take fields only, no method");
  const a = encodeAbiFields(fields);
  if (a.length > MAX_CONTRACT_ARGS) throw new Error(`args over ${MAX_CONTRACT_ARGS} bytes`);
  return a;
}

/**
 * A contract transaction in window.pearl JSON form as a TxBody; throws with the reason.
 * Amounts and gas are decimal strings (or numbers); keys base58; hashes hex; code hex or base64.
 */
export function contractBodyFromJson(t: any): TxBody {
  switch (t.kind) {
    case "DeployCode": {
      const code = bytesArg(t.code, "code");
      if (code.length < 1 || code.length > MAX_CODE_SIZE) throw new Error(`code must be 1..${MAX_CODE_SIZE} bytes`);
      return { kind: "DeployCode", gasLimit: gas(t.gasLimit), code };
    }
    case "Instantiate": {
      const label = String(t.label ?? "");
      if (utf8(label).length > MAX_CONTRACT_LABEL) throw new Error(`label over ${MAX_CONTRACT_LABEL} bytes`);
      return { kind: "Instantiate", codeHash: hash32(t.codeHash, "codeHash"), admin: adminKey(t.admin), value: amount(t.value, "value"), gasLimit: gas(t.gasLimit), label, args: argsOf(t, false) };
    }
    case "Call":
      return { kind: "Call", contract: acct(t.contract, "contract"), value: amount(t.value, "value"), gasLimit: gas(t.gasLimit), args: argsOf(t, true) };
    case "Migrate":
      return { kind: "Migrate", contract: acct(t.contract, "contract"), codeHash: hash32(t.codeHash, "codeHash"), gasLimit: gas(t.gasLimit), args: argsOf(t, false) };
    case "SetContractAdmin":
      // making a contract immutable is permanent, so admin must be named (null for none)
      if (!("admin" in t)) throw new Error("admin is required (a base58 account, or null to make the contract immutable)");
      return { kind: "SetContractAdmin", contract: acct(t.contract, "contract"), admin: adminKey(t.admin) };
  }
  throw new Error(`not a contract kind: ${t.kind}`);
}

// ---------------------------------------------------------------- approval wording

export interface ContractRow {
  label: string;
  value: string;
  full?: string;
}

export interface ContractContext {
  /** the connected account, base58 (for "you") */
  me?: string;
  /** fee rule from getParams; null or missing = no breakdown */
  fee?: ContractFeeParams | null;
  /** known contract labels by base58 address (from getContract) */
  labels?: Record<string, string>;
}

const contractName = (a: Uint8Array, ctx: ContractContext) => {
  const l = ctx.labels?.[b58(a)];
  return l ? `${l} (${shortKey(a)})` : shortKey(a);
};

/** Plain words for a contract tx, plus the fee rows (null for other kinds). */
export function summarizeContract(
  t: UnsignedTx,
  nat: (v: bigint) => string,
  ctx: ContractContext = {},
): { headline: string; rows: ContractRow[]; feeRows: ContractRow[]; warnings: string[] } | null {
  const b = t.body;
  if (!isContractKind(b.kind)) return null;
  const rows: ContractRow[] = [];
  const warnings: string[] = [];
  const me = ctx.me ?? b58(t.from);
  const who = (k: Uint8Array) => (isZero(k) ? "none (immutable)" : b58(k) === me ? "you" : shortKey(k));
  const gasText = (g: bigint) => `up to ${int(g)} gas`;
  let headline = b.kind as string;
  switch (b.kind) {
    case "DeployCode": {
      const h = codeHash(b.code);
      headline = `Deploy contract code (${int(BigInt(b.code.length))} bytes, hash ${shortHex(h)})`;
      rows.push({ label: "Code size", value: `${int(BigInt(b.code.length))} bytes` }, { label: "Code hash", value: shortHex(h, 16), full: hex(h) }, { label: "Gas limit", value: gasText(b.gasLimit) });
      if (ctx.fee?.storageByteDeposit !== undefined) rows.push({ label: "Code storage deposit", value: `${nat(BigInt(b.code.length) * ctx.fee.storageByteDeposit)} from your balance` });
      break;
    }
    case "Instantiate": {
      const addr = contractAddress(t.from, t.nonce);
      const label = b.label ? `'${b.label}'` : "(no label)";
      headline = `Create contract ${label} from code ${shortHex(b.codeHash)}, admin: ${who(b.admin)}`;
      if (b.value > 0n) headline += `, sending ${nat(b.value)}`;
      rows.push(
        { label: "Label", value: b.label || "none" },
        { label: "Code", value: shortHex(b.codeHash, 16), full: hex(b.codeHash) },
        { label: "Admin", value: who(b.admin), ...(isZero(b.admin) ? {} : { full: b58(b.admin) }) },
        { label: "New contract address", value: shortKey(addr), full: b58(addr) },
        { label: "Init arguments", value: describeFields(b.args).text, ...(b.args.length ? { full: hex(b.args) } : {}) },
        { label: "Sends", value: b.value > 0n ? nat(b.value) : "nothing" },
        { label: "Gas limit", value: gasText(b.gasLimit) },
      );
      if (isZero(b.admin)) warnings.push("No admin: nobody will ever be able to migrate this contract's code.");
      else if (b58(b.admin) !== me) warnings.push(`Admin ${shortKey(b.admin)} (not you) will be able to replace this contract's code.`);
      break;
    }
    case "Call": {
      const p = parseCallArgs(b.args);
      const target = contractName(b.contract, ctx);
      if (p) {
        const f = describeFields(p.fields);
        headline = `Call ${p.method} on contract ${target}${p.fields.length ? ` with ${f.text}` : ""}`;
        rows.push({ label: "Method", value: p.method }, { label: "Arguments", value: f.text, ...(p.fields.length ? { full: hex(p.fields) } : {}) });
      } else {
        headline = `Call contract ${target} with raw input`;
        rows.push({ label: "Input (hex)", value: b.args.length ? shortHex(b.args, 24) : "empty", full: hex(b.args) });
      }
      if (b.value > 0n) headline += `, sending ${nat(b.value)}`;
      headline += `, gas up to ${int(b.gasLimit)}`;
      rows.unshift({ label: "Contract", value: target, full: b58(b.contract) });
      rows.push({ label: "Sends", value: b.value > 0n ? nat(b.value) : "nothing" }, { label: "Gas limit", value: gasText(b.gasLimit) });
      break;
    }
    case "Migrate": {
      const target = contractName(b.contract, ctx);
      headline = `Migrate contract ${target} to code ${shortHex(b.codeHash)} (admin action)`;
      rows.push(
        { label: "Contract", value: target, full: b58(b.contract) },
        { label: "New code", value: shortHex(b.codeHash, 16), full: hex(b.codeHash) },
        { label: "Migrate arguments", value: describeFields(b.args).text, ...(b.args.length ? { full: hex(b.args) } : {}) },
        { label: "Gas limit", value: gasText(b.gasLimit) },
      );
      warnings.push("Admin action: this replaces the contract's code for everyone who uses it. Only the contract's admin can do it; from any other account it fails and the fee is still charged.");
      break;
    }
    case "SetContractAdmin": {
      const target = contractName(b.contract, ctx);
      if (isZero(b.admin)) {
        headline = `Make contract ${target} immutable`;
        warnings.push("Permanent: after this nobody, you included, can ever migrate this contract's code.");
      } else {
        headline = `Hand contract ${target}'s admin to ${who(b.admin)}`;
        if (b58(b.admin) !== me) warnings.push(`${shortKey(b.admin)} will control this contract's code (migrate and admin changes); you will not.`);
      }
      rows.push({ label: "Contract", value: target, full: b58(b.contract) }, { label: "New admin", value: who(b.admin), ...(isZero(b.admin) ? {} : { full: b58(b.admin) }) });
      break;
    }
  }
  if (b.kind === "Instantiate" || b.kind === "Call") {
    if (b.value > 0n) warnings.push(`Only the contract's code decides whether the ${nat(b.value)} sent to it ever comes back.`);
  }
  if (b.kind !== "DeployCode" && b.kind !== "SetContractAdmin" && ctx.fee?.storageByteDeposit !== undefined) {
    rows.push({ label: "Storage deposit", value: `${nat(ctx.fee.storageByteDeposit)} per byte of storage added, refunded when freed` });
  }

  const feeRows: ContractRow[] = [];
  const g = gasLimitOf(b);
  if (ctx.fee) {
    const len = encodedTxLen(t);
    const f = contractFeeFloor(ctx.fee, len, g);
    feeRows.push({ label: "Network fee (most you pay)", value: nat(t.fee) });
    if (f.fixed + f.gas >= ctx.fee.minFee) {
      feeRows.push({ label: "Base and size", value: `${nat(f.fixed)} (${int(BigInt(len))} bytes)` });
      if (g > 0n) feeRows.push({ label: "Gas", value: `up to ${nat(f.gas)}, unused gas is refunded` });
    } else {
      feeRows.push({ label: "Floor", value: `${nat(ctx.fee.minFee)} (the chain minimum fee)` });
    }
    if (t.fee < f.floor) warnings.push(`Fee ${nat(t.fee)} is below this transaction's floor of ${nat(f.floor)}; the node will refuse it.`);
    else if (t.fee > f.floor) feeRows.push({ label: "Above the floor", value: `${nat(t.fee - f.floor)} (not refunded)` });
  } else {
    feeRows.push({ label: "Network fee (most you pay)", value: g > 0n ? `${nat(t.fee)}, the gas part of it is refunded for unused gas` : nat(t.fee) });
  }
  if (b.kind !== "SetContractAdmin") feeRows.push({ label: "If it fails", value: "the fee is still charged and the contract's changes are undone" });
  return { headline, rows, feeRows, warnings };
}

/** One activity line for a contract tx this account sent (labels from getContract when known). */
export function contractActivity(
  t: UnsignedTx,
  me: string,
  labels: Record<string, string> = {},
): { summary: string; amount?: string; counterparty?: string; flow: "in" | "out" | "none" } | null {
  const b = t.body;
  const name = (a: Uint8Array) => contractName(a, { labels });
  const mine = b58(t.from) === me;
  switch (b.kind) {
    case "DeployCode":
      return { summary: `Deployed contract code (${int(BigInt(b.code.length))} bytes, hash ${shortHex(codeHash(b.code))})`, flow: "none" };
    case "Instantiate": {
      const addr = b58(contractAddress(t.from, t.nonce));
      return { summary: `Created contract ${b.label ? `'${b.label}'` : addr.slice(0, 4) + "..." + addr.slice(-3)}`, counterparty: addr, ...(b.value > 0n ? { amount: b.value.toString() } : {}), flow: b.value > 0n && mine ? "out" : "none" };
    }
    case "Call": {
      const p = parseCallArgs(b.args);
      return {
        summary: p ? `Called ${p.method} on ${name(b.contract)}` : `Called contract ${name(b.contract)}`,
        counterparty: b58(b.contract),
        ...(b.value > 0n ? { amount: b.value.toString() } : {}),
        flow: b.value > 0n && mine ? "out" : "none",
      };
    }
    case "Migrate":
      return { summary: `Migrated contract ${name(b.contract)} to code ${shortHex(b.codeHash)}`, counterparty: b58(b.contract), flow: "none" };
    case "SetContractAdmin":
      return {
        summary: isZero(b.admin) ? `Made contract ${name(b.contract)} immutable` : `Handed contract ${name(b.contract)}'s admin to ${b58(b.admin) === me ? "you" : shortKey(b.admin)}`,
        counterparty: b58(b.contract),
        flow: "none",
      };
  }
  return null;
}

/** Extra warnings when a site (window.pearl) asks for a contract tx. */
export function dappContractWarnings(b: TxBody, nat: (v: bigint) => string): string[] {
  const w: string[] = [];
  if ((b.kind === "Call" || b.kind === "Instantiate") && b.value > 0n) w.push(`This site asks you to send ${nat(b.value)} into a contract.`);
  if (b.kind === "Migrate") w.push(`This site asks for an admin action: replacing the code of contract ${shortKey(b.contract)}. Approve only if you run this contract and expect this upgrade.`);
  if (b.kind === "SetContractAdmin")
    w.push(
      isZero(b.admin)
        ? `This site asks you to make contract ${shortKey(b.contract)} immutable forever.`
        : `This site asks you to hand control of contract ${shortKey(b.contract)} to ${shortKey(b.admin)}.`,
    );
  return w;
}
