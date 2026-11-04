// RH (EVM) side of the relayer and the header-oracle poster. The RH chain is test/fake-rh.ts
// behind a viem custom transport (so the real ViemEvm client encodes, signs and sends), the
// Cadre chain is test/fake-node-v2.ts. No ports, no network.

import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, describe, it } from "node:test";
import { Keypair, PublicKey } from "@solana/web3.js";
import { encodeAbiParameters, keccak256, toHex, concat } from "viem";
import type { Address, Hex } from "viem";
import { generatePrivateKey } from "viem/accounts";
import { KIND, bridgeLockV2Body, decodeTx, signTx } from "../src/agent-tx.ts";
import { encodePostJobBody, evmRecipient, orderJobIdV2 } from "../src/codec.ts";
import { parseFinality } from "../src/evm/config.ts";
import { ViemEvm } from "../src/evm/io.ts";
import { OraclePoster, parseHeader } from "../src/evm/oracle.ts";
import { EvmRelayer, bodyFees } from "../src/evm/relayer.ts";
import { evmKey, pickSigners, releaseDigest, sortKeys } from "../src/evm/sign.ts";
import type { RelayerSide } from "../src/side.ts";
import type { Relayer } from "../src/relayer.ts";
import { FakeNodeV2 } from "./fake-node-v2.ts";
import { FakeRh } from "./fake-rh.ts";

// Both sides fit the daemon interface (compile-time check under `npm run typecheck`).
type _Fits = [Relayer extends RelayerSide ? true : never, EvmRelayer extends RelayerSide ? true : never];
const _fits: _Fits = [true, true];
void _fits;

const dir = mkdtempSync(join(tmpdir(), "rh-relayer-"));
after(() => rmSync(dir, { recursive: true, force: true }));
let files = 0;

const hex32 = (pk: PublicKey): Hex => `0x${pk.toBuffer().toString("hex")}`;

function world(o: { releaseMode?: "auto" | "signers" | "proof"; refundGraceMs?: number; bridgeAddrOnChain?: string; modes?: number } = {}) {
  const keys = [evmKey(generatePrivateKey()), evmKey(generatePrivateKey()), evmKey(generatePrivateKey())];
  const set = keys.map((k) => k.address);
  const rh = new FakeRh({ bridgeSigners: set, bridgeThreshold: 2, storeSigners: set, storeThreshold: 2, oracleSigners: set, oracleThreshold: 2, modes: o.modes });
  const attester = Keypair.generate();
  const agent = Keypair.generate();
  const node = new FakeNodeV2({ attesters: [attester.publicKey], K: 3,
    domains: [{ domain: 2, extChainId: 46630n, bridgeAddr: (o.bridgeAddrOnChain ?? rh.bridge).toLowerCase(), paused: false }] });
  const held = [keys[0], keys[2]]; // the relayer holds 2 of 3
  const evm = new ViemEvm({ transport: rh.transport(), bridge: rh.bridge, storefront: rh.store, oracle: rh.oracle, signer: keys[0], finality: 2, receiptTimeoutMs: 5000 });
  let clock = 1_000_000;
  const logs: string[] = [];
  const relayer = new EvmRelayer({
    node: node.rpc(), evm, domain: 2, bridge: rh.bridge, storefront: rh.store, chainSigners: [attester], agentKey: agent, attesters: held,
    stateFile: join(dir, `state-${files++}.json`), expectedChainId: 46630n, releaseMode: o.releaseMode ?? "auto",
    refundGraceMs: o.refundGraceMs ?? 1000, now: () => clock, log: (m) => logs.push(m),
  });
  const poster = new OraclePoster({ node: node.rpc(), evm, oracle: rh.oracle, attesters: held, every: 10, domain: 2,
    isProcessed: (n) => evm.isProcessed(n, "latest"), log: () => {} });
  return { rh, node, relayer, poster, keys, held, attester, agent, evm, logs, advance: (ms: number) => (clock += ms) };
}

type W = ReturnType<typeof world>;

