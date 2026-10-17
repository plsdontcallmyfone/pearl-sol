// Relayer tests: Solana side in LiteSVM with the real bridge.so and
// storefront.so (token/target/deploy), agent chain = test/fake-node.ts (docs/RPC.md
// shapes, in-process). Build the programs first:
//   (cd token && anchor build -p bridge && anchor build -p storefront)
//
// Storefront orders are staged without pump: a normal bridge deposit, then its
// Deposit.sender is rewritten to the storefront trader PDA and an Order account
// is written at ["order", nonce] in the exact storefront layout. finalize_order
// and release then run for real against the programs.

import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";
import { blake3 } from "@noble/hashes/blake3";
import { Keypair, PublicKey, TransactionInstruction } from "@solana/web3.js";
import { LiteSVM } from "litesvm";
import { KIND, bridgeLockBody, signTx } from "../src/agent-tx.ts";
import { RpcChain } from "../src/chain.ts";
import {
  BRIDGE_PROGRAM_ID, ORDER_LAYOUT, ORDER_STATUS_FINALIZED, ORDER_STATUS_PENDING, STOREFRONT_PROGRAM_ID, TOKEN_2022_PROGRAM_ID,
  accDisc, ata, bridgeConfigPda, bridgeDepositIx, bridgeInitializeIx, bridgeProcessedPda, createAtaIdempotentIx, decodeOrder,
  depositPda, encodePostJobBody, orderJobId, orderPda, sha256, storefrontInitializeIx, tokenAccountAmount, traderPda, u16le, u32le,
  u64le, vaultAta,
} from "../src/codec.ts";
import type { PostJobBody } from "../src/codec.ts";
import { REPO_ROOT } from "../src/config.ts";
import { startHealth } from "../src/health.ts";
import { NodeRpc, httpTransport } from "../src/node.ts";
import { Relayer } from "../src/relayer.ts";
import { loadState, saveState, emptyState } from "../src/state.ts";
import { postJobFromJson } from "./vectors.ts";
import { FakeNode } from "./fake-node.ts";
import { LiteSvmChain } from "./litesvm-chain.ts";

const BRIDGE_SO = join(REPO_ROOT, "token/target/deploy/bridge.so");
const STOREFRONT_SO = join(REPO_ROOT, "token/target/deploy/storefront.so");
const SOL = 1_000_000_000n;
const K = 30;

// Vector keys (chain/agent-types/tests/vectors.rs: seed(i) = blake3("pearl-sol:vector-seed" || i)).
const vseed = (i: number) => Keypair.fromSeed(blake3(Buffer.concat([Buffer.from("pearl-sol:vector-seed"), Buffer.from([i])])));
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const vtx: any[] = JSON.parse(readFileSync(join(REPO_ROOT, "chain/agent-types/vectors/tx.json"), "utf8"));
const vec = (name: string) => vtx.find((t) => t.name === name);
const RELAYER_KEY = Keypair.fromSeed(Buffer.from(vec("bridge_unlock").seed, "hex"));

interface World {
  token: LiteSvmChain;
  node: FakeNode;
  mint: PublicKey;
  mintAuthority: Keypair;
  payer: Keypair;
  bridgeSigner: Keypair;
  attesters: Keypair[];
  sfAttester: Keypair;
  stateFile: string;
}

