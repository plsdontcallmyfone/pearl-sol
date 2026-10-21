// A small fake agent chain with contracts for the contract page tests and headless screenshots:
// a fake node JSON-RPC serving the contract methods (docs/RPC.md "Contracts") with the real tx
// bytes and receipt of chain/agent-types/vectors/contracts.json, plus an empty Solana RPC
// (agent-fixture.ts). Fixture data only; never shown as real figures.
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { eventTopic } from "../src/contracts.ts";
import { key } from "./agent-fixture.ts";
export { fakeSolana } from "./agent-fixture.ts";

const V = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../../../chain/agent-types/vectors/contracts.json"), "utf8"));
const vtx = (name: string) => V.txs.find((t: any) => t.name === name);
const jid = (c: string) => c.repeat(64).slice(0, 64);

export const CALL = vtx("call");
export const INST = vtx("instantiate");
export const COUNTER = V.address_rule.address as string;
export const CREATOR = V.address_rule.creator as string;
export const CODE = vtx("deploy_code").body.code_hash as string;
export const ESCROW = key(41);
export const ESCROW_CODE = jid("e5");
export const OTHER_TX = jid("77");
const tip = 30;

export const counter = {
  address: COUNTER, code_hash: CODE, admin: CREATOR, upgradable: true, creator: CREATOR, label: "counter", created_height: 12,
  migrated_height: null, storage_bytes: 48, storage_items: 2, deposit: "48000", index: 1, calls: 3, events: 3,
  balance: "1000000000", code_size: 8, entries: ["instantiate", "call", "query", "migrate"], imports: ["env.storage_get", "env.storage_set", "env.emit"],
};
export const escrow = {
  address: ESCROW, code_hash: ESCROW_CODE, admin: null, upgradable: false, creator: CREATOR, label: "escrow", created_height: 20,
  migrated_height: null, storage_bytes: 210, storage_items: 4, deposit: "210000", index: 0, calls: 1, events: 2,
};
const codes = [
  { code_hash: CODE, deployer: CREATOR, height: 11, size: 8, deposit: "8000", instances: 1, index: 0 },
  { code_hash: ESCROW_CODE, deployer: CREATOR, height: 19, size: 2311, deposit: "2311000", instances: 1, index: 1 },
];

const ev = (n: number, height: number, tx: string, name: string, data: string, extra: string[] = []) => ({
  contract: COUNTER, tx_hash: tx, index: 0, topics: [eventTopic(name), ...extra], data, bytes: "", hash: jid(String(n)), height, n,
});
export const counterEvents = [
  ev(2, 24, CALL.tx_hash, "Incremented", "0500000000000000"),
  ev(1, 22, OTHER_TX, "Reset", ""),
  { ...ev(0, 13, jid("13"), "Incremented", "0100000000000000"), topics: [jid("ab")] }, // an unknown topic stays hex
];

export const receipt = {
  tx_hash: CALL.tx_hash, kind: 32, status: "ok", ok: true, gas_limit: "1000000", gas_used: "412345", fee_charged: "1497000",
  contract: COUNTER, code_hash: CODE, event_count: 1, return: "0500000000000000", error: "", bytes: V.receipt.bytes, hash: V.receipt.hash,
  height: 24, events: [counterEvents[0]], block_hash: jid("h24"), final: true,
};
export const failed = { ...receipt, tx_hash: OTHER_TX, status: "out_of_gas", ok: false, gas_used: "1000000", return: "", error: "out of gas after 1000000", event_count: 0, events: [], height: 22 };

export const PARAMS = {
  alpha_ppm: 800000, emission_ppm: 10000, min_fee: "1000", gas_price: "1500", gas_price_unit: "1000000", tx_gas_limit: "50000000",
  block_gas_limit: "150000000", contract_base_fee: "100000", contract_byte_fee: "1000", storage_byte_deposit: "1000", max_code_size: 262144, max_call_depth: 8,
};

