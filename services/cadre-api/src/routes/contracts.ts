// Contracts (docs/CONTRACTS.md, docs/API.md "Contracts"): read-only proxies of the node's
// contract RPC (listContracts, getContract, getContractEvents, queryContract, simulateCall,
// getContractReceipt, listCodes, getCode) with known event names decoded, and the unsigned
// Call tx builder with its fee floor from getParams. The codec is the explorer's
// (apps/explorer/src/contracts.ts, checked against chain/agent-types/vectors/contracts.json).
// A node without the contract RPC answers 503 `unavailable`; nothing is stubbed with data.

import type { Api } from "../http.ts";
import { ApiError, badRequest, notFound } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr } from "./wallet.ts";
import { hex } from "../../../../apps/explorer/src/codec.ts";
import {
  FIELD_TYPES, KNOWN_EVENTS, MAX_ARGS, TX_HDR, TX_SIG, contractFeeFloor, contractSigningBytes, decodeFields, encodeCallArgs,
  encodeCallBody, eventTopic, framedMessage, isUnknownMethod, splitMethod, withTopicNames, type FieldType,
} from "../../../../apps/explorer/src/contracts.ts";

const HEX64 = /^[0-9a-fA-F]{64}$/;
const FEE_RULE = "max(min_fee, contract_base_fee + contract_byte_fee x tx_len + ceil(gas_limit x gas_price / gas_price_unit)) from getParams; unused gas is refunded at gas_price";

/** Node call; a node without the method is 503 `unavailable`, other node errors as Deps.chain maps them. */
async function cc<T = any>(d: Deps, method: string, params: unknown): Promise<T> {
  try {
    return await d.chain<T>(method, params);
  } catch (e) {
    if (isUnknownMethod(e)) throw new ApiError(503, "unavailable", `this node does not serve ${method} (the contracts upgrade is not deployed on it)`);
    throw e;
  }
}

function intq(q: URLSearchParams, k: string, d: number, lo: number, hi: number) {
  const v = q.get(k);
  if (v === null || v === "") return d;
  const n = Number(v);
  if (!Number.isInteger(n) || n < lo || n > hi) throw badRequest(`${k} must be an integer in ${lo}..${hi}`);
  return n;
}

const u64 = (v: unknown, name: string, dflt?: bigint, min = 0n): bigint => {
  if ((v === undefined || v === null || v === "") && dflt !== undefined) return dflt;
  const t = typeof v === "number" && Number.isSafeInteger(v) ? String(v) : v;
  if (typeof t !== "string" || !/^\d{1,20}$/.test(t)) throw badRequest(`${name} must be a decimal string`);
  const n = BigInt(t);
  if (n < min || n >= 2n ** 64n) throw badRequest(`${name} must be in [${min}, 2^64)`);
  return n;
};

/** Call args from `{args: hex}` or `{method, fields}` (fields: `{u64: "5"}` or `"u64:5"`). */
export function argsFrom(b: any): Uint8Array {
  const hasArgs = b.args !== undefined && b.args !== null && b.args !== "";
  if (hasArgs && (b.method !== undefined || b.fields !== undefined)) throw badRequest("pass args (hex) or method + fields, not both");
  if (hasArgs) {
    if (typeof b.args !== "string" || !/^(0x)?([0-9a-fA-F]{2})*$/.test(b.args.trim())) throw badRequest("args must be hex");
    const a = Uint8Array.from(Buffer.from(b.args.trim().replace(/^0x/, ""), "hex"));
    if (a.length > MAX_ARGS) throw badRequest(`args are ${a.length} bytes, at most ${MAX_ARGS}`);
    return a;
  }
  if (typeof b.method !== "string" || !b.method) throw badRequest("method is required (or pass args as hex)");
  if (b.fields !== undefined && !Array.isArray(b.fields)) throw badRequest("fields must be an array");
  try {
    return encodeCallArgs(b.method, b.fields ?? []);
  } catch (e) {
    throw badRequest((e as Error).message);
  }
}

function returnsOf(b: any): FieldType[] | null {
  if (b.returns === undefined || b.returns === null) return null;
  if (!Array.isArray(b.returns) || b.returns.length > 32 || b.returns.some((t: unknown) => !FIELD_TYPES.includes(t as FieldType))) throw badRequest(`returns is a list of field types: ${FIELD_TYPES.join(", ")}`);
  return b.returns;
}

