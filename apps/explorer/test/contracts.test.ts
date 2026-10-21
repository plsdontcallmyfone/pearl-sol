// Contract codec (src/contracts.ts) byte for byte against chain/agent-types/vectors/contracts.json:
// call args ABI, event topic, contract address rule, every contract tx kind decoded, and the
// Call signing bytes / framed message / tx length rebuilt from the decoded fields.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { hex } from "../src/codec.ts";
import * as C from "../src/contracts.ts";

const V = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../../../chain/agent-types/vectors/contracts.json"), "utf8"));
const vtx = (name: string) => V.txs.find((t: any) => t.name === name);

test("call args: method + fields match the vector, both field forms", () => {
  const a = V.call_args;
  assert.equal(hex(C.encodeCallArgs(a.method, a.fields)), a.args);
  assert.equal(hex(C.encodeCallArgs(a.method, ["u64:5"])), a.args);
  assert.deepEqual(C.splitMethod(a.args), { method: "increment", rest: "0500000000000000" });
  assert.deepEqual(C.decodeFields("0500000000000000", ["u64"]), ["5"]);
  // every type round-trips
  const fields: C.Field[] = [{ u8: 7 }, { u16: "513" }, { u32: 70000 }, { u64: "18446744073709551615" }, { bool: true }, { addr: V.address_rule.creator }, { hash: "ab".repeat(32) }, { bytes: "00ff" }, { str: "héllo" }];
  const enc = C.encodeFields(fields);
  assert.deepEqual(C.decodeFields(enc, ["u8", "u16", "u32", "u64", "bool", "addr", "hash", "bytes", "str"]), ["7", "513", "70000", "18446744073709551615", true, V.address_rule.creator, "ab".repeat(32), "00ff", "héllo"]);
  assert.throws(() => C.encodeFields([{ u8: 256 }]), /does not fit/);
  assert.throws(() => C.encodeFields(["i64:5"]), /unknown type/);
  assert.throws(() => C.decodeFields("0500", ["u64"]), /too short/);
  assert.throws(() => C.decodeFields("050000000000000000", ["u64"]), /trailing/);
});

test("event topic and the known-name table", () => {
  assert.equal(C.eventTopic(V.event_topic.name), V.event_topic.topic);
  assert.equal(C.topicName(V.event_topic.topic), "Incremented");
  assert.equal(C.topicName(V.event_topic.topic.toUpperCase()), "Incremented");
  assert.equal(C.topicName("00".repeat(32)), null);
  for (const n of C.KNOWN_EVENTS) assert.equal(C.topicName(C.eventTopic(n)), n);
  assert.deepEqual(C.withTopicNames([{ topics: [V.event_topic.topic, "11".repeat(32)] }, { topics: [] }]).map((e) => e.name), ["Incremented", null]);
});

test("contract address rule", () => {
  const r = V.address_rule;
  assert.equal(C.contractAddress(r.creator, r.nonce), r.address);
});

test("every contract tx kind decodes to the vector's body", () => {
  for (const t of V.txs) {
    const b64 = Buffer.from(t.tx, "hex").toString("base64");
    const d = C.decodeContractTx(b64)!;
    assert.ok(d, t.name);
    assert.equal(d.kind, t.kind_name);
    assert.equal(d.from, t.from);
    assert.equal(d.nonce, String(t.nonce));
    assert.equal(d.fee, t.fee);
    assert.equal(d.chain_id, t.chain_id);
    for (const [k, v] of Object.entries(t.body)) {
      if (k === "code") continue;
      if (k === "admin" && v === "11111111111111111111111111111111") assert.equal(d.body.admin, null, `${t.name}.admin zero = none`);
      else assert.equal(String(d.body[k]), String(v), `${t.name}.${k}`);
    }
  }
  const dep = C.decodeContractTx(Buffer.from(vtx("deploy_code").tx, "hex"))!;
  assert.equal(dep.body.code_size, 8);
  const inst = C.decodeContractTx(Buffer.from(vtx("instantiate").tx, "hex"))!;
  assert.equal(inst.body.contract, V.address_rule.address, "Instantiate predicts the address from sender + nonce");
  const call = C.decodeContractTx(Buffer.from(vtx("call").tx, "hex"))!;
  assert.equal(call.body.method, "increment");
  // a non-contract kind is not decoded
  const other = Buffer.from(vtx("call").tx, "hex");
  other[53] = 3;
  assert.equal(C.decodeContractTx(other), null);
});

test("Call: signing bytes, framed message and tx length rebuilt exactly", () => {
  const t = vtx("call");
  const body = C.encodeCallBody({ contract: t.body.contract, value: t.body.value, gasLimit: t.body.gas_limit, args: Buffer.from(t.body.args, "hex") });
  const sb = C.contractSigningBytes(t.chain_id, t.from, t.nonce, t.fee, "Call", body);
  assert.equal(hex(sb), t.signing_bytes);
  assert.equal(hex(C.framedMessage(sb)), t.framed_message);
  assert.equal(sb.length + C.TX_SIG, Buffer.from(t.tx, "hex").length);
});

test("fee floor: base + byte fee x len + ceil(gas x price / unit), at least min_fee; refuses a node without the params", () => {
  const p = { min_fee: "1000", contract_base_fee: "10000", contract_byte_fee: "10", gas_price: "3", gas_price_unit: "1000000" };
  const f = C.contractFeeFloor(p, 200, 1_000_001);
  assert.equal(f.gas_fee, 4n, "ceil(3000003 / 1e6) = 4");
  assert.equal(f.fee, 10000n + 2000n + 4n);
  assert.equal(C.contractFeeFloor({ ...p, contract_base_fee: "0", contract_byte_fee: "0", gas_price: "0" }, 200, 5).fee, 1000n);
  assert.throws(() => C.contractFeeFloor({ min_fee: "1000" }, 200, 5), /no contract_base_fee/);
});