/** An RH deposit unlocked and final on the chain; returns the chain recipient. */
async function bridgeIn(w: W, amount: bigint): Promise<Keypair> {
  const user = "0x1111111111111111111111111111111111111111" as Address;
  w.rh.mint(user, amount);
  const r = Keypair.generate();
  w.rh.deposit(user, amount, hex32(r.publicKey));
  w.rh.mine(2);
  const t = await w.relayer.tick();
  assert.equal(t.unlocked.length, 1, JSON.stringify(t.errors));
  w.node.mine(4);
  return r;
}

function lock(w: W, from: Keypair, to: Address, amountChain: bigint): void {
  const a = w.node.acct(from.publicKey.toBase58());
  const tx = signTx(from, { chainId: w.node.chainId, from: from.publicKey, nonce: a.nonce, fee: 0n, kind: KIND.BridgeLockV2 }, bridgeLockV2Body(2, evmRecipient(to), amountChain));
  w.node.handle("sendTx", { tx: tx.toString("base64") });
  w.node.mine(1);
  assert.deepEqual(w.node.refused, []);
}

function orderBody(evaluator: PublicKey, tokens: bigint, pBps = 500n) {
  const p = (tokens * pBps) / 10_000n;
  const workerPay = (tokens - p) * 1000n;
  const protocolFee = p * 1000n;
  const body = encodePostJobBody({ m: 4, n: 8, k: 64, config: Buffer.alloc(52, 1), hashA: Buffer.alloc(32, 2), hashB: Buffer.alloc(32, 3),
    dataUri: Buffer.from("blob:00"), workerPay, protocolFee, deadline: 100n, maxBlocks: 4, evaluators: [evaluator], threshold: 1 });
  return { body, workerPay, protocolFee };
}

describe("RH side: config and helpers", () => {
  it("RH_FINALITY parses tags and block counts", () => {
    assert.equal(parseFinality(undefined), "safe");
    assert.equal(parseFinality("finalized"), "finalized");
    assert.equal(parseFinality("12"), 12);
    assert.throws(() => parseFinality("soon"));
  });

  it("EIP-712 Release digest equals the Solidity construction (solady EIP712, FORMATS.md 3.3)", () => {
    const chainId = 46630n;
    const bridge = "0x5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a" as Address;
    const recipient = "0x00000000000000000000000000000000000000c1" as Address;
    const dt = keccak256(toHex("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"));
    const sep = keccak256(encodeAbiParameters([{ type: "bytes32" }, { type: "bytes32" }, { type: "bytes32" }, { type: "uint256" }, { type: "address" }],
      [dt, keccak256(toHex("CadreBridge")), keccak256(toHex("2")), chainId, bridge]));
    const th = keccak256(toHex("Release(uint64 nonce,address recipient,uint64 amount)"));
    const sh = keccak256(encodeAbiParameters([{ type: "bytes32" }, { type: "uint64" }, { type: "address" }, { type: "uint64" }], [th, 7n, recipient, 1234n]));
    assert.equal(releaseDigest(chainId, bridge, 7n, recipient, 1234n), keccak256(concat(["0x1901", sep, sh])));
  });

  it("signer bundles: only members, exactly threshold, ascending by address", () => {
    const ks = [evmKey(generatePrivateKey()), evmKey(generatePrivateKey()), evmKey(generatePrivateKey())];
    const outsider = evmKey(generatePrivateKey());
    assert.equal(pickSigners([outsider, ks[0]], ks.map((k) => k.address), 2), null);
    const got = pickSigners([ks[2], outsider, ks[1], ks[0]], ks.map((k) => k.address), 2)!;
    assert.equal(got.length, 2);
    assert.ok(BigInt(got[0].address) < BigInt(got[1].address));
    assert.deepEqual(sortKeys(got).map((k) => k.address), got.map((k) => k.address));
  });

  it("bodyFees reads W and P after the data uri", () => {
    const { body, workerPay, protocolFee } = orderBody(Keypair.generate().publicKey, 1_000_000n);
    assert.deepEqual(bodyFees(body), { workerPay, protocolFee });
  });

  it("refuses a Solana relayer state file", () => {
    const f = join(dir, "solana-state.json");
    writeFileSync(f, JSON.stringify({ deposit: "3", withdrawal: "0", inflight: {} }));
    const w = world();
    assert.throws(() => new EvmRelayer({ ...w.relayer.o, expectedChainId: undefined, stateFile: f }), /not an RH relayer state file/);
  });
});

