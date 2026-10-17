// Human-readable decode of what a user is about to sign. Shown in approval popups.

import { b58, equal, fromUtf8, hex } from "../core/bytes";
import { UnsignedTx } from "../chain/tx";
import { RESERVED } from "../chain/messages";
import { ParsedMessage, COMPUTE_BUDGET, SYSTEM_PROGRAM, TOKEN_2022, TOKEN_PROGRAM, ATA_PROGRAM, isWritableIndex } from "../solana/tx";
import { disc } from "../solana/programs";
import { NATIVE_DECIMALS, SOL_DECIMALS, SPL_DECIMALS, TOKEN_SYMBOL, formatUnits, short } from "./constants";

export interface Row {
  label: string;
  value: string;
  full?: string;
}
export interface Section {
  title: string;
  rows: Row[];
}
export interface Summary {
  headline: string;
  sections: Section[];
  warnings: string[];
}

const nat = (v: bigint) => `${formatUnits(v, NATIVE_DECIMALS, 9)} ${TOKEN_SYMBOL}`;
const addr = (b: Uint8Array): Row["value"] => {
  if (equal(b, RESERVED.escrow)) return "bridge escrow";
  if (equal(b, RESERVED.pool)) return "mining pool";
  return short(b58(b), 6);
};

export function summarizeChainTx(t: UnsignedTx, me?: string, expectChainId?: number): Summary {
  const warnings: string[] = [];
  if (me && b58(t.from) !== me) warnings.push(`Signer ${short(b58(t.from))} is not the connected account.`);
  if (expectChainId !== undefined && t.chainId !== expectChainId) warnings.push(`Chain id ${t.chainId} does not match this network (${expectChainId}).`);
  const head: Row[] = [
    { label: "From", value: short(b58(t.from), 6), full: b58(t.from) },
    { label: "Nonce", value: t.nonce.toString() },
    { label: "Network fee", value: nat(t.fee) },
    { label: "Chain id", value: t.chainId.toString() },
  ];
  const b = t.body;
  let headline: string = b.kind;
  const rows: Row[] = [];
  switch (b.kind) {
    case "Transfer":
      headline = `Send ${nat(b.amount)}`;
      rows.push({ label: "To", value: addr(b.to), full: b58(b.to) }, { label: "Amount", value: nat(b.amount) });
      break;
    case "RegisterAgent":
      headline = `Register agent "${b.name}"`;
      rows.push({ label: "Name", value: b.name }, { label: "Profile URI", value: b.uri || "none" });
      break;
    case "PostJob":
      headline = `Post a job paying ${nat(b.workerPay)}`;
      rows.push(
        { label: "Worker pay (escrowed)", value: nat(b.workerPay) },
        { label: "Protocol fee (to pool, not refunded)", value: nat(b.protocolFee) },
        { label: "Shape", value: `${b.m} x ${b.n}, k ${b.k}` },
        { label: "Deadline height", value: b.deadlineHeight.toString() },
        { label: "Max blocks", value: b.maxBlocks.toString() },
        { label: "Evaluators", value: `${b.threshold} of ${b.evaluators.length}` },
        { label: "Data", value: b.dataUri || "none" },
        { label: "hash_a", value: short(hex(b.hashA), 8), full: hex(b.hashA) },
        { label: "hash_b", value: short(hex(b.hashB), 8), full: hex(b.hashB) },
      );
      break;
    case "SubmitResult":
      headline = "Deliver a job result";
      rows.push({ label: "Job", value: short(hex(b.jobId), 8), full: hex(b.jobId) }, { label: "Result hash", value: short(hex(b.resultHash), 8) }, { label: "URI", value: b.uri });
      break;
    case "CompleteJob":
      headline = "Complete a job";
      rows.push({ label: "Job", value: short(hex(b.jobId), 8), full: hex(b.jobId) }, { label: "Evaluator signatures", value: b.sigs.length.toString() });
      break;
    case "ExpireJob":
      headline = "Expire a job";
      rows.push({ label: "Job", value: short(hex(b.jobId), 8), full: hex(b.jobId) });
      break;
    case "BridgeUnlock":
      headline = "Bridge unlock (relayer)";
      rows.push({ label: "Recipient", value: addr(b.recipient) }, { label: "Amount", value: `${formatUnits(b.amountSpl, SPL_DECIMALS)} ${TOKEN_SYMBOL}` }, { label: "Solana nonce", value: b.solanaNonce.toString() });
      break;
    case "BridgeLock":
      headline = `Bridge ${nat(b.amount)} to Solana`;
      rows.push({ label: "Solana recipient", value: short(b58(b.solanaRecipient), 6), full: b58(b.solanaRecipient) }, { label: "Amount", value: nat(b.amount) });
      if (b.amount % 1000n !== 0n) warnings.push("Amount is not a multiple of 1000 base units; the node will refuse it.");
      break;
    case "AdminConfig":
      headline = `Admin config key ${b.key}`;
      rows.push({ label: "Key", value: b.key.toString() }, { label: "Value", value: short(hex(b.value), 12) });
      warnings.push("Admin-only transaction.");
      break;
  }
  return { headline, sections: [{ title: "Action", rows }, { title: "Transaction", rows: head }], warnings };
}

