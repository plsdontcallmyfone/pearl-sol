// Contract kinds 30-34 against the node's own vectors (chain/agent-types/vectors/contracts.json).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { b58, fromHex, hex, key32 } from "../src/core/bytes";
import { blake3 } from "../src/core/crypto";
import { UnsignedTx, decodeTx, signingBytes } from "../src/chain/tx";
import {
  codeHash, contractActivity, contractAddress, contractBodyFromJson, contractFeeFloor, contractFeeFor, contractFeeParams, dappContractWarnings,
  decodeContractsRecord, decodeEvent, decodeReceipt, describeFields, encodeCallArgs, encodedTxLen, eventTopic, leafHash,
  merkleRoot, parseCallArgs, summarizeContract, verifyPath,
} from "../src/chain/contracts";
import { summarizeChainTx } from "../src/shared/summary";

const v = JSON.parse(readFileSync(resolve(__dirname, "../../chain/agent-types/vectors/contracts.json"), "utf8"));
const tx = (name: string) => v.txs.find((t: any) => t.name === name);
const unsigned = (name: string): UnsignedTx => {
  const d = decodeTx(fromHex(tx(name).tx));
  return { chainId: d.chainId, from: d.from, nonce: d.nonce, fee: d.fee, body: d.body };
};
const nat = (x: bigint) => `${x} units`;
// chain/state/src/config.rs CONTRACT_DEFAULTS with the node's min_fee as getParams reports them
const params = contractFeeParams({ min_fee: "1000", gas_price: "10000", gas_price_unit: "1000000", contract_base_fee: "1000", contract_byte_fee: "10", storage_byte_deposit: "100000" })!;

describe("contract vectors", () => {
  it("address rule", () => {
    const a = v.address_rule;
    expect(b58(contractAddress(key32(a.creator), BigInt(a.nonce)))).toBe(a.address);
    // the instantiate vector creates it: same sender, nonce 7
    const ins = tx("instantiate");
    expect(ins.from).toBe(a.creator);
    expect(ins.nonce).toBe(a.nonce);
    expect(tx("call").body.contract).toBe(a.address);
  });

  it("code hash", () => {
    const d = tx("deploy_code");
    expect(hex(codeHash(fromHex(d.body.code)))).toBe(d.body.code_hash);
  });

  it("call args ABI", () => {
    const c = v.call_args;
    expect(hex(encodeCallArgs(c.method, c.fields))).toBe(c.args);
    expect(tx("call").body.args).toBe(c.args);
    const p = parseCallArgs(fromHex(c.args))!;
    expect(p.method).toBe("increment");
    expect(describeFields(p.fields).text).toBe("5 (u64)");
  });

  it("event topic, event and receipt", () => {
    expect(hex(eventTopic(v.event_topic.name))).toBe(v.event_topic.topic);
    const e = decodeEvent(fromHex(v.event.bytes));
    expect(hex(leafHash(fromHex(v.event.bytes)))).toBe(v.event.hash);
    expect(b58(e.contract)).toBe(v.address_rule.address);
    expect(hex(e.txHash)).toBe(tx("call").tx_hash);
    expect(hex(e.topics[0])).toBe(v.event_topic.topic);

    const r = decodeReceipt(fromHex(v.receipt.bytes));
    expect(hex(leafHash(fromHex(v.receipt.bytes)))).toBe(v.receipt.hash);
    expect(hex(r.txHash)).toBe(tx("call").tx_hash);
    expect(r.kind).toBe(32);
    expect(r.status).toBe("ok");
    expect(r.gasLimit).toBe(1_000_000n);
    expect(b58(r.contract)).toBe(v.address_rule.address);
    expect(hex(r.codeHash)).toBe(tx("deploy_code").body.code_hash);
    expect(r.eventCount).toBe(1);
    expect(r.gasUsed).toBeLessThanOrEqual(r.gasLimit);
  });

  it("contracts record, its roots and the block's tx_root paths", () => {
    const rec = decodeContractsRecord(fromHex(v.record.bytes));
    expect(hex(leafHash(fromHex(v.record.bytes)))).toBe(v.record.hash);
    expect(hex(rec.receiptsRoot)).toBe(v.record.receipts_root);
    expect(hex(rec.eventsRoot)).toBe(v.record.events_root);
    expect(hex(merkleRoot([fromHex(v.receipt.hash)]))).toBe(v.record.receipts_root);
    expect(hex(merkleRoot([fromHex(v.event.hash)]))).toBe(v.record.events_root);
    expect(rec.gasUsed).toBe(decodeReceipt(fromHex(v.receipt.bytes)).gasUsed);

    const b = v.block;
    const leaves = [blake3(fromHex(b.txs[0])), fromHex(v.record.hash), blake3(fromHex(b.coinbase))];
    expect(b.record_leaf_index).toBe(1);
    expect(hex(merkleRoot(leaves))).toBe(b.tx_root);
    expect(verifyPath(fromHex(v.record.hash), fromHex(b.record_path), fromHex(b.tx_root))).toBe(true);
    expect(verifyPath(fromHex(v.event.hash), fromHex(b.event_path), rec.eventsRoot)).toBe(true);
    expect(verifyPath(fromHex(v.receipt.hash), fromHex(b.record_path), fromHex(b.tx_root))).toBe(false);
  });
});