async function world(o: { attesters?: Keypair[]; threshold?: number; relayerNonce?: bigint } = {}): Promise<World> {
  const payer = Keypair.generate();
  const bridgeSigner = Keypair.generate();
  const sfAttester = Keypair.generate();
  const mintAuthority = Keypair.generate();
  const attesters = o.attesters ?? [vseed(30)];

  const svm = new LiteSVM();
  svm.addProgramFromFile(BRIDGE_PROGRAM_ID, BRIDGE_SO);
  svm.addProgramFromFile(STOREFRONT_PROGRAM_ID, STOREFRONT_SO);
  svm.airdrop(payer.publicKey, 100n * SOL);
  svm.airdrop(mintAuthority.publicKey, 10n * SOL);
  svm.airdrop(sfAttester.publicKey, SOL);
  const token = new LiteSvmChain("token", svm);
  const mint = Keypair.generate().publicKey;
  svm.setAccount(mint, { lamports: Number(svm.minimumBalanceForRentExemption(82n)), data: new Uint8Array(82), owner: TOKEN_2022_PROGRAM_ID, executable: false });

  const node = new FakeNode({
    attesters: attesters.map((k) => k.publicKey), threshold: o.threshold ?? 1, finalityK: K,
    genesis: o.relayerNonce ? { [RELAYER_KEY.publicKey.toBase58()]: { nonce: o.relayerNonce } } : {},
  });
  const stateFile = join(mkdtempSync(join(tmpdir(), "relayer-test-")), "state.json");
  const w = { token, node, mint, mintAuthority, payer, bridgeSigner, attesters, sfAttester, stateFile };

  const initMint = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [{ pubkey: mint, isSigner: false, isWritable: true }],
    data: Buffer.concat([Buffer.from([20, 6]), mintAuthority.publicKey.toBuffer(), Buffer.from([0])]),
  });
  await token.send([initMint], payer);
  await token.send([bridgeInitializeIx(payer.publicKey, mint, [bridgeSigner.publicKey], 1)], payer);
  await token.send([storefrontInitializeIx(payer.publicKey, [sfAttester.publicKey], 1)], payer);
  return w;
}

function relayer(w: World, extra: Partial<ConstructorParameters<typeof Relayer>[0]> = {}): Relayer {
  return new Relayer({
    token: w.token,
    node: w.node.client(),
    signers: [...w.attesters, w.bridgeSigner, w.sfAttester],
    tokenPayer: w.payer,
    agentKey: RELAYER_KEY,
    stateFile: w.stateFile,
    log: () => {},
    ...extra,
  });
}

/** Makes the bridge's next deposit nonce `n` (and the relayer start there), as if n deposits happened earlier. */
function startAtNonce(w: World, n: bigint): void {
  const k = bridgeConfigPda();
  const a = w.token.svm.getAccount(k)!;
  const data = Buffer.from(a.data);
  data.writeBigUInt64LE(n, 136);
  w.token.svm.setAccount(k, { ...a, data });
  saveState(w.stateFile, { ...emptyState(), deposit: n, withdrawal: 0n });
}

/** A Solana user with `amount` SPL deposits it all to `chainRecipient`. */
async function userDeposits(w: World, amount: bigint, chainRecipient: PublicKey, nonce: bigint): Promise<Keypair> {
  const user = Keypair.generate();
  w.token.svm.airdrop(user.publicKey, SOL);
  await mintTo(w, ata(user.publicKey, w.mint), amount, [createAtaIdempotentIx(user.publicKey, user.publicKey, w.mint)], user);
  await w.token.send([bridgeDepositIx({ payer: user.publicKey, sender: user.publicKey, mint: w.mint, nonce, amount, agentRecipient: chainRecipient })], user);
  return user;
}

async function mintTo(w: World, dest: PublicKey, amount: bigint, pre: TransactionInstruction[] = [], payer = w.payer): Promise<void> {
  const ix = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: w.mint, isSigner: false, isWritable: true },
      { pubkey: dest, isSigner: false, isWritable: true },
      { pubkey: w.mintAuthority.publicKey, isSigner: true, isWritable: false },
    ],
    data: Buffer.concat([Buffer.from([7]), u64le(amount)]),
  });
  await w.token.send([...pre, ix], payer, [w.mintAuthority]);
}

