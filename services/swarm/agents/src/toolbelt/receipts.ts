// Tool receipts (docs/TOOLBELT.md 5): one per tool call, committed on Cadre in batches.
//
// A receipt says which agent called which tool, the sha256 of the canonical inputs and of the
// outputs, what the call cost outside (USD) and what the agent paid (lamports of its prepaid
// toolbelt balance), and the thought_ref (the gateway request id of the paid thought that decided
// the call). A batch is the receipts of seq first..first+n-1 of one agent; its Merkle root is
// committed on the Cadre chain by the agent's own key (contracts/toolbelt `commit`), and the batch
// itself (every receipt) is stored as a Cadre blob whose blake3 id is committed with the root, so
// anyone can fetch the blob, recompute every leaf and the root, and compare with the chain.
//
// What a receipt proves and what it does not: see docs/TOOLBELT.md 6 (it proves payment, the
// decision and the hash of what was delivered; it does not prove that a page said something).
import { createHash } from "node:crypto";
import { canonical } from "../spec.ts";

export const RECEIPT_FORMAT = "cadre-tool-receipt-v1";
export const BATCH_FORMAT = "cadre-tool-batch-v1";

export interface ToolReceipt {
  format: typeof RECEIPT_FORMAT;
  agent: string; // base58
  seq: number; // 0, 1, 2, ... per agent, no gaps
  tool: string; // e.g. web.search
  capability: string;
  inputs_sha256: string;
  outputs_sha256: string;
  ok: boolean;
  error: string | null;
  cost_usd: string; // decimal text, the outside cost this call caused (provider-reported or list x measured)
  cost_basis: string; // where the cost figure comes from
  price_lamports: string; // what the agent paid from its toolbelt balance (2x cost, docs/TOOLBELT.md 4)
  thought_ref: string | null; // the paid thought (gateway request id) that decided the call
  decided: string; // "thought" or the fixed rule that picked the call after a thought (e.g. "rule: top 3 results")
  task: string | null; // the task this call served (e.g. "hire:12", "watch")
  artifacts: string[]; // blob URIs this call stored (files)
  started_ms: number;
  ms: number;
}

export const sha256hex = (b: string | Uint8Array) => createHash("sha256").update(b).digest("hex");
export const hashJson = (v: unknown) => sha256hex(canonical(v ?? null));

/** leaf = sha256(0x00 || canonical receipt), node = sha256(0x01 || left || right); an odd last node is promoted. */
export function leafOf(r: ToolReceipt): Buffer {
  return createHash("sha256").update(Buffer.from([0])).update(canonical(r), "utf8").digest();
}

export function merkleRoot(leaves: Buffer[]): Buffer {
  if (!leaves.length) return Buffer.alloc(32);
  let level = leaves;
  while (level.length > 1) {
    const next: Buffer[] = [];
    for (let i = 0; i < level.length; i += 2) {
      if (i + 1 === level.length) next.push(level[i]);
      else next.push(createHash("sha256").update(Buffer.from([1])).update(level[i]).update(level[i + 1]).digest());
    }
    level = next;
  }
  return level[0];
}

/** The sibling path of leaf `i` (for a buyer who wants to check one receipt without the rest). */
export function merkleProof(leaves: Buffer[], i: number): { side: "L" | "R"; hash: string }[] {
  const out: { side: "L" | "R"; hash: string }[] = [];
  let level = leaves;
  let idx = i;
  while (level.length > 1) {
    const sib = idx % 2 === 0 ? idx + 1 : idx - 1;
    if (sib < level.length) out.push({ side: idx % 2 === 0 ? "R" : "L", hash: level[sib].toString("hex") });
    const next: Buffer[] = [];
    for (let j = 0; j < level.length; j += 2) {
      if (j + 1 === level.length) next.push(level[j]);
      else next.push(createHash("sha256").update(Buffer.from([1])).update(level[j]).update(level[j + 1]).digest());
    }
    level = next;
    idx = Math.floor(idx / 2);
  }
  return out;
}

export function verifyProof(leaf: Buffer, proof: { side: "L" | "R"; hash: string }[], root: string): boolean {
  let h = leaf;
  for (const p of proof) {
    const s = Buffer.from(p.hash, "hex");
    h = createHash("sha256").update(Buffer.from([1])).update(p.side === "R" ? h : s).update(p.side === "R" ? s : h).digest();
  }
  return h.toString("hex") === root;
}

export interface ToolBatch {
  format: typeof BATCH_FORMAT;
  agent: string;
  first_seq: number;
  n: number;
  root: string; // hex
  price_lamports: string; // sum over the batch
  cost_usd: string;
  receipts: ToolReceipt[];
  limits: string;
}

export const BATCH_LIMITS = "Receipts prove what the agent paid, which paid thought decided each call, and the sha256 of each call's inputs and outputs. They do not prove what a web page said, that a sandbox ran honestly, or that the work is good: buyers and market judges decide that (docs/TOOLBELT.md 6).";

export function makeBatch(agent: string, receipts: ToolReceipt[]): ToolBatch {
  if (!receipts.length) throw new Error("empty batch");
  receipts.forEach((r, i) => { if (r.seq !== receipts[0].seq + i || r.agent !== agent) throw new Error("batch receipts must be one agent's consecutive seqs"); });
  const usd = receipts.reduce((s, r) => s + Number(r.cost_usd), 0);
  return {
    format: BATCH_FORMAT, agent, first_seq: receipts[0].seq, n: receipts.length, root: merkleRoot(receipts.map(leafOf)).toString("hex"),
    price_lamports: receipts.reduce((s, r) => s + BigInt(r.price_lamports), 0n).toString(), cost_usd: usd.toFixed(9), receipts, limits: BATCH_LIMITS,
  };
}

/** Recompute a batch from its receipts; the problems found (empty = consistent). */
export function checkBatch(b: ToolBatch): string[] {
  const e: string[] = [];
  if (b?.format !== BATCH_FORMAT) return ["not a cadre-tool-batch-v1"];
  if (b.receipts.length !== b.n) e.push("n does not match the receipts");
  b.receipts.forEach((r, i) => {
    if (r.seq !== b.first_seq + i) e.push(`receipt ${i} has seq ${r.seq}, expected ${b.first_seq + i}`);
    if (r.agent !== b.agent) e.push(`receipt ${i} names another agent`);
  });
  const root = merkleRoot(b.receipts.map(leafOf)).toString("hex");
  if (root !== b.root) e.push(`root is ${root}, the batch says ${b.root}`);
  const total = b.receipts.reduce((s, r) => s + BigInt(r.price_lamports), 0n).toString();
  if (total !== b.price_lamports) e.push(`price sum is ${total}, the batch says ${b.price_lamports}`);
  return e;
}
