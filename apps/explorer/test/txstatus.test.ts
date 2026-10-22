// Tx status normalization and the tx page status cell (docs/SPEED.md 2.4/2.5).
import { test } from "node:test";
import assert from "node:assert/strict";
import { confidenceOf, normalizeTxStatus, parsePreconf, statusLabel } from "../src/txstatus.ts";
import { txStatusCell } from "../src/txstatus-view.ts";
import { txModel } from "../src/contract-route.ts";
import { txPage } from "../src/contract-view.ts";

const H = "ab".repeat(32);
const dash = String.fromCharCode(0x2014);

test("pending, preconfirmed, included, final from a new node", () => {
  assert.equal(normalizeTxStatus({ tx_hash: H, status: "pending" })!.status, "pending");
  const p = normalizeTxStatus({ tx_hash: H, status: "preconfirmed", preconf: { weak_count: 3, weak_miners: 2, weak_share: 0.75, first_seen_ms: 1000, latest_age_ms: 1200, confidence: "medium" } })!;
  assert.equal(p.status, "preconfirmed");
  assert.equal(p.preconf!.weak_count, 3);
  assert.equal(p.preconf!.confidence, "medium");
  assert.match(p.risk, /Not binding/);
  assert.equal(p.height, null);
  const inc = normalizeTxStatus({ status: "included", height: 10, confirmations: 3, final: false, block_hash: "cd".repeat(32) }, { finality_k: 30, finalized_height: 50 })!;
  assert.equal(inc.status, "included", "node says final: false, trust it over the finalized height");
  assert.equal(inc.confirmations, 3);
  const fin = normalizeTxStatus({ status: "final", height: 10, confirmations: 40, final: true })!;
  assert.equal(fin.status, "final");
  assert.equal(fin.final_source, "node");
});

test("an old node: final derived from the finalized height or K", () => {
  const a = normalizeTxStatus({ status: "included", height: 10, confirmations: 31 }, { finalized_height: 20, finality_k: 30 })!;
  assert.equal(a.status, "final");
  assert.equal(a.final_source, "derived");
  const b = normalizeTxStatus({ status: "included", height: 10, confirmations: 31 }, { finality_k: 30 })!;
  assert.equal(b.status, "final", "confirmations > K");
  const c = normalizeTxStatus({ status: "included", height: 10, confirmations: 30 }, { finality_k: 30 })!;
  assert.equal(c.status, "included", "exactly K confirmations is not yet final");
  const d = normalizeTxStatus({ status: "included", height: 10, confirmations: 2 }, { finalized_height: 5, finality_k: 30 })!;
  assert.equal(d.status, "included");
  assert.equal(d.final, false);
  assert.equal(normalizeTxStatus(null), null);
});

test("preconf parsing: missing or bad fields, confidence from counts", () => {
  assert.equal(parsePreconf(null), null);
  assert.equal(parsePreconf({ weak_count: 0 }), null);
  assert.equal(confidenceOf(1, 1, 1), "low");
  assert.equal(confidenceOf(2, 2, 0.5), "medium");
  assert.equal(confidenceOf(4, 2, 0.95), "high");
  assert.equal(confidenceOf(4, 1, 1), "low", "one miner is never more than low");
  const p = parsePreconf({ weak_count: "5", weak_miners: 3, weak_share: 1.7, confidence: "bogus" })!;
  assert.equal(p.weak_share, 1);
  assert.equal(p.confidence, "high");
  // a status of pending with preconf evidence is shown as preconfirmed
  assert.equal(normalizeTxStatus({ status: "pending", preconf: { weak_count: 1 } })!.status, "preconfirmed");
  assert.equal(normalizeTxStatus({ status: "preconfirmed" })!.preconf, null);
});

test("labels and the status cell, no em dash", () => {
  const p = normalizeTxStatus({ status: "preconfirmed", preconf: { weak_count: 1, weak_miners: 1, weak_share: 1, latest_age_ms: 900 } })!;
  assert.equal(statusLabel(p), "preconfirmed (1 weak block, 1 miner, low, latest 0.9 s ago)");
  const cells = [
    txStatusCell(normalizeTxStatus({ status: "pending" })),
    txStatusCell(p),
    txStatusCell(normalizeTxStatus({ status: "included", height: 7, confirmations: 2, finality_k: 30 })),
    txStatusCell(normalizeTxStatus({ status: "final", height: 7, final: true }, { finality_k: 60 })),
  ];
  assert.match(cells[0], /pending in the mempool/);
  assert.match(cells[1], /preconfirmed, low confidence/);
  assert.match(cells[1], /1 weak block from 1 miner, 100% share, latest 0\.9 s ago/);
  assert.match(cells[2], /2 of 30 confirmations/);
  assert.match(cells[3], /cannot be reorged/);
  for (const c of cells) assert.ok(!c.includes(dash));
});

test("tx page model carries the normalized status from the node and the indexer's chain info", async () => {
  const ix: any = {
    chainInfo: { height: 100, finalized_height: 70, finality_k: 30 },
    node: { call: async (m: string) => (m === "getTx" ? { tx_hash: H, kind: "Transfer", from: "1".repeat(32), nonce: 0, fee: "1000", status: "included", height: 60, confirmations: 41 } : null) },
  };
  const m: any = await txModel(ix, H);
  assert.equal(m.tx_status.status, "final");
  const sm: any = { network: "devnet", height: 100 };
  const html = txPage(sm, m);
  assert.match(html, /cannot be reorged/);
  const ix2: any = { chainInfo: null, node: { call: async () => ({ tx_hash: H, kind: "Transfer", from: "1".repeat(32), nonce: 0, fee: "1000", status: "preconfirmed", preconf: { weak_count: 2, weak_miners: 2, weak_share: 1, latest_age_ms: 500, confidence: "medium" } }) } };
  const m2: any = await txModel(ix2, H);
  assert.equal(m2.tx_status.status, "preconfirmed");
  assert.equal(m2.tx_status.preconf.confidence, "medium");
});