/** Stages a storefront order deposit (see the header comment). Returns the buyer. */
async function orderDeposit(w: World, nonce: bigint, body: PostJobBody, recipient: PublicKey, o: { badHash?: boolean } = {}): Promise<Keypair> {
  const amount = (body.workerPay + body.protocolFee) / 1000n;
  await userDeposits(w, amount, recipient, nonce);
  const dk = depositPda(nonce);
  const dep = w.token.svm.getAccount(dk)!;
  const dd = Buffer.from(dep.data);
  traderPda().toBuffer().copy(dd, 16);
  w.token.svm.setAccount(dk, { ...dep, data: dd });

  const buyer = Keypair.generate();
  const [opda, bump] = PublicKey.findProgramAddressSync([Buffer.from("order"), u64le(nonce)], STOREFRONT_PROGRAM_ID);
  const L = ORDER_LAYOUT;
  const d = Buffer.alloc(L.LEN);
  accDisc("Order").copy(d, 0);
  d[8] = 1; // version
  d[L.STATUS] = ORDER_STATUS_PENDING;
  d[11] = bump;
  d.writeBigUInt64LE(nonce, L.DEPOSIT_NONCE);
  orderJobId(nonce).copy(d, L.JOB_ID);
  buyer.publicKey.toBuffer().copy(d, L.BUYER);
  recipient.toBuffer().copy(d, L.RECIPIENT);
  const hash = sha256(encodePostJobBody(body));
  if (o.badHash) hash[0] ^= 1;
  hash.copy(d, L.BODY_HASH);
  d.writeBigUInt64LE(amount, L.TOKENS);
  d.writeBigUInt64LE(body.workerPay, L.WORKER_PAY);
  d.writeBigUInt64LE(body.protocolFee, L.PROTOCOL_FEE);
  const S = L.SPEC;
  Buffer.concat([u32le(body.m), u32le(body.n), u32le(body.k), body.config, body.hashA, body.hashB, u64le(body.deadline),
    u32le(body.maxBlocks), Buffer.from([body.threshold, body.evaluators.length])]).copy(d, S);
  body.evaluators.forEach((e, i) => e.toBuffer().copy(d, S + 142 + 32 * i));
  u16le(body.dataUri.length).copy(d, S + 398);
  body.dataUri.copy(d, S + 400);
  w.token.svm.setAccount(opda, { lamports: Number(w.token.svm.minimumBalanceForRentExemption(BigInt(L.LEN))), data: d, owner: STOREFRONT_PROGRAM_ID, executable: false });
  assert.ok(orderPda(nonce).equals(opda));
  return buyer;
}

/** A chain user locks `amount` (chain units) back to `solanaRecipient` (BridgeLock tx through the fake node). */
async function lock(w: World, user: Keypair, amount: bigint, solanaRecipient: PublicKey): Promise<void> {
  const nonce = w.node.account(user.publicKey).nonce;
  const tx = signTx(user, { chainId: w.node.chainId, from: user.publicKey, nonce, fee: 1000n, kind: KIND.BridgeLock }, bridgeLockBody(solanaRecipient, amount));
  await w.node.client().sendTx(tx);
}

const spl = (w: World, k: PublicKey): bigint => {
  const a = w.token.svm.getAccount(k);
  return a && a.data.length >= 72 ? tokenAccountAmount(a.data) : 0n;
};
const hexOf = (b: Buffer) => b.toString("hex");

/** Deposit, unlock and finalize `amountSpl` to a fresh chain account; returns it. */
async function funded(w: World, rl: Relayer, amountSpl: bigint, nonce: bigint): Promise<Keypair> {
  const u = Keypair.generate();
  await userDeposits(w, amountSpl, u.publicKey, nonce);
  const t = await rl.tick();
  assert.deepEqual(t.unlocked, [nonce.toString()], String(t.errors));
  w.node.mine();
  w.node.mineN(K);
  await rl.tick();
  return u;
}

// ------------------------------------------------------------------ tests

