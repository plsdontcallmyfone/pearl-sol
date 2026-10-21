// Contract pages and their /api twins (docs/CONTRACTS.md; API side in docs/API.md "Contracts"): /contracts, /contract/{address},
// /code/{hash}, /tx/{hash}. Every figure is a node read at query time (listContracts, listCodes,
// getContract, getContractEvents, getCode, getTx with its receipt) or the codec in contracts.ts
// over those bytes. A node without the contract RPC shows "Unavailable" with the reason.

import type { Indexer } from "./indexer.ts";
import type { Summary } from "./models.ts";
import { KNOWN_EVENTS, decodeContractTx, eventTopic, isContractKind, isUnknownMethod, splitMethod, withTopicNames } from "./contracts.ts";
import { codePage, contractPage, contractsPage, txPage } from "./contract-view.ts";
import { notFound } from "./views.ts";
import { normalizeTxStatus } from "./txstatus.ts";

type Res = { status: number; type: string; body: string | Buffer; headers?: Record<string, string> };
const json = (v: unknown, status = 200): Res => ({ status, type: "application/json; charset=utf-8", body: JSON.stringify(v, null, 1) });
const html = (body: string, status = 200): Res => ({ status, type: "text/html; charset=utf-8", body });

export type Read<T = any> = { ok: true; v: T } | { ok: false; reason: string };

const B58 = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;
const HEX64 = /^[0-9a-fA-F]{64}$/;
const PAGE = 25;

/** One node read; a missing method or a node error becomes the reason shown as "Unavailable". */
export async function read<T = any>(ix: Indexer, method: string, params: unknown): Promise<Read<T>> {
  try {
    return { ok: true, v: await ix.node.call<T>(method, params) };
  } catch (e) {
    const m = (e as Error).message ?? String(e);
    if (isUnknownMethod(e)) return { ok: false, reason: `this node does not serve ${method}: the contracts upgrade is not deployed on it` };
    return { ok: false, reason: `the node did not answer ${method}: ${m.slice(0, 200)}` };
  }
}

const intq = (q: URLSearchParams, k: string, d: number, max: number) => {
  const v = Number(q.get(k) ?? d);
  return Number.isInteger(v) && v >= 0 ? Math.min(v, max) : d;
};

/** Contract params from the indexer's getParams; null fields when the node predates contracts. */
export function contractParams(ix: Indexer) {
  const p: any = ix.params ?? {};
  const pick = (k: string) => (p[k] === undefined ? null : p[k]);
  return {
    gas_price: pick("gas_price"), gas_price_unit: pick("gas_price_unit"), tx_gas_limit: pick("tx_gas_limit"), block_gas_limit: pick("block_gas_limit"),
    contract_base_fee: pick("contract_base_fee"), contract_byte_fee: pick("contract_byte_fee"), storage_byte_deposit: pick("storage_byte_deposit"),
    max_code_size: pick("max_code_size"), max_call_depth: pick("max_call_depth"), min_fee: pick("min_fee"),
  };
}

export async function contractsModel(ix: Indexer, offset: number, codesOffset: number) {
  const [contracts, codes] = await Promise.all([read(ix, "listContracts", { offset, limit: PAGE }), read(ix, "listCodes", { offset: codesOffset, limit: PAGE })]);
  return { offset, codes_offset: codesOffset, limit: PAGE, contracts, codes, params: contractParams(ix), order: "newest first (the node's contract and code index)" };
}

/** `topic` is 64 hex or an event name (topic = blake3("pearl-sol:event:v1" || name)). */
export function topicFilter(t: string | null): { topic: string | null; name: string | null } {
  if (!t) return { topic: null, name: null };
  if (HEX64.test(t)) return { topic: t.toLowerCase(), name: null };
  return { topic: eventTopic(t), name: t };
}