describe("RH -> chain", () => {
  it("a deposit is unlocked only once it is in the finality block, with the v2 message; invariant exact", async () => {
    const w = world();
    const user = "0x1111111111111111111111111111111111111111" as Address;
    w.rh.mint(user, 5_000_000n);
    const r = Keypair.generate();
    w.rh.deposit(user, 2_500_000n, hex32(r.publicKey));
    let t = await w.relayer.tick();
    assert.deepEqual(t.unlocked, [], "not yet in the finality block");
    assert.equal(t.invariant?.delta, "0");
    w.rh.mine(2);
    t = await w.relayer.tick();
    assert.deepEqual(t.unlocked, ["0"], JSON.stringify(t.errors));
    const tx = decodeTx(w.node.pool[0]);
    assert.equal(tx.kind, KIND.BridgeUnlockV2);
    assert.equal(tx.fee, 0n);
    assert.ok(tx.body.kind === 40 && tx.body.domain === 2 && tx.body.extNonce === 0n && tx.body.amountExt === 2_500_000n && tx.body.jobOrder === null);
    w.node.mine(1);
    assert.deepEqual(w.node.refused, [], "the chain accepted the attester signature over cadre:deposit:v2");
    assert.equal(w.node.acct(r.publicKey.toBase58()).balance, 2_500_000_000n);
    t = await w.relayer.tick();
    assert.deepEqual(t.unlocked, [], "no second unlock");
    assert.equal(t.invariant?.delta, "0");
    assert.equal(t.invariant?.strictFormHolds, true);
    w.node.mine(3);
    await w.relayer.tick();
    assert.equal(w.relayer.state.deposit, 1n, "cursor passes once final on the chain");
  });

  it("refuses to sign when the chain's domain names another bridge, or v2 is off", async () => {
    const w = world({ bridgeAddrOnChain: "0x9999999999999999999999999999999999999999" });
    const user = "0x1111111111111111111111111111111111111111" as Address;
    w.rh.mint(user, 10n);
    w.rh.deposit(user, 10n, hex32(Keypair.generate().publicKey));
    w.rh.mine(2);
    let t = await w.relayer.tick();
    assert.match(t.refused ?? "", /names bridge/);
    assert.deepEqual(t.unlocked, []);
    w.node.domains[0].bridgeAddr = w.rh.bridge.toLowerCase();
    w.node.v2Active = false;
    t = await w.relayer.tick();
    assert.match(t.refused ?? "", /not active/);
  });

  it("a vault deficit refuses everything", async () => {
    const w = world();
    await bridgeIn(w, 1_000_000n);
    w.rh.s.balances[w.rh.bridge.toLowerCase()] -= 1n;
    w.rh.mine(3);
    const t = await w.relayer.tick();
    assert.match(t.refused ?? "", /invariant broken/);
    assert.equal(t.invariant?.delta, "-1000");
  });
});