describe("Solana -> chain", () => {
  it("plain deposit: BridgeUnlock matches the vector byte for byte, cursor waits for K", async () => {
    const w = await world({ attesters: [vseed(30), vseed(31)], threshold: 2 });
    startAtNonce(w, 42n);
    const v = vec("bridge_unlock");
    const rl = relayer(w);
    await userDeposits(w, BigInt(v.body.amount_spl), new PublicKey(v.body.recipient), 42n);

    let t = await rl.tick();
    assert.equal(t.refused, null, String(t.errors));
    assert.deepEqual(t.unlocked, ["42"]);
    assert.equal(w.node.mempool.length, 1);
    assert.equal(hexOf(w.node.mempool[0]), v.tx, "relayer tx == chain/agent-types/vectors/tx.json bridge_unlock");

    // Next tick before mining: in flight, nothing resubmitted; invariant counts it as pending.
    t = await rl.tick();
    assert.deepEqual(t.unlocked, []);
    assert.equal(t.invariant!.pendingDepositSpl, v.body.amount_spl);
    assert.equal(w.node.mempool.length, 1);

    w.node.mine();
    assert.equal(w.node.account(v.body.recipient).balance, BigInt(v.body.amount_spl) * 1000n);
    t = await rl.tick();
    assert.ok(t.invariant!.ok && t.invariant!.strictFormHolds);
    assert.deepEqual(t.pendingDeposits, []);
    assert.equal(rl.state.deposit, 42n, "cursor does not pass a deposit that is not final");

    w.node.mineN(K);
    t = await rl.tick();
    assert.equal(rl.state.deposit, 43n);
    assert.deepEqual(rl.state.inflight, {});
  });

  it("order deposit: PostJob body rebuilt from the Order, BridgeUnlock(job_order) matches the vector", async () => {
    const w = await world({ attesters: [vseed(30)], threshold: 1, relayerNonce: 1n });
    startAtNonce(w, 43n);
    const v = vec("bridge_unlock_order");
    const body = postJobFromJson(v.body.job_order);
    await orderDeposit(w, 43n, body, new PublicKey(v.body.recipient));
    const rl = relayer(w);

    const t = await rl.tick();
    assert.equal(t.refused, null, String(t.errors));
    assert.deepEqual(t.unlocked, ["43"]);
    assert.equal(hexOf(w.node.mempool[0]), v.tx, "relayer tx == vectors bridge_unlock_order");
    assert.deepEqual(rl.state.orders, ["43"]);

    w.node.mine();
    const job = await w.node.client().getJob(hexOf(orderJobId(43n)));
    assert.equal(job?.status, "open");
    assert.equal(job?.poster, v.body.recipient);
    const t2 = await rl.tick();
    assert.ok(t2.invariant!.strictFormHolds, JSON.stringify(t2.invariant));
  });

  it("order deposit whose body does not hash to order.body_hash is never signed", async () => {
    const w = await world();
    const body = postJobFromJson(vec("bridge_unlock_order").body.job_order);
    await orderDeposit(w, 0n, body, Keypair.generate().publicKey, { badHash: true });
    const t = await relayer(w).tick();
    assert.deepEqual(t.unlocked, []);
    assert.ok(t.errors.some((e) => /body_hash/.test(e)), String(t.errors));
    assert.equal(w.node.mempool.length, 0);
    assert.ok(t.invariant!.ok, "the deposit is still accounted as pending");
  });

  it("an unlock that is dropped, or reorged out, is resubmitted (the chain's replay set keeps it single)", async () => {
    const w = await world();
    const rl = relayer(w);
    const u = Keypair.generate();
    await userDeposits(w, 1_000_000n, u.publicKey, 0n);
    await rl.tick();
    const first = hexOf(w.node.mempool[0]);
    w.node.mempool = []; // dropped by the node
    let t = await rl.tick();
    assert.deepEqual(t.unlocked, ["0"]);
    assert.equal(hexOf(w.node.mempool[0]), first, "same nonce + content = identical tx");

    w.node.mine();
    w.node.mineN(3);
    w.node.reorg(4); // unlock block gone, tx not returned to the mempool
    assert.equal(w.node.account(u.publicKey).balance, 0n);
    t = await rl.tick();
    assert.deepEqual(t.unlocked, ["0"]);
    w.node.mine();
    assert.equal(w.node.account(u.publicKey).balance, 1_000_000_000n);
  });
});