export async function contractModel(ix: Indexer, address: string, offset: number, topicQ: string | null) {
  const c = await read(ix, "getContract", { address });
  if (c.ok && c.v === null) return null;
  const tf = topicFilter(topicQ);
  const ev = c.ok ? await read(ix, "getContractEvents", { address, offset, limit: PAGE, ...(tf.topic ? { topic: tf.topic } : {}) }) : c;
  const events = ev.ok && ev.v ? { ...ev, v: { ...ev.v, events: withTopicNames(ev.v.events) } } : ev;
  return { address, contract: c, events, offset, limit: PAGE, topic: tf.topic, topic_name: tf.name, known_events: KNOWN_EVENTS, topic_rule: 'topic 0 = blake3("pearl-sol:event:v1" || name); names decoded when topic 0 equals a known name' };
}

export async function codeModel(ix: Indexer, hash: string) {
  const c = await read(ix, "getCode", { code_hash: hash.toLowerCase() });
  if (c.ok && c.v === null) return null;
  // contracts running this code, among the newest 200 (the node has no per-code index)
  const l = c.ok ? await read(ix, "listContracts", { offset: 0, limit: 200 }) : c;
  const instances = l.ok ? { ok: true as const, v: { scanned: l.v.items.length, total: l.v.total, items: l.v.items.filter((x: any) => x.code_hash === hash.toLowerCase()) } } : l;
  return { code_hash: hash.toLowerCase(), code: c, instances, instances_rule: "contracts whose code_hash is this code, among the newest 200 in the node's index" };
}

export async function txModel(ix: Indexer, hash: string) {
  const t = await read(ix, "getTx", { tx_hash: hash.toLowerCase() });
  if (!t.ok) return { tx_hash: hash.toLowerCase(), tx: t, contract: null, receipt: null };
  if (t.v === null) return null;
  const tx = t.v;
  const contract = isContractKind(tx.kind) && tx.tx ? decodeContractTx(tx.tx) : null;
  let receipt: Read | null = null;
  if (isContractKind(tx.kind)) {
    if (tx.status === "pending" || tx.status === "preconfirmed") receipt = { ok: false, reason: "pending in the mempool: a receipt exists once the tx is in a block" };
    else if (tx.receipt) receipt = { ok: true, v: { ...tx.receipt, events: withTopicNames(tx.receipt.events), method: tx.receipt && contract?.body.args ? splitMethod(String(contract.body.args))?.method ?? null : null } };
    else receipt = { ok: false, reason: "the node returned no receipt for this contract tx" };
  }
  return { tx_hash: hash.toLowerCase(), tx: { ok: true as const, v: { ...tx, tx: undefined } }, tx_status: normalizeTxStatus(tx, ix.chainInfo), contract, receipt };
}

export async function contractRoute(ix: Indexer, api: boolean, rest: string[], q: URLSearchParams, sm: Summary): Promise<Res | null> {
  const bad = (what: string, msg: string) => (api ? json({ error: msg }, 400) : html(notFound(sm, what), 404));
  switch (rest[0]) {
    case "contracts": {
      const m = await contractsModel(ix, intq(q, "offset", 0, 1_000_000), intq(q, "codes_offset", 0, 1_000_000));
      return api ? json(m) : html(contractsPage(sm, m));
    }
    case "contract": {
      const a = rest[1] ?? "";
      if (!B58.test(a)) return bad("Contract", "not a base58 address");
      const m = await contractModel(ix, a, intq(q, "offset", 0, 1_000_000), q.get("topic"));
      if (!m) return api ? json({ error: "no contract at this address" }, 404) : html(notFound(sm, "Contract"), 404);
      return api ? json(m) : html(contractPage(sm, m));
    }
    case "code": {
      const h = rest[1] ?? "";
      if (!HEX64.test(h)) return bad("Code", "code hash is 64 hex");
      const m = await codeModel(ix, h);
      if (!m) return api ? json({ error: "no code with this hash" }, 404) : html(notFound(sm, "Code"), 404);
      return api ? json(m) : html(codePage(sm, m));
    }
    case "tx": {
      const h = rest[1] ?? "";
      if (!HEX64.test(h)) return bad("Transaction", "tx hash is 64 hex");
      const m = await txModel(ix, h);
      if (!m) return api ? json({ error: "tx not known to the node (not in the mempool or the canonical chain)" }, 404) : html(notFound(sm, "Transaction"), 404);
      return api ? json(m) : html(txPage(sm, m));
    }
  }
  return null;
}
