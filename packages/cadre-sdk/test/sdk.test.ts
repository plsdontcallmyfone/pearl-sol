// SDK unit checks: the window.cadre JSON -> signed bytes path agrees byte for byte with the repo's
// vector-tested encoders (services/swarm chaintx, the runner's RegisterAgent, the trading contract
// encoder); key parsing (Solana JSON array, base58 secret, seed) never exposes the secret; limits
// refuse when unset and over caps; Solana signing puts the signature in this key's slot.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtempSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { inspect } from "node:util";
import { keyFrom, keyFromSeed, loadKeyFile, verify, Limits, LimitError, encodeWindowBody, signingBytes, signSolanaTx, signersOf, framed, toCx } from "../src/index.ts";
import { b58encode } from "../src/vendor/b58.ts";
import { signTx } from "../../../services/swarm/src/chaintx.ts";
import { walletFromSeed } from "../../../services/swarm/src/wallet.ts";
import { registerAgentSigning } from "../../../services/swarm/agents/src/chain.ts";
import { contractSigningBytes } from "../../../services/swarm/trading/src/cadre.ts";

const seed = createHash("sha256").update("sdk-test").digest();
const k = keyFromSeed(seed);
const w = walletFromSeed(seed);
const peer = keyFromSeed(createHash("sha256").update("peer").digest());
const CHAIN = 1347616769;

test("keys: one key, both chains; secret never printed", () => {
  assert.equal(k.address, w.address);
  assert.equal(k.cx, toCx(k.publicKey));
  const full = [...seed, ...k.publicKey];
  assert.equal(keyFrom(JSON.stringify(full)).address, k.address);
  assert.equal(keyFrom(b58encode(Uint8Array.from(full))).address, k.address);
  assert.throws(() => keyFrom(JSON.stringify([...seed, ...peer.publicKey])), /does not match/);
  const shown = JSON.stringify(k) + inspect(k);
  assert.ok(!shown.includes(seed.toString("hex")) && !shown.includes(b58encode(Uint8Array.from(full))));
  const dir = mkdtempSync(join(tmpdir(), "sdk-"));
  const f = join(dir, "k.json");
  writeFileSync(f, JSON.stringify(full));
  chmodSync(f, 0o644);
  assert.throws(() => loadKeyFile(f), /readable by others/);
  chmodSync(f, 0o600);
  assert.equal(loadKeyFile(f).address, k.address);
  const sig = k.sign(Buffer.from("hi"));
  assert.ok(verify(k.publicKey, Buffer.from("hi"), sig));
});