describe("chain -> Solana", () => {
  it("releases a withdrawal only once it is K deep, exactly once", async () => {
    const w = await world();
    const rl = relayer(w);
    const u = await funded(w, rl, 5_000_000n, 0n);
    const solUser = Keypair.generate().publicKey;

    await lock(w, u, 2_000_000_000n, solUser);
    w.node.mine();
    let t = await rl.tick();
    assert.deepEqual(t.waitingDepth, ["0"]);
    assert.deepEqual(t.released, []);
    assert.equal(t.invariant!.pendingWithdrawalSpl, "2000000");
    assert.ok(t.invariant!.ok);

    w.node.mineN(K - 1); // confirmations K - 1
    t = await rl.tick();
    assert.deepEqual([t.waitingDepth, t.released], [["0"], []], "refuses before depth K");
    assert.equal(spl(w, ata(solUser, w.mint)), 0n);

    w.node.mine(); // confirmations K
    t = await rl.tick();
    assert.equal(t.refused, null, String(t.errors));
    assert.deepEqual(t.released, ["0"]);
    assert.equal(spl(w, ata(solUser, w.mint)), 2_000_000n);
    assert.equal(spl(w, vaultAta(w.mint)), 3_000_000n);
    assert.equal(rl.state.released["0"].recipient, solUser.toBase58());

    t = await rl.tick();
    assert.deepEqual(t.released, []);
    assert.ok(t.invariant!.strictFormHolds, JSON.stringify(t.invariant));
    assert.equal(rl.state.withdrawal, 1n);
  });

  it("reorg: a withdrawal that disappears before K is never released; its nonce is released with the new content", async () => {
    const w = await world();
    const rl = relayer(w);
    const u = await funded(w, rl, 5_000_000n, 0n);
    const first = Keypair.generate().publicKey;
    const second = Keypair.generate().publicKey;

    await lock(w, u, 2_000_000_000n, first);
    w.node.mine();
    w.node.mineN(K - 2);
    let t = await rl.tick();
    assert.deepEqual(t.waitingDepth, ["0"]);

    w.node.reorg(K - 1); // the lock block and everything after it are replaced
    w.node.mineN(K + 2);
    t = await rl.tick();
    assert.deepEqual([t.pendingWithdrawals, t.released], [[], []]);
    assert.ok(t.invariant!.strictFormHolds);
    assert.equal(w.token.svm.getAccount(bridgeProcessedPda(0n))?.data.length ?? 0, 0);
    assert.equal(spl(w, ata(first, w.mint)), 0n);

    // A different withdrawal takes nonce 0 on the new branch.
    await lock(w, u, 1_000_000_000n, second);
    w.node.mine();
    w.node.mineN(K);
    t = await rl.tick();
    assert.deepEqual(t.released, ["0"], String(t.errors));
    assert.equal(spl(w, ata(second, w.mint)), 1_000_000n);
    assert.equal(spl(w, ata(first, w.mint)), 0n);
  });

  it("re-checks the event right before signing (reorg between snapshot and release)", async () => {
    const w = await world();
    const rl = relayer(w);
    const u = await funded(w, rl, 5_000_000n, 0n);
    const dest = Keypair.generate().publicKey;
    await lock(w, u, 1_000_000_000n, dest);
    w.node.mine();
    w.node.mineN(K);
    w.node.hook = (method, p) => {
      if (method === "getBridgeEvents" && p.limit === 1) {
        w.node.hook = null;
        w.node.reorg(K + 1); // simulated (the real node refuses reorgs deeper than K)
      }
    };
    const t = await rl.tick();
    assert.deepEqual(t.released, []);
    assert.ok(t.errors.some((e) => /vanished|changed/.test(e)), String(t.errors));
    assert.equal(spl(w, ata(dest, w.mint)), 0n);
  });
});