export function fakeNode(opts: { contracts?: boolean } = {}) {
  const on = opts.contracts !== false;
  const handle = (m: string, p: any) => {
    const cm = ["listContracts", "listCodes", "getContract", "getCode", "getContractEvents", "getContractReceipt", "queryContract", "simulateCall"];
    if (!on && cm.includes(m)) throw new Error(`unknown method ${m}`);
    switch (m) {
      case "getChainInfo": return { genesis_hash: jid("c0"), height: tip, finalized_height: tip - 2, finality_k: 2, chain_id: CALL.chain_id };
      case "getParams": return on ? PARAMS : { alpha_ppm: 800000, emission_ppm: 10000, min_fee: "1000" };
      case "getBlock": {
        const h = p.height ?? (typeof p.hash === "string" ? parseInt(p.hash.replace(/^h/, ""), 10) : 0);
        if (!h || h < 1 || h > tip) return null;
        const txs = h === 24 ? [{ tx_hash: CALL.tx_hash, kind: "Call", from: CALL.from, nonce: CALL.nonce, fee: CALL.fee, tx: Buffer.from(CALL.tx, "hex").toString("base64") }] : [];
        return {
          hash: jid(`h${h}`), canonical: true, final: h <= tip - 2, confirmations: tip - h, fees: h === 24 ? CALL.fee : "0", subsidy: "0", reward: h === 24 ? CALL.fee : "0", proof_len: 100,
          header_fields: { height: h, prev_hash: jid(`h${h - 1}`), timestamp_ms: 1_760_000_000_000 + h * 60_000, miner: CREATOR, job_id: "0".repeat(64), proof_hash: jid("p"), target_bits: 1 },
          txs,
          contracts: h === 24 ? { record: V.record.bytes, record_hash: V.record.hash, height: 24, receipt_count: 1, receipts_root: V.record.receipts_root, event_count: 1, events_root: V.record.events_root, gas_used: "412345", leaf_index: 1, path: "" } : null,
        };
      }
      case "listJobs": return { total: 0, jobs: [] };
      case "getBridgeEvents": return { withdrawals: [] };
      case "listContracts": return { total: 2, offset: p.offset ?? 0, items: [escrow, counter].slice(p.offset ?? 0, (p.offset ?? 0) + (p.limit ?? 50)) };
      case "listCodes": return { total: 2, offset: p.offset ?? 0, items: [...codes].reverse().slice(p.offset ?? 0, (p.offset ?? 0) + (p.limit ?? 50)) };
      case "getContract": return p.address === COUNTER ? counter : p.address === ESCROW ? { ...escrow, balance: "0", code_size: 2311, entries: ["instantiate", "call"] } : null;
      case "getCode": return codes.find((c) => c.code_hash === p.code_hash) ?? null;
      case "getContractEvents": {
        if (p.address !== COUNTER) return p.address === ESCROW ? { address: ESCROW, total: 0, events: [], next_offset: 0 } : null;
        const l = counterEvents.filter((e) => !p.topic || e.topics[0] === p.topic).slice(p.offset ?? 0, (p.offset ?? 0) + (p.limit ?? 50));
        return { address: COUNTER, total: counterEvents.length, events: l, next_offset: (p.offset ?? 0) + l.length };
      }
      case "getContractReceipt": return p.tx_hash === CALL.tx_hash ? receipt : p.tx_hash === OTHER_TX ? failed : null;
      case "getTx": {
        if (p.tx_hash === CALL.tx_hash) return { tx_hash: CALL.tx_hash, kind: "Call", from: CALL.from, nonce: CALL.nonce, fee: CALL.fee, tx: Buffer.from(CALL.tx, "hex").toString("base64"), status: "included", block_hash: jid("h24"), height: 24, index: 0, confirmations: 6, ...(on ? { receipt } : {}) };
        if (p.tx_hash === INST.tx_hash) return { tx_hash: INST.tx_hash, kind: "Instantiate", from: INST.from, nonce: INST.nonce, fee: INST.fee, tx: Buffer.from(INST.tx, "hex").toString("base64"), status: "pending" };
        return null;
      }
      default: throw new Error("no " + m);
    }
  };
  return createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => { try { return { jsonrpc: "2.0", id: x.id, result: handle(x.method, x.params) }; } catch (e) { return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } }; } };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
}
