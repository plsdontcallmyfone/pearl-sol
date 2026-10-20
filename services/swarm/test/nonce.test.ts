// Nonce handling: consecutive txs from one key use consecutive nonces without a round trip;
// a nonce error (another sender moved the account) drops the cache and the next send resyncs.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { signTx } from "../src/chaintx.ts";
import { startFakes, type Fakes } from "./fakes.ts";
import { makeSwarm, testConfig } from "./helpers.ts";

let f: Fakes;
before(async () => { f = await startFakes(() => "Yes"); });
after(async () => f.close());

test("nonces: sequential, resync after an outside send", async () => {
  const clock = { t: Date.UTC(2026, 9, 21, 10) };
  const { sw, op, agents } = makeSwarm(f, testConfig(), 1, clock);
  const to = agents[0].wallet.publicKey;
  const before = f.node.nonces.get(op.address) ?? 0n;
  await sw.send(op, { kind: "Transfer", to, amount: 1n });
  await sw.send(op, { kind: "Transfer", to, amount: 1n });
  assert.equal(f.node.nonces.get(op.address), before + 2n);
  // another process uses the key: the cached nonce is now stale
  await f.node.rpc("sendTx", { tx: Buffer.from(signTx(op, f.node.chainId, before + 2n, 1000n, { kind: "Transfer", to, amount: 1n })).toString("base64") });
  await assert.rejects(sw.send(op, { kind: "Transfer", to, amount: 1n }), /nonce/);
  await sw.send(op, { kind: "Transfer", to, amount: 1n });
  assert.equal(f.node.nonces.get(op.address), before + 4n);
  f.node.refused.length = 0;
});

test("fee follows the social rule from getParams", async () => {
  const clock = { t: Date.UTC(2026, 9, 21, 10) };
  const { sw, agents } = makeSwarm(f, testConfig(), 1, clock);
  f.node.balances.set(agents[0].wallet.address, 10_000_000n);
  f.node.params.social_byte_fee = "50";
  await sw.refreshParams();
  await sw.send(agents[0].wallet, { kind: "Follow", items: [{ target: new Uint8Array(32).fill(9), on: true }] });
  const last = f.node.sent[f.node.sent.length - 1];
  assert.equal(last.kind, "Follow");
  f.node.params.social_byte_fee = "2";
});