describe("order results", () => {
  async function orderWorld() {
    const w = await world();
    const body = postJobFromJson(vec("bridge_unlock_order").body.job_order);
    const poster = Keypair.generate().publicKey;
    await orderDeposit(w, 0n, body, poster);
    return { w, jobId: hexOf(orderJobId(0n)) };
  }

  it("finalize_order once the job is Completed and K deep (relayer holds the storefront attester key)", async () => {
    const { w, jobId } = await orderWorld();
    const rl = relayer(w);
    await rl.tick();
    w.node.mine();
    const deliverer = Keypair.generate().publicKey;
    const rh = Buffer.alloc(32, 0x5a);
    w.node.mine([{ op: "deliver", jobId, deliverer, resultHash: rh, uri: "ipfs://result" }, { op: "complete", jobId }]);

    w.node.mineN(K - 1);
    let t = await rl.tick();
    assert.deepEqual(t.finalized, [], "not before K");
    w.node.mine();
    t = await rl.tick();
    assert.deepEqual(t.finalized, ["0"], String(t.errors));
    const o = decodeOrder(w.token.svm.getAccount(orderPda(0n))!.data);
    assert.equal(o.status, ORDER_STATUS_FINALIZED);
    const raw = Buffer.from(w.token.svm.getAccount(orderPda(0n))!.data);
    assert.ok(raw.subarray(ORDER_LAYOUT.DELIVERER, ORDER_LAYOUT.DELIVERER + 32).equals(deliverer.toBuffer()));
    assert.ok(raw.subarray(ORDER_LAYOUT.RESULT_HASH, ORDER_LAYOUT.RESULT_HASH + 32).equals(rh));
    assert.equal(raw.subarray(270, 270 + raw.readUInt16LE(268)).toString(), "ipfs://result");
    t = await rl.tick();
    assert.deepEqual([t.finalized, rl.state.orders], [[], []]);
  });

  it("expire_order when the job expired on the chain", async () => {
    const { w, jobId } = await orderWorld();
    const rl = relayer(w);
    await rl.tick();
    w.node.mine();
    w.node.mine([{ op: "expire", jobId }]);
    w.node.mineN(K);
    const t = await rl.tick();
    assert.deepEqual(t.expired, ["0"], String(t.errors));
    assert.equal(decodeOrder(w.token.svm.getAccount(orderPda(0n))!.data).status, 3);
  });

  it("without the storefront attester key it reports that an attester must finalize", async () => {
    const { w, jobId } = await orderWorld();
    const rl = relayer(w, { signers: [...w.attesters, w.bridgeSigner] });
    await rl.tick();
    w.node.mine();
    w.node.mine([{ op: "deliver", jobId, deliverer: Keypair.generate().publicKey, resultHash: Buffer.alloc(32, 1), uri: "" }, { op: "complete", jobId }]);
    w.node.mineN(K);
    const t = await rl.tick();
    assert.deepEqual([t.finalized, t.needsAttester], [[], ["0"]]);
    assert.equal(decodeOrder(w.token.svm.getAccount(orderPda(0n))!.data).status, ORDER_STATUS_PENDING);
    assert.ok((rl.health(2000).orders as string[]).includes("0"));
  });
});

describe("supply invariant", () => {
  it("refuses on a deficit, allows a surplus unless ALLOW_SURPLUS=0", async () => {
    const w = await world();
    const rl = relayer(w);
    const u = Keypair.generate();
    await userDeposits(w, 1_000_000n, u.publicKey, 0n);

    w.node.mine([{ op: "leak", amount: 7000n, to: Keypair.generate().publicKey }]); // escrow funds leave outside the bridge
    let t = await rl.tick();
    assert.match(String(t.refused), /invariant/);
    assert.equal(t.invariant!.delta, "-7000");
    assert.deepEqual(t.unlocked, []);
    assert.equal(w.node.mempool.length, 0);
    assert.equal(rl.health(2000).ok, false);

    // Restore by a reorg of that block, then donate SPL to the vault (surplus).
    w.node.reorg(1);
    await mintTo(w, vaultAta(w.mint), 3n);
    t = await relayer(w, { allowSurplus: false }).tick();
    assert.match(String(t.refused), /invariant/);
    assert.equal(t.invariant!.delta, "3000");
    t = await rl.tick();
    assert.equal(t.refused, null);
    assert.deepEqual(t.unlocked, ["0"]);
  });
});

describe("restart and modes", () => {
  it("resumes from the state file without resubmitting or re-releasing", async () => {
    const w = await world();
    const rl = relayer(w);
    const u = await funded(w, rl, 5_000_000n, 0n);
    await userDeposits(w, 1_000_000n, Keypair.generate().publicKey, 1n);
    await rl.tick(); // unlock 1 in flight
    await lock(w, u, 1_000_000_000n, Keypair.generate().publicKey);
    w.node.mine();
    w.node.mineN(K);
    let t = await rl.tick();
    assert.deepEqual(t.released, ["0"]);

    const saved = loadState(w.stateFile);
    assert.equal(saved.withdrawal, 1n);
    assert.equal(saved.deposit, 2n);

    const rl2 = relayer(w);
    assert.deepEqual([rl2.state.deposit, rl2.state.withdrawal], [2n, 1n]);
    const sends = w.node.calls.get("sendTx") ?? 0;
    t = await rl2.tick();
    assert.deepEqual([t.unlocked, t.released, t.refused], [[], [], null]);
    assert.equal(w.node.calls.get("sendTx") ?? 0, sends);
    assert.ok(t.invariant!.strictFormHolds);
  });

  it("resumes an in-flight unlock after a restart without sending it again", async () => {
    const w = await world();
    await userDeposits(w, 1_000_000n, Keypair.generate().publicKey, 0n);
    await relayer(w).tick();
    const rl2 = relayer(w);
    assert.ok(rl2.state.inflight["0"]);
    const t = await rl2.tick();
    assert.deepEqual(t.unlocked, []);
    assert.equal(w.node.mempool.length, 1);
  });

  it("dry run checks but never submits", async () => {
    const w = await world();
    await userDeposits(w, 1_000_000n, Keypair.generate().publicKey, 0n);
    const t = await relayer(w, { dryRun: true }).tick();
    assert.deepEqual(t.pendingDeposits, ["0"]);
    assert.ok(t.invariant!.ok);
    assert.deepEqual(t.unlocked, []);
    assert.equal(w.node.mempool.length, 0);
  });

  it("does not unlock without enough local attester keys", async () => {
    const w = await world({ attesters: [vseed(30), vseed(31)], threshold: 2 });
    await userDeposits(w, 1_000_000n, Keypair.generate().publicKey, 0n);
    let t = await relayer(w, { signers: [vseed(30)] }).tick();
    assert.deepEqual(t.unlocked, []);
    assert.ok(t.errors.some((e) => /fewer than 2/.test(e)), String(t.errors));
    t = await relayer(w).tick();
    assert.deepEqual(t.unlocked, ["0"]);
  });
});

