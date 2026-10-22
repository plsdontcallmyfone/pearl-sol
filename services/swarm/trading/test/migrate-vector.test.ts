// The Migrate encoder (src/cadre.ts) against the chain's published vector (chain/agent-types/vectors/contracts.json).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { b58decode } from "../../src/b58.ts";
import { contractSigningBytes } from "../src/cadre.ts";

test("Migrate signing bytes match the chain vector", () => {
  const v = JSON.parse(readFileSync(new URL("../../../../chain/agent-types/vectors/contracts.json", import.meta.url), "utf8")).txs.find((t: any) => t.kind_name === "Migrate");
  const b = contractSigningBytes(v.chain_id, b58decode(v.from), BigInt(v.nonce), BigInt(v.fee), { kind: "Migrate", contract: v.body.contract, codeHash: v.body.code_hash, gasLimit: BigInt(v.body.gas_limit), args: Buffer.from(v.body.args, "hex") });
  assert.equal(Buffer.from(b).toString("hex"), v.signing_bytes);
});