/** `topic` is 64 hex or an event name. */
function topicParam(t: string | null): { topic?: string; topic_name?: string } {
  if (!t) return {};
  if (HEX64.test(t)) return { topic: t.toLowerCase() };
  if (t.length > 128) throw badRequest("topic is 64 hex or an event name");
  return { topic: eventTopic(t), topic_name: t };
}

const named = (v: any) => (v && Array.isArray(v.events) ? { ...v, events: withTopicNames(v.events) } : v);

export function registerContracts(api: Api, d: Deps) {
  api.get("/v1/contracts", async (c) => {
    const offset = intq(c.query, "offset", 0, 0, 1_000_000_000);
    const limit = intq(c.query, "limit", 50, 1, 200);
    const r = await cc<any>(d, "listContracts", { offset, limit });
    return { total: r.total, offset, limit, order: "newest first", items: r.items };
  }, { cache: 5 });

  api.get("/v1/contracts/receipts/:tx_hash", async (c) => {
    const h = c.params.tx_hash;
    if (!HEX64.test(h)) throw badRequest("tx_hash is 64 hex");
    const r = await cc<any>(d, "getContractReceipt", { tx_hash: h.toLowerCase() });
    if (!r) throw notFound("no contract receipt for this tx (not a contract tx, not in a block yet, or unknown)");
    return named(r);
  }, { cache: 2 });

  api.get("/v1/contracts/:address", async (c) => {
    const a = addr(c.params.address, "address");
    const r = await cc<any>(d, "getContract", { address: a });
    if (!r) throw notFound("no contract at this address");
    return r;
  }, { cache: 3 });

  api.get("/v1/contracts/:address/events", async (c) => {
    const a = addr(c.params.address, "address");
    const offset = intq(c.query, "offset", 0, 0, 1_000_000_000);
    const limit = intq(c.query, "limit", 50, 1, 200);
    const tp = topicParam(c.query.get("topic"));
    const r = await cc<any>(d, "getContractEvents", { address: a, offset, limit, ...(tp.topic ? { topic: tp.topic } : {}) });
    if (!r) throw notFound("no contract at this address");
    return {
      ...named(r), offset, limit, topic: tp.topic ?? null, topic_name: tp.topic_name ?? null, known_events: KNOWN_EVENTS,
      topic_rule: 'topic 0 = blake3("pearl-sol:event:v1" || name); `name` is set when topic 0 equals a known name; newest first, `next_offset` pages on',
    };
  }, { cache: 3 });

  api.post("/v1/contracts/:address/query", async (c) => {
    const a = addr(c.params.address, "address");
    const b = await c.json();
    const args = argsFrom(b);
    const returns = returnsOf(b);
    const gas = b.gas === undefined ? undefined : u64(b.gas, "gas", undefined, 1n);
    const r = await cc<any>(d, "queryContract", { address: a, args: hex(args), ...(gas ? { gas: gas.toString() } : {}) });
    const out: Record<string, unknown> = { contract: a, args: hex(args), method: splitMethod(args)?.method ?? null, ...r };
    if (returns && r.ok) {
      try {
        out.decoded = decodeFields(r.return ?? "", returns);
      } catch (e) {
        out.decoded = null;
        out.decode_error = `return data does not decode as ${returns.join(", ")}: ${(e as Error).message}`;
      }
    }
    return out;
  }, { cost: 3 });

  api.post("/v1/contracts/:address/simulate", async (c) => {
    const a = addr(c.params.address, "address");
    const b = await c.json();
    const from = addr(b.from, "from");
    const args = argsFrom(b);
    const value = u64(b.value, "value", 0n);
    const gas = b.gas === undefined ? undefined : u64(b.gas, "gas", undefined, 1n);
    const r = await cc<any>(d, "simulateCall", { from, address: a, args: hex(args), value: value.toString(), ...(gas ? { gas: gas.toString() } : {}) });
    return { contract: a, from, value: value.toString(), args: hex(args), method: splitMethod(args)?.method ?? null, ...named(r) };
  }, { cost: 5 });

  api.get("/v1/codes", async (c) => {
    const offset = intq(c.query, "offset", 0, 0, 1_000_000_000);
    const limit = intq(c.query, "limit", 50, 1, 200);
    const r = await cc<any>(d, "listCodes", { offset, limit });
    return { total: r.total, offset, limit, order: "newest first", items: r.items };
  }, { cache: 5 });

  api.get("/v1/codes/:hash", async (c) => {
    const h = c.params.hash;
    if (!HEX64.test(h)) throw badRequest("hash is 64 hex");
    const include = c.query.get("include_code") === "1" || c.query.get("include_code") === "true";
    const r = await cc<any>(d, "getCode", { code_hash: h.toLowerCase(), include_code: include });
    if (!r) throw notFound("no code with this hash");
    return r;
  }, { cache: 10 });

  // ---------------------------------------------------------------- unsigned Call

  api.post("/v1/tx/contract-call", async (c) => {
    const b = await c.json();
    const from = addr(b.from, "from");
    const contract = addr(b.contract, "contract");
    const value = u64(b.value, "value", 0n);
    const gasLimit = u64(b.gas_limit, "gas_limit", undefined, 1n);
    const args = argsFrom(b);
    const [acct, params, info, rec] = await Promise.all([
      d.chain<any>("getAccount", { account: from }), d.chain<any>("getParams", {}), d.chain<any>("getChainInfo", {}), cc<any>(d, "getContract", { address: contract }),
    ]);
    if (!rec) throw notFound("no contract at this address");
    if (params.tx_gas_limit !== undefined && gasLimit > BigInt(params.tx_gas_limit)) throw badRequest(`gas_limit ${gasLimit} above tx_gas_limit ${params.tx_gas_limit}`);
    const body = encodeCallBody({ contract, value, gasLimit, args });
    const txLen = TX_HDR + body.length + TX_SIG;
    let floor;
    try {
      floor = contractFeeFloor(params, txLen, gasLimit);
    } catch (e) {
      throw new ApiError(503, "unavailable", (e as Error).message);
    }
    let fee = floor.fee;
    if (b.fee !== undefined && b.fee !== null && b.fee !== "") {
      fee = u64(b.fee, "fee", undefined, 1n);
      if (fee < floor.fee) throw badRequest(`fee ${fee} below the floor ${floor.fee} (${FEE_RULE})`);
    }
    if (BigInt(acct.balance) < fee + value) throw new ApiError(422, "insufficient_funds", `balance ${acct.balance} < fee ${fee}${value ? ` + value ${value}` : ""} (chain units; the sender must hold CADRE on the agent chain)`);
    const signing = contractSigningBytes(info.chain_id, from, acct.nonce, fee, "Call", body);
    return {
      window_pearl: { method: "signAndSendTransaction", params: { transaction: { kind: "Call", contract, value: value.toString(), gasLimit: gasLimit.toString(), args: hex(args), fee: fee.toString() } } },
      chain_id: info.chain_id, nonce: acct.nonce, fee: fee.toString(), fee_floor: floor.fee.toString(), fee_rule: FEE_RULE,
      fee_parts: { contract_base_fee: floor.base.toString(), byte_fee: floor.byte_fee.toString(), gas_fee: floor.gas_fee.toString(), min_fee: floor.min_fee.toString(), tx_len: txLen },
      unsigned_tx: hex(signing),
      framed_message: hex(framedMessage(signing)),
      tx_len: txLen,
      encoding: "unsigned_tx = the tx without its signature (version, chain_id, from, nonce, fee, kind 32, body); the wallet signs framed_message (varint len || \"pearl-sol:tx:v1\" || unsigned_tx) with ed25519 and appends the 64-byte signature; POST /v1/tx/chain {tx: base64} relays it",
      summary: { from, contract, label: rec.label || null, method: splitMethod(args)?.method ?? null, value: value.toString(), gas_limit: gasLimit.toString() },
      sign_with: "window.pearl.signAndSendTransaction({ transaction: window_pearl.params.transaction }) -> { txHash }; then GET /v1/tx/chain/{txHash} and GET /v1/contracts/receipts/{txHash}",
    };
  });
}