describe("transports", () => {
  let server: ReturnType<typeof startHealth>;
  let ok = true;
  before(() => {
    server = startHealth(0, () => ({ ok, ticks: 1 })); // ephemeral port for the test
  });
  after(() => server.close());

  it("health: JSON, 200 when healthy and 503 when not", async () => {
    if (!server.listening) await new Promise((r) => server.once("listening", r));
    const port = (server.address() as AddressInfo).port;
    let res = await fetch(`http://127.0.0.1:${port}/health`);
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { ok: true, ticks: 1 });
    ok = false;
    res = await fetch(`http://127.0.0.1:${port}/health`);
    assert.equal(res.status, 503);
    assert.equal((await fetch(`http://127.0.0.1:${port}/nope`)).status, 404);
  });

  it("NodeRpc over HTTP JSON-RPC (fake node behind an ephemeral port)", async () => {
    const fake = new FakeNode({ attesters: [] });
    const srv = createServer((req, res) => {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        const j = JSON.parse(body);
        res.writeHead(200, { "content-type": "application/json" });
        try {
          res.end(JSON.stringify({ jsonrpc: "2.0", id: j.id, result: fake.handle(j.method, j.params) }));
        } catch (e) {
          res.end(JSON.stringify({ jsonrpc: "2.0", id: j.id, error: { code: -32000, message: String((e as Error).message) } }));
        }
      });
    }).listen(0, "127.0.0.1");
    await new Promise((r) => srv.once("listening", r));
    try {
      const n = new NodeRpc(httpTransport(`http://127.0.0.1:${(srv.address() as AddressInfo).port}/`));
      const e = await n.getEscrow();
      assert.equal(e.escrowBalance, 1_000_000_000_000_000_000n);
      assert.equal((await n.getChainInfo()).finalityK, 30);
      await assert.rejects(n.sendTx(Buffer.alloc(10)), /-?too short|node rpc sendTx/);
    } finally {
      srv.close();
    }
  });

  it("RpcChain keeps 1e18-scale lamports exact", async () => {
    const owner = Keypair.generate().publicKey;
    const srv = createServer((req, res) => {
      req.resume();
      req.on("end", () => {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(`{"jsonrpc":"2.0","id":1,"result":{"context":{"slot":5},"value":[{"lamports":999999999999999999,` +
          `"owner":"${owner.toBase58()}","data":["AQID","base64"],"executable":false,"rentEpoch":18446744073709551615,"space":3},null]}}`);
      });
    }).listen(0, "127.0.0.1");
    await new Promise((r) => srv.once("listening", r));
    try {
      const chain = new RpcChain("mock", `http://127.0.0.1:${(srv.address() as AddressInfo).port}`);
      const [a, b] = await chain.getAccounts([Keypair.generate().publicKey, Keypair.generate().publicKey]);
      assert.equal(a!.lamports, 999_999_999_999_999_999n);
      assert.deepEqual([...a!.data], [1, 2, 3]);
      assert.equal(b, null);
    } finally {
      srv.close();
    }
  });
});