test("window JSON encoders match the repo's vector-tested encoders", () => {
  const nonce = 7n, fee = 1234n;
  const sign = (t: any) => { const b = encodeWindowBody(t)!; const sb = signingBytes(CHAIN, k.publicKey, nonce, fee, b); return Buffer.from([...sb, ...k.sign(framed(sb))]).toString("hex"); };
  const post = { kind: "Post", postKind: 1, parent: "ab".repeat(32), channel: null, body: "hello town", blobUri: "", blobHash: null, thoughtRef: "req_abc123" };
  assert.equal(sign(post), Buffer.from(signTx(w, CHAIN, nonce, fee, { kind: "Post", postKind: 1, parent: Buffer.from("ab".repeat(32), "hex"), body: "hello town", thoughtRef: "req_abc123" })).toString("hex"));
  const dm = { kind: "SendMessage", to: peer.address, mode: 1, replyTo: null, pay: "5", reclaimAfter: 0, bodyHex: "0102ff", thoughtRef: "" };
  assert.equal(sign(dm), Buffer.from(signTx(w, CHAIN, nonce, fee, { kind: "SendMessage", to: peer.publicKey, mode: 1, pay: 5n, body: Uint8Array.from([1, 2, 255]) })).toString("hex"));
  const follow = { kind: "Follow", items: [{ target: peer.address, on: true }] };
  assert.equal(sign(follow), Buffer.from(signTx(w, CHAIN, nonce, fee, { kind: "Follow", items: [{ target: peer.publicKey, on: true }] })).toString("hex"));
  const tr = { kind: "Transfer", to: peer.address, amount: "1000000000" };
  assert.equal(sign(tr), Buffer.from(signTx(w, CHAIN, nonce, fee, { kind: "Transfer", to: peer.publicKey, amount: 1_000_000_000n })).toString("hex"));
  // RegisterAgent (kind 1) and Call (kind 32)
  const reg = encodeWindowBody({ kind: "RegisterAgent", name: "Bot", uri: "https://x/card" })!;
  assert.equal(Buffer.from(signingBytes(CHAIN, k.publicKey, nonce, fee, reg)).toString("hex"), Buffer.from(registerAgentSigning(CHAIN, k.publicKey, nonce, fee, "Bot", "https://x/card")).toString("hex"));
  const call = encodeWindowBody({ kind: "Call", contract: peer.address, value: "9", gasLimit: "100000", args: "0568656c6c6f" })!;
  assert.equal(Buffer.from(signingBytes(CHAIN, k.publicKey, nonce, fee, call)).toString("hex"), Buffer.from(contractSigningBytes(CHAIN, k.publicKey, nonce, fee, { kind: "Call", contract: peer.address, value: 9n, gasLimit: 100000n, args: Buffer.from("0568656c6c6f", "hex") })).toString("hex"));
  const code = randomBytes(40);
  const dep = encodeWindowBody({ kind: "DeployCode", gasLimit: "5000", code: code.toString("hex") })!;
  assert.equal(Buffer.from(signingBytes(CHAIN, k.publicKey, nonce, fee, dep)).toString("hex"), Buffer.from(contractSigningBytes(CHAIN, k.publicKey, nonce, fee, { kind: "DeployCode", gasLimit: 5000n, code })).toString("hex"));
  assert.equal(encodeWindowBody({ kind: "BridgeLock" }), null, "unknown kinds go to /v1/tx/encode");
});

test("limits: unset refuses, caps hold, booking accumulates", () => {
  const none = new Limits({});
  assert.throws(() => none.check("sol", 1n), LimitError);
  const l = new Limits({ maxSolPerTx: 100n, maxSolPerDay: 150n, maxCadrePerTx: "unlimited", maxCadrePerDay: "unlimited", maxInferenceLamportsPerDay: 10n });
  assert.throws(() => l.check("sol", 101n), /per-tx/);
  l.check("sol", 100n); l.book("sol", 100n);
  assert.throws(() => l.check("sol", 60n), /today/);
  l.check("cadre", 10n ** 18n);
  assert.throws(() => l.check("inference", 11n), /today/);
  const env = Limits.fromEnv({ CADRE_MAX_SOL_PER_TX: "0.05", CADRE_MAX_SOL_PER_DAY: "0.2", CADRE_MAX_INFERENCE_SOL_PER_DAY: "0.01" });
  assert.equal(env.maxSolPerTx, 50_000_000n);
  assert.equal(env.maxInferenceLamportsPerDay, 10_000_000n);
});

test("solana: signs only its own slot of a two-signer legacy message", () => {
  const other = keyFromSeed(createHash("sha256").update("fee-payer").digest());
  // legacy message: header (2 sigs, 0 ro signed, 1 ro unsigned), 3 keys, blockhash, 0 instructions
  const msg = Uint8Array.from([2, 0, 1, 3, ...other.publicKey, ...k.publicKey, ...new Uint8Array(32), ...new Uint8Array(32).fill(7), 0]);
  const tx = Buffer.from([2, ...new Uint8Array(128), ...msg]).toString("base64");
  assert.deepEqual(signersOf(tx), [other.address, k.address]);
  const s = Buffer.from(signSolanaTx(tx, k.address, k.sign), "base64");
  assert.ok(s.subarray(1, 65).every((x) => x === 0), "the other signer's slot stays empty");
  assert.ok(verify(k.publicKey, msg, s.subarray(65, 129)));
  assert.throws(() => signSolanaTx(tx, peer.address, peer.sign), /not a signer/);
});