// ---------------------------------------------------------------- Solana

const KNOWN: Record<string, string> = {
  [b58(SYSTEM_PROGRAM)]: "System",
  [b58(TOKEN_PROGRAM)]: "Token",
  [b58(TOKEN_2022)]: "Token-2022",
  [b58(ATA_PROGRAM)]: "Associated token",
  [b58(COMPUTE_BUDGET)]: "Compute budget",
};

const u64At = (d: Uint8Array, o: number) => new DataView(d.buffer, d.byteOffset, d.byteLength).getBigUint64(o, true);
const u32At = (d: Uint8Array, o: number) => new DataView(d.buffer, d.byteOffset, d.byteLength).getUint32(o, true);
const startsWith = (d: Uint8Array, p: Uint8Array) => d.length >= p.length && p.every((x, i) => d[i] === x);

export function summarizeSolanaTx(m: ParsedMessage, opts: { me?: string; bridge?: string; storefront?: string; mintDecimals?: number }): Summary {
  const warnings: string[] = [];
  const keys = m.staticKeys;
  const key = (i: number) => (i < keys.length ? b58(keys[i]) : `lookup #${i - keys.length}`);
  const payer = b58(keys[0]);
  if (opts.me && payer !== opts.me) warnings.push(`Fee payer is ${short(payer)}, not you.`);
  if (m.lookups.length) warnings.push(`Uses ${m.lookups.length} address lookup table(s); some accounts are not shown by address.`);
  const names: Record<string, string> = { ...KNOWN };
  if (opts.bridge) names[opts.bridge] = "Bridge";
  if (opts.storefront) names[opts.storefront] = "Storefront";
  const sections: Section[] = [];
  let headline = "Solana transaction";
  let solOut = 0n;
  m.instructions.forEach((ix, n) => {
    const prog = key(ix.programIdIndex);
    const d = ix.data;
    const rows: Row[] = [];
    let title = `${names[prog] ?? short(prog)} instruction`;
    const acc = (j: number) => key(ix.accounts[j]);
    if (prog === b58(SYSTEM_PROGRAM) && d.length >= 12 && u32At(d, 0) === 2) {
      const lamports = u64At(d, 4);
      title = "Send SOL";
      rows.push({ label: "From", value: short(acc(0), 6) }, { label: "To", value: short(acc(1), 6), full: acc(1) }, { label: "Amount", value: `${formatUnits(lamports, SOL_DECIMALS, 9)} SOL` });
      if (opts.me && acc(0) === opts.me) solOut += lamports;
    } else if ((prog === b58(TOKEN_PROGRAM) || prog === b58(TOKEN_2022)) && d[0] === 12 && d.length >= 10) {
      title = "Send token";
      rows.push({ label: "Mint", value: short(acc(1), 6) }, { label: "To token account", value: short(acc(2), 6) }, { label: "Amount", value: `${formatUnits(u64At(d, 1), d[9], d[9])}` });
    } else if ((prog === b58(TOKEN_PROGRAM) || prog === b58(TOKEN_2022)) && d[0] === 3) {
      title = "Send token (unchecked)";
      rows.push({ label: "To token account", value: short(acc(1), 6) }, { label: "Base units", value: u64At(d, 1).toString() });
    } else if ((prog === b58(TOKEN_PROGRAM) || prog === b58(TOKEN_2022)) && (d[0] === 4 || d[0] === 13)) {
      title = "Approve a delegate";
      rows.push({ label: "Delegate", value: short(acc(d[0] === 4 ? 1 : 2), 6) }, { label: "Base units", value: u64At(d, 1).toString() });
      warnings.push("Grants another account the right to move your tokens.");
    } else if ((prog === b58(TOKEN_PROGRAM) || prog === b58(TOKEN_2022)) && d[0] === 6) {
      title = "Change token account authority";
      warnings.push("Changes who controls a token account.");
    } else if (prog === b58(ATA_PROGRAM)) {
      title = "Create token account";
      rows.push({ label: "Owner", value: short(acc(2), 6) }, { label: "Mint", value: short(acc(3), 6) });
    } else if (prog === b58(COMPUTE_BUDGET)) {
      title = "Compute budget";
      rows.push({ label: "Setting", value: d[0] === 2 ? `limit ${u32At(d, 1).toLocaleString("en-US")} CU` : d[0] === 3 ? `price ${u64At(d, 1)} micro-lamports/CU` : `op ${d[0]}` });
    } else if (opts.bridge && prog === opts.bridge && startsWith(d, disc("deposit"))) {
      title = "Bridge to agent chain";
      headline = "Bridge deposit";
      rows.push({ label: "Amount", value: `${formatUnits(u64At(d, 8), opts.mintDecimals ?? SPL_DECIMALS)} ${TOKEN_SYMBOL}` }, { label: "Agent-chain recipient", value: short(b58(d.subarray(16, 48)), 6), full: b58(d.subarray(16, 48)) });
    } else if (opts.storefront && prog === opts.storefront && startsWith(d, disc("open_order"))) {
      title = "Open storefront order";
      rows.push({ label: "Draft id", value: u64At(d, 8).toString() }, { label: "Agent-chain poster", value: short(b58(d.subarray(16, 48)), 6) }, { label: "Shape", value: `${u32At(d, 48)} x ${u32At(d, 52)}, k ${u32At(d, 56)}` });
    } else if (opts.storefront && prog === opts.storefront && (startsWith(d, disc("buy_job")) || startsWith(d, disc("buy_job_amm")))) {
      title = startsWith(d, disc("buy_job")) ? "Buy job (curve)" : "Buy job (PumpSwap)";
      headline = "Buy an AI job";
      rows.push({ label: "Pay", value: `${formatUnits(u64At(d, 16), SOL_DECIMALS)} SOL` }, { label: "Min tokens out", value: formatUnits(u64At(d, 24), SPL_DECIMALS) }, { label: "Bridge nonce", value: u64At(d, 8).toString() });
    } else {
      const writable = ix.accounts.filter((a) => isWritableIndex(m, a)).length;
      rows.push({ label: "Program", value: short(prog, 6), full: prog }, { label: "Accounts", value: `${ix.accounts.length} (${writable} writable)` }, { label: "Data", value: `${d.length} bytes` });
    }
    sections.push({ title: `${n + 1}. ${title}`, rows });
  });
  if (solOut > 0n && headline === "Solana transaction") headline = `Send ${formatUnits(solOut, SOL_DECIMALS, 9)} SOL`;
  sections.push({
    title: "Transaction",
    rows: [
      { label: "Fee payer", value: short(payer, 6), full: payer },
      { label: "Format", value: m.version === "legacy" ? "legacy" : "v0" },
      { label: "Signers", value: m.header.numRequiredSignatures.toString() },
    ],
  });
  return { headline, sections, warnings };
}

export function summarizeMessage(msg: Uint8Array): Summary {
  let text: string | null = null;
  try {
    const t = fromUtf8(msg);
    if (!/[\u0000-\u0008\u000e-\u001f]/.test(t)) text = t;
  } catch {
    /* binary */
  }
  return {
    headline: "Sign a message",
    sections: [{ title: "Message", rows: [{ label: text !== null ? "Text" : "Bytes (hex)", value: text ?? hex(msg).slice(0, 512) }] }],
    warnings: text === null ? ["The message is not readable text. Only sign it if you trust this site."] : [],
  };
}