describe("window.pearl JSON form", () => {
  const json: Record<string, any> = {
    deploy_code: (b: any) => ({ kind: "DeployCode", gasLimit: b.gas_limit, code: b.code }),
    instantiate: (b: any) => ({ kind: "Instantiate", codeHash: b.code_hash, admin: b.admin, value: b.value, gasLimit: b.gas_limit, label: b.label, fields: [{ u64: "10" }] }),
    call: (b: any) => ({ kind: "Call", contract: b.contract, value: b.value, gasLimit: b.gas_limit, method: "increment", fields: [{ u64: 5 }] }),
    migrate: (b: any) => ({ kind: "Migrate", contract: b.contract, codeHash: b.code_hash, gasLimit: b.gas_limit, args: b.args }),
    set_contract_admin: (b: any) => ({ kind: "SetContractAdmin", contract: b.contract, admin: null }),
  };
  for (const t of v.txs) {
    it(`${t.name}: JSON form encodes to the vector`, () => {
      const u = { chainId: t.chain_id, from: key32(t.from), nonce: BigInt(t.nonce), fee: BigInt(t.fee), body: contractBodyFromJson(json[t.name](t.body)) };
      expect(hex(signingBytes(u))).toBe(t.signing_bytes);
    });
  }

  it("refuses bad JSON", () => {
    expect(() => contractBodyFromJson({ kind: "Call", contract: "nope", gasLimit: 1, method: "x" })).toThrow(/base58/);
    expect(() => contractBodyFromJson({ kind: "Call", contract: v.address_rule.address, method: "x" })).toThrow(/gasLimit/);
    expect(() => contractBodyFromJson({ kind: "Call", contract: v.address_rule.address, gasLimit: 1 })).toThrow(/method/);
    expect(() => contractBodyFromJson({ kind: "Call", contract: v.address_rule.address, gasLimit: 1, method: "x", args: "00" })).toThrow(/not both/);
    expect(() => contractBodyFromJson({ kind: "SetContractAdmin", contract: v.address_rule.address })).toThrow(/admin is required/);
    expect(() => contractBodyFromJson({ kind: "Instantiate", codeHash: "00", gasLimit: 1 })).toThrow(/64 hex/);
    expect(() => contractBodyFromJson({ kind: "Call", contract: v.address_rule.address, gasLimit: 1, method: "x", fields: [{ u8: 256 }] })).toThrow(/u8/);
  });
});

describe("fee floor", () => {
  it("max(min_fee, base + byte x len + ceil(gas x price / unit))", () => {
    // gas 1,000,000 x 10,000 / 1e6 = 10,000; ceil on a remainder
    expect(contractFeeFloor(params, 100, 1_000_000n)).toEqual({ floor: 1000n + 1000n + 10_000n, fixed: 2000n, gas: 10_000n });
    expect(contractFeeFloor(params, 0, 1n).gas).toBe(1n);
    expect(contractFeeFloor({ ...params, minFee: 50_000n }, 10, 10n).floor).toBe(50_000n);
  });
  it("uses the encoded tx length (signing bytes + sig)", () => {
    const t = unsigned("call");
    expect(encodedTxLen(t)).toBe(fromHex(tx("call").tx).length);
    expect(contractFeeFor(t, params)).toBe(1000n + 10n * BigInt(encodedTxLen(t)) + 10_000n);
  });
  it("needs every getParams field", () => {
    expect(contractFeeParams({ min_fee: "1000" })).toBeNull();
  });
});