describe("chain -> RH", () => {
  it("signer mode: a final BridgeLockV2 is released once with 2-of-3 sorted EIP-712 signatures", async () => {
    const w = world();
    const r = await bridgeIn(w, 3_000_000n);
    const dest = "0x00000000000000000000000000000000000000d1" as Address;
    lock(w, r, dest, 1_000_000_000n);
    let t = await w.relayer.tick();
    assert.deepEqual(t.waitingDepth, ["0"]);
    assert.equal(t.invariant?.delta, "0", "pending withdrawal counted");
    w.node.mine(3);
    t = await w.relayer.tick();
    assert.deepEqual(t.released, ["0"], JSON.stringify(t.errors));
    assert.equal(w.rh.bal(dest), 1_000_000n);
    const call = w.rh.sent.find((s) => s.fn === "release")!;
    assert.equal((call.args[3] as Hex[]).length, 2);
    w.rh.mine(3);
    t = await w.relayer.tick();
    assert.deepEqual(t.released, []);
    assert.equal(w.rh.sent.filter((s) => s.fn === "release").length, 1, "released exactly once");
    assert.equal(t.invariant?.delta, "0");
    assert.equal(w.relayer.state.withdrawal, 1n);
  });

  it("proof mode: waits for the header oracle, then releaseProof against its latest final header", async () => {
    const w = world({ releaseMode: "proof" });
    const r = await bridgeIn(w, 3_000_000n);
    const dest = "0x00000000000000000000000000000000000000d2" as Address;
    lock(w, r, dest, 2_000_000_000n);
    w.node.mine(4);
    let t = await w.relayer.tick();
    assert.deepEqual(t.waitingOracle, ["0"]);
    const p = await w.poster.tick();
    assert.ok(p.posted, JSON.stringify(p.errors));
    assert.equal(p.posted!.height, w.node.finalized);
    t = await w.relayer.tick();
    assert.deepEqual(t.releasedViaProof, ["0"], JSON.stringify(t.errors));
    assert.equal(w.rh.bal(dest), 2_000_000n);
  });
});

describe("header oracle poster", () => {
  it("posts every N final blocks and early for a waiting withdrawal; checks blake3(header) and offsets", async () => {
    const w = world();
    w.node.mine(5);
    let p = await w.poster.tick();
    assert.equal(p.posted?.reason, "first header");
    const first = p.posted!.height;
    const h = parseHeader(Buffer.from(w.node.blocks[first].header));
    assert.equal(h.height, BigInt(first));
    w.node.mine(5);
    p = await w.poster.tick();
    assert.equal(p.posted, null, "fewer than 10 new final blocks");
    w.node.mine(5);
    p = await w.poster.tick();
    assert.equal(p.posted?.reason, "every 10");
    // a withdrawal above the oracle's header triggers a post before the cadence
    const r = await bridgeIn(w, 1_000_000n);
    lock(w, r, "0x00000000000000000000000000000000000000d3", 1_000_000_000n);
    w.node.mine(3);
    p = await w.poster.tick();
    assert.match(p.posted?.reason ?? "", /withdrawal 0 waiting/);
    const sent = w.rh.sent.filter((s) => s.fn === "postFinalHeader");
    assert.equal(sent.length, 3);
    // a node whose header does not hash to its claimed hash is refused
    w.node.mine(12);
    w.node.blocks[w.node.finalized].header[200] ^= 1;
    p = await w.poster.tick();
    assert.match(p.errors.join(), /blake3/);
  });
});

