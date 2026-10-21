import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { b58decode, b58encode } from "../../src/b58.ts";
import { walletFromSeed, verifySig } from "../../src/wallet.ts";
import { registerAgentSigning, signRegisterAgent, signSolanaTx, txSignature } from "../src/chain.ts";

const VEC = JSON.parse(readFileSync(new URL("../../../../chain/agent-types/vectors/tx.json", import.meta.url), "utf8"));
const h = (s: string) => Uint8Array.from(Buffer.from(s, "hex"));

test("RegisterAgent bytes match the chain's vector byte for byte", () => {
  const v = VEC.find((x: any) => x.kind_name === "RegisterAgent");
  const w = walletFromSeed(h(v.seed));
  assert.equal(w.address, v.from);
  const sb = registerAgentSigning(Number(v.chain_id), w.publicKey, BigInt(v.nonce), BigInt(v.fee), v.body.name, v.body.uri);
  assert.equal(Buffer.from(sb).toString("hex"), v.signing_bytes);
  const tx = signRegisterAgent(w, Number(v.chain_id), BigInt(v.nonce), BigInt(v.fee), v.body.name, v.body.uri);
  assert.equal(Buffer.from(tx).toString("hex"), v.tx);
});

test("signs its slot in a Solana legacy tx and only its slot", () => {
  const payer = walletFromSeed(new Uint8Array(32).fill(7));
  const other = walletFromSeed(new Uint8Array(32).fill(9));
  const keys = [payer.publicKey, other.publicKey, new Uint8Array(32)];
  const msg = Uint8Array.from([2, 0, 1, 3, ...keys.flatMap((k) => [...k]), ...new Uint8Array(32).fill(1), 1, 2, 2, 0, 1, 0]);
  const unsigned = Buffer.from(Uint8Array.from([2, ...new Uint8Array(128), ...msg])).toString("base64");
  const s1 = Buffer.from(signSolanaTx(other, unsigned), "base64");
  assert.ok(s1.subarray(1, 65).every((x) => x === 0), "payer slot untouched");
  assert.ok(verifySig(other.publicKey, msg, s1.subarray(65, 129)));
  const s2 = Buffer.from(signSolanaTx(payer, s1.toString("base64")), "base64");
  assert.ok(verifySig(payer.publicKey, msg, s2.subarray(1, 65)));
  assert.equal(txSignature(s2.toString("base64")), b58encode(s2.subarray(1, 65)));
  const stranger = walletFromSeed(new Uint8Array(32).fill(3));
  assert.throws(() => signSolanaTx(stranger, unsigned), /not a signer/);
  assert.equal(b58decode(b58encode(payer.publicKey)).length, 32);
});