describe("approval wording", () => {
  const me = tx("instantiate").from;
  it("DeployCode", () => {
    const s = summarizeContract(unsigned("deploy_code"), nat, { me, fee: params })!;
    expect(s.headline).toBe("Deploy contract code (8 bytes, hash 0d66d411...)");
    expect(s.rows.find((r) => r.label === "Code storage deposit")!.value).toBe("800000 units from your balance");
  });
  it("Instantiate", () => {
    const s = summarizeContract(unsigned("instantiate"), nat, { me })!;
    expect(s.headline).toBe("Create contract 'counter' from code 0d66d411..., admin: you");
    expect(s.rows.find((r) => r.label === "New contract address")!.full).toBe(v.address_rule.address);
    expect(s.rows.find((r) => r.label === "Init arguments")!.value).toBe("10 (u64)");
    const none = summarizeContract({ ...unsigned("instantiate"), body: { ...(unsigned("instantiate").body as any), admin: new Uint8Array(32) } }, nat, { me })!;
    expect(none.headline).toContain("admin: none (immutable)");
  });
  it("Call with the contract label, value and gas", () => {
    const t = unsigned("call");
    const s = summarizeContract(t, nat, { me: tx("call").from, labels: { [v.address_rule.address]: "counter" }, fee: params })!;
    expect(s.headline).toBe("Call increment on contract counter (Ad49...wcZ) with 5 (u64), sending 1000000000 units, gas up to 1,000,000");
    expect(s.warnings.join(" ")).toContain("Only the contract's code decides");
    const gasRow = s.feeRows.find((r) => r.label === "Gas")!;
    expect(gasRow.value).toBe("up to 10000 units, unused gas is refunded");
    // the vector's fee 2,000,000 is above this floor
    expect(s.feeRows.find((r) => r.label === "Above the floor")).toBeTruthy();
  });
  it("Call with raw input that is not the ABI", () => {
    const t = unsigned("call");
    const s = summarizeContract({ ...t, body: { ...(t.body as any), args: Uint8Array.of(0, 1, 2) } }, nat, {})!;
    expect(s.headline).toContain("with raw input");
    expect(s.rows.find((r) => r.label === "Input (hex)")!.full).toBe("000102");
  });
  it("Migrate and SetContractAdmin", () => {
    const m = summarizeContract(unsigned("migrate"), nat, { me })!;
    expect(m.headline).toBe("Migrate contract Ad49...wcZ to code 0d66d411... (admin action)");
    expect(m.warnings.join(" ")).toContain("Admin action");
    const a = summarizeContract(unsigned("set_contract_admin"), nat, { me })!;
    expect(a.headline).toBe("Make contract Ad49...wcZ immutable");
    expect(a.warnings.join(" ")).toContain("Permanent");
    const t = unsigned("set_contract_admin");
    const h = summarizeContract({ ...t, body: { ...(t.body as any), admin: key32(tx("call").from) } }, nat, { me })!;
    expect(h.headline).toBe("Hand contract Ad49...wcZ's admin to BAWZ...9km");
  });
  it("fee below the floor warns", () => {
    const t = { ...unsigned("call"), fee: 1000n };
    expect(summarizeContract(t, nat, { fee: params })!.warnings.join(" ")).toContain("below this transaction's floor");
  });
  it("site warnings for value and admin actions", () => {
    expect(dappContractWarnings(unsigned("call").body, nat)[0]).toContain("send 1000000000 units into a contract");
    expect(dappContractWarnings(unsigned("migrate").body, nat)[0]).toContain("admin action");
    expect(dappContractWarnings(unsigned("set_contract_admin").body, nat)[0]).toContain("immutable forever");
    expect(dappContractWarnings(unsigned("deploy_code").body, nat)).toEqual([]);
  });
  it("activity lines", () => {
    const me = tx("instantiate").from;
    expect(contractActivity(unsigned("deploy_code"), me)!.summary).toBe("Deployed contract code (8 bytes, hash 0d66d411...)");
    expect(contractActivity(unsigned("instantiate"), me)!).toMatchObject({ summary: "Created contract 'counter'", counterparty: v.address_rule.address, flow: "none" });
    expect(contractActivity(unsigned("call"), tx("call").from, { [v.address_rule.address]: "counter" })!).toMatchObject({ summary: "Called increment on counter (Ad49...wcZ)", amount: "1000000000", flow: "out" });
    expect(contractActivity(unsigned("migrate"), me)!.summary).toBe("Migrated contract Ad49...wcZ to code 0d66d411...");
    expect(contractActivity(unsigned("set_contract_admin"), me)!.summary).toBe("Made contract Ad49...wcZ immutable");
  });
  it("summarizeChainTx puts the fee rows in the Transaction section", () => {
    const s = summarizeChainTx(unsigned("call"), tx("call").from, 1347616769, { fee: params });
    expect(s.headline).toContain("Call increment on contract Ad49...wcZ");
    const head = s.sections.find((x) => x.title === "Transaction")!.rows.map((r) => r.label);
    expect(head).toContain("Network fee (most you pay)");
    expect(head).toContain("Gas");
    expect(head).not.toContain("Network fee");
  });
});