describe("orders", () => {
  it("order deposit -> unlock with the PostJob body -> job completed -> finalizeOrder (Result) -> settleShare", async () => {
    const w = world();
    const evaluator = Keypair.generate();
    const buyer = "0x00000000000000000000000000000000000000b1" as Address;
    const r = Keypair.generate();
    const { body, workerPay, protocolFee } = orderBody(evaluator.publicKey, 1_000_000n);
    const n = w.rh.buyJob(buyer, 1_000_000n, hex32(r.publicKey), body, workerPay, protocolFee, 1000n);
    w.rh.mine(2);
    let t = await w.relayer.tick();
    assert.deepEqual(t.unlocked, [n.toString()], JSON.stringify(t.errors));
    const tx = decodeTx(w.node.pool[0]);
    assert.ok(tx.body.kind === 40 && tx.body.jobOrder !== null && encodePostJobBody(tx.body.jobOrder).equals(body));
    w.node.mine(1);
    assert.deepEqual(w.node.refused, [], "the chain accepted cadre:deposit-order:v2");
    const jobId = orderJobIdV2(2, n).toString("hex");
    assert.equal(w.node.jobs.get(jobId)?.status, "open");
    const deliverer = Keypair.generate();
    w.node.closeJob(jobId, { deliverer: deliverer.publicKey, resultHash: Buffer.alloc(32, 9), uri: "blob:result" });
    t = await w.relayer.tick();
    assert.deepEqual(t.finalized, [], "job close not K deep yet");
    w.node.mine(3);
    t = await w.relayer.tick();
    assert.deepEqual(t.finalized, [n.toString()], JSON.stringify(t.errors));
    const o = w.rh.s.orders[n.toString()];
    assert.equal(o.status, 2);
    assert.equal(o.deliverer, hex32(deliverer.publicKey));
    t = await w.relayer.tick();
    assert.equal(w.rh.s.orders[n.toString()].shareSettled, true);
    assert.deepEqual(w.relayer.state.orders, []);
  });

  it("an expired job expires the order (Expired signatures)", async () => {
    const w = world();
    const { body, workerPay, protocolFee } = orderBody(Keypair.generate().publicKey, 2_000_000n);
    const n = w.rh.buyJob("0x00000000000000000000000000000000000000b2", 2_000_000n, hex32(Keypair.generate().publicKey), body, workerPay, protocolFee, 0n);
    w.rh.mine(2);
    await w.relayer.tick();
    w.node.mine(1);
    w.node.closeJob(orderJobIdV2(2, n).toString("hex"), "expire");
    w.node.mine(3);
    const t = await w.relayer.tick();
    assert.deepEqual(t.expired, [n.toString()], JSON.stringify(t.errors));
    assert.equal(w.rh.s.orders[n.toString()].status, 3);
  });

  it("an order whose body does not hash to bodyHash is never signed", async () => {
    const w = world();
    const { body, workerPay, protocolFee } = orderBody(Keypair.generate().publicKey, 1_000_000n);
    w.rh.buyJob("0x00000000000000000000000000000000000000b3", 1_000_000n, hex32(Keypair.generate().publicKey), body, workerPay, protocolFee, 0n, { badHash: true });
    w.rh.mine(2);
    const t = await w.relayer.tick();
    assert.deepEqual(t.unlocked, []);
    assert.ok(t.errors.some((e) => /bodyHash/.test(e)), String(t.errors));
    assert.equal(w.node.pool.length, 0);
  });

  it("refund: P below the chain floor holds the order, after the grace refundDeposit + markRefunded + settleShare; never unlocked", async () => {
    const w = world({ refundGraceMs: 1000 });
    w.node.minProtocolFee = 10n ** 15n;
    const buyer = "0x00000000000000000000000000000000000000b4" as Address;
    const { body, workerPay, protocolFee } = orderBody(Keypair.generate().publicKey, 1_000_000n);
    const n = w.rh.buyJob(buyer, 1_000_000n, hex32(Keypair.generate().publicKey), body, workerPay, protocolFee, 500n);
    w.rh.mine(2);
    let t = await w.relayer.tick();
    assert.deepEqual(t.needsRefund, [n.toString()]);
    assert.deepEqual(t.unlocked, []);
    assert.equal(w.relayer.state.needsRefund[n.toString()].reason, "protocol_fee_below_min");
    w.advance(1500);
    t = await w.relayer.tick();
    assert.deepEqual(t.refunded, [n.toString()], JSON.stringify(t.errors));
    assert.equal(w.rh.bal(buyer), 1_000_000n);
    const o = w.rh.s.orders[n.toString()];
    assert.equal(o.status, 4);
    assert.equal(o.shareSettled, true);
    // the floor comes back down: the refunded deposit is still never unlocked
    w.node.minProtocolFee = 2n;
    w.rh.mine(3);
    t = await w.relayer.tick();
    assert.deepEqual(t.unlocked, []);
    assert.equal(t.invariant?.delta, "0");
    assert.equal(w.relayer.state.deposit, 1n, "cursor passes the refunded deposit");
  });
});
