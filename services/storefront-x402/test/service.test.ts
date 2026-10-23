// End to end in LiteSVM (no validator, no ports): real pump bytecode
// (fixtures/pump), the real bridge and storefront programs
// (token/target/deploy), and the service's request handler.
// Build first: cd token && anchor build -p bridge && anchor build -p storefront

import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { existsSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Ed25519Program, Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram, TransactionInstruction, TransactionMessage, VersionedTransaction, ComputeBudgetProgram } from "@solana/web3.js";
import { Clock, FailedTransactionMetadata, LiteSVM } from "./litesvm-web3.ts";
import { LiteSvmChain } from "./litesvm-chain.ts";
import { orphanDrafts, Service } from "../src/service.ts";
import type { HttpRes } from "../src/service.ts";
import * as sf from "../src/storefront.ts";
import { buildNativePayment, decodePaymentHeader, encodePaymentHeader, PaymentError, verifyExact } from "../src/x402.ts";
import type { PaymentRequirements } from "../src/x402.ts";

const require = createRequire(import.meta.url);
const pumpSdk = require("@pump-fun/pump-sdk");
const BN = require("bn.js");

const here = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(here, "../../..");
const FIX = join(ROOT, "fixtures/pump");
const DEPLOY = join(ROOT, "token/target/deploy");
const enc = (s: string) => Buffer.from(s);
const disc = (n: string) => sf.sha256(enc(`global:${n}`)).subarray(0, 8);
const some = (b: Buffer) => Buffer.concat([Buffer.from([1]), b]);
const none = Buffer.from([0]);
const vecKeys = (ks: PublicKey[]) => Buffer.concat([sf.u32(ks.length), ...ks.map((k) => k.toBuffer())]);
const m = (pubkey: PublicKey, isWritable = false, isSigner = false) => ({ pubkey, isWritable, isSigner });

const svm = new LiteSVM();
const chain = new LiteSvmChain(svm);
const A = sf.addresses();
const admin = Keypair.generate(), wallet = Keypair.generate(), client = Keypair.generate(), mintKp = Keypair.generate();
const attesters = [Keypair.generate(), Keypair.generate(), Keypair.generate()];
const MINT = mintKp.publicKey;
const VAULT = sf.ata(A.bridgeConfig, MINT, sf.TOKEN_2022);
const NETWORK = "solana-localnet";

function rawSend(ixs: TransactionInstruction[], signers: Keypair[]) {
  const msg = new TransactionMessage({ payerKey: signers[0].publicKey, recentBlockhash: svm.latestBlockhash(), instructions: [ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }), ...ixs] }).compileToLegacyMessage();
  const tx = new VersionedTransaction(msg);
  tx.sign(signers);
  const r = svm.sendTransaction(tx);
  svm.expireBlockhash();
  if (r instanceof FailedTransactionMetadata) throw new Error(`${r.err()} ${r.meta().logs().join("\n")}`);
  return r;
}
const configParams = (o: { mint?: PublicKey; bridge?: PublicKey; slippage?: number; minSol?: bigint; maxSol?: bigint } = {}) => Buffer.concat([
  o.mint ? some(o.mint.toBuffer()) : none, o.bridge ? some(o.bridge.toBuffer()) : none, none, none,
  o.slippage !== undefined ? some(sf.u16(o.slippage)) : none, o.minSol ? some(sf.u64(o.minSol)) : none, o.maxSol ? some(sf.u64(o.maxSol)) : none, none, none, none,
]);
const updateConfigIx = (p: Buffer) => new TransactionInstruction({ programId: A.storefront, keys: [m(admin.publicKey, false, true), m(A.config, true)], data: Buffer.concat([disc("update_config"), p]) });

let service: Service;
const call = (method: string, url: string, headers: Record<string, string> = {}, body?: unknown) =>
  service.handle({ method, url, headers, body: body === undefined ? undefined : JSON.stringify(body) });
const jobBody = (o: Record<string, unknown> = {}) => ({
  spec: { m: 256, n: 256, k: 512, config: "11".repeat(52), hash_a: "aa".repeat(32), hash_b: "bb".repeat(32), data_uri: "ipfs://job-a-b", deadline_blocks: 600, max_blocks: 20, ...o },
});

test("setup: pump coin, bridge, storefront, service", () => {
  for (const [n, id] of [["pump", sf.PUMP], ["pump_amm", sf.AMM], ["pump_fees", sf.PUMP_FEES]] as const) svm.addProgramFromFile(id, join(FIX, "programs", `${n}.so`));
  for (const f of readdirSync(join(FIX, "accounts"))) {
    const j = JSON.parse(readFileSync(join(FIX, "accounts", f), "utf8"));
    svm.setAccount(new PublicKey(j.pubkey), { lamports: j.account.lamports, data: Buffer.from(j.account.data[0], "base64"), owner: new PublicKey(j.account.owner), executable: j.account.executable, rentEpoch: 0 });
  }
  svm.addProgramFromFile(A.bridge, join(DEPLOY, "bridge.so"));
  svm.addProgramFromFile(A.storefront, join(DEPLOY, "storefront.so"));
  const c = svm.getClock();
  svm.setClock(new Clock(c.slot + 10n, c.epochStartTimestamp, c.epoch, c.leaderScheduleEpoch, BigInt(Math.floor(Date.now() / 1000))));
  for (const k of [admin, wallet, client]) svm.airdrop(k.publicKey, BigInt(100 * LAMPORTS_PER_SOL));
  return (async () => {
    const create = await pumpSdk.PUMP_SDK.createV2Instruction({ mint: MINT, name: "Pearl Test", symbol: "PRLT", uri: "https://example.com/p.json", creator: admin.publicKey, user: admin.publicKey, mayhemMode: false });
    rawSend([create], [admin, mintKp]);
    rawSend([new TransactionInstruction({
      programId: A.bridge,
      keys: [m(admin.publicKey, true, true), m(A.bridgeConfig, true), m(MINT), m(VAULT, true), m(sf.TOKEN_2022), m(sf.ATA_PROGRAM), m(SystemProgram.programId)],
      data: Buffer.concat([disc("initialize"), vecKeys([Keypair.generate().publicKey]), Buffer.from([1])]),
    })], [admin]);
    rawSend([new TransactionInstruction({
      programId: A.storefront,
      keys: [m(admin.publicKey, true, true), m(A.config, true), m(A.trader, true), m(SystemProgram.programId)],
      data: Buffer.concat([disc("initialize"), configParams({ mint: MINT, bridge: A.bridge, minSol: 10_000_000n, maxSol: 5n * BigInt(LAMPORTS_PER_SOL) }), vecKeys(attesters.map((k) => k.publicKey)), Buffer.from([2])]),
    })], [admin]);
    for (const prog of [sf.PUMP, sf.AMM]) {
      rawSend([new TransactionInstruction({
        programId: prog,
        keys: [m(admin.publicKey, true, true), m(A.trader), m(sf.pda([enc("user_volume_accumulator"), A.trader.toBuffer()], prog), true), m(SystemProgram.programId), m(sf.pda([enc("__event_authority")], prog)), m(prog)],
        data: Buffer.from([94, 6, 202, 115, 255, 96, 232, 183]),
      })], [admin]);
    }
    service = new Service({ chain, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing: { baseLamports: 10_000_000n, lamportsPerGflop: 100_000n, overheadLamports: 14_000_000n } });
  })();
});

test("buyback recipient offsets match the pump SDKs", () => {
  const g = svm.getAccount(sf.pda([enc("global")], sf.PUMP))!;
  const decoded = pumpSdk.PUMP_SDK.decodeGlobal({ data: Buffer.from(g.data), owner: g.owner, lamports: Number(g.lamports), executable: false, rentEpoch: 0 });
  assert.equal(sf.pumpGlobalBuyback(g.data).toBase58(), decoded.buybackFeeRecipients[0].toBase58());
  const gc = svm.getAccount(sf.pda([enc("global_config")], sf.AMM))!;
  assert.equal(sf.ammGlobalBuyback(gc.data).toBase58(), decoded.buybackFeeRecipients[0].toBase58());
});

test("GET /evaluators lists the default set and how buyers name their own", async () => {
  const r = await call("GET", "/evaluators");
  assert.equal(r.status, 200);
  const b = r.body as any;
  assert.deepEqual(b.default, attesters.map((k) => k.publicKey.toBase58()));
  assert.ok(b.default_threshold >= 1 && b.default_threshold <= b.default.length);
  assert.equal(b.max_evaluators, sf.MAX_EVALUATORS);
  assert.match(b.check, /EVALUATORS\.md/);
});

test("GET /quote prices a job from its size", async () => {
  const r = await call("GET", "/quote?m=1024&n=1024&k=1024&max_blocks=10");
  assert.equal(r.status, 200);
  const q = r.body as any;
  assert.equal(q.work_flops, 2n * 1024n ** 3n * 10n);
  // job = 10M base + ceil(21.47 GFLOP x 100k); sol_in grossed up so that sol_in minus the
  // on-chain buyback share (default 1000 bps) still covers the job
  const job = 10_000_000n + 2_147_484n;
  assert.equal(q.buyback_bps, 1000);
  assert.equal(q.buyback_destination, "reserve");
  assert.equal(q.sol_in_lamports, (job * 10_000n + 8_999n) / 9_000n);
  assert.equal(q.buyback_lamports, (q.sol_in_lamports * 1000n) / 10_000n, "the program's floor split");
  assert.equal(q.job_lamports, q.sol_in_lamports - q.buyback_lamports);
  assert.ok(q.job_lamports >= job && q.job_lamports - job <= 1n, "the job part covers the job price");
  assert.equal(q.price_lamports, q.sol_in_lamports + 14_000_000n);
  assert.equal(q.venue, "curve");
  assert.ok(q.estimate.tokens_spl > 0n && q.estimate.min_tokens_out_spl < q.estimate.tokens_spl);
  assert.equal((await call("GET", "/quote?m=0&n=1&k=1")).status, 400);
  assert.equal((await call("GET", "/quote?m=100000&n=1&k=1")).status, 400);
  assert.equal((await call("GET", "/quote?m=8192&n=8192&k=8192&max_blocks=10000")).status, 400, "above max_sol_in");
});

test("x402 verifier: accepts an exact native payment, rejects tampering", async () => {
  const req = service.requirements(25_000_000n);
  const bh = await chain.latestBlockhash();
  const good = verifyExact(decodePaymentHeader(buildNativePayment(client, req, bh)), req);
  assert.equal(good.payer.toBase58(), client.publicKey.toBase58());
  assert.equal(good.amount, 25_000_000n);
  assert.equal(good.needsFeePayerSig, true);
  const bad = (ixs: TransactionInstruction[], payer = wallet.publicKey, signers = [client], r: PaymentRequirements = req) => {
    const tx = new VersionedTransaction(new TransactionMessage({ payerKey: payer, recentBlockhash: bh, instructions: ixs }).compileToV0Message());
    tx.sign(signers);
    const h = encodePaymentHeader({ x402Version: 1, scheme: "exact", network: NETWORK, payload: { transaction: Buffer.from(tx.serialize()).toString("base64") } });
    assert.throws(() => verifyExact(decodePaymentHeader(h), r), PaymentError);
  };
  const xfer = (lamports: bigint, to = wallet.publicKey, from = client.publicKey) => SystemProgram.transfer({ fromPubkey: from, toPubkey: to, lamports });
  bad([xfer(24_999_999n)]);
  bad([xfer(25_000_000n, Keypair.generate().publicKey)]);
  bad([xfer(25_000_000n), xfer(1n)]);
  bad([xfer(25_000_000n), SystemProgram.transfer({ fromPubkey: wallet.publicKey, toPubkey: client.publicKey, lamports: 1n })]);
  bad([ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 50_000_000n }), xfer(25_000_000n)]);
  bad([new TransactionInstruction({ programId: sf.STOREFRONT_ID, keys: [], data: Buffer.alloc(0) }), xfer(25_000_000n)]);
  // service as transfer source (would make the service pay itself)
  assert.throws(() => {
    const tx = new VersionedTransaction(new TransactionMessage({ payerKey: wallet.publicKey, recentBlockhash: bh, instructions: [xfer(25_000_000n, wallet.publicKey, wallet.publicKey)] }).compileToV0Message());
    verifyExact({ x402Version: 1, scheme: "exact", network: NETWORK, payload: { transaction: Buffer.from(tx.serialize()).toString("base64") } }, req);
  }, PaymentError);
  // unsigned by the payer
  assert.throws(() => {
    const tx = new VersionedTransaction(new TransactionMessage({ payerKey: wallet.publicKey, recentBlockhash: bh, instructions: [xfer(25_000_000n)] }).compileToV0Message());
    verifyExact({ x402Version: 1, scheme: "exact", network: NETWORK, payload: { transaction: Buffer.from(tx.serialize()).toString("base64") } }, req);
  }, /signature/);
  // wrong network / scheme
  const p = decodePaymentHeader(buildNativePayment(client, req, bh));
  assert.throws(() => verifyExact({ ...p, network: "solana" }, req), /network/);
  assert.throws(() => verifyExact({ ...p, scheme: "upto" }, req), /scheme/);
  // client pays its own fee: fine too
  const own = new VersionedTransaction(new TransactionMessage({ payerKey: client.publicKey, recentBlockhash: bh, instructions: [xfer(25_000_000n)] }).compileToV0Message());
  own.sign([client]);
  const v = verifyExact({ x402Version: 1, scheme: "exact", network: NETWORK, payload: { transaction: Buffer.from(own.serialize()).toString("base64") } }, req);
  assert.equal(v.needsFeePayerSig, false);
});

test("x402 verifier: SPL (USDC-style) TransferChecked to payTo's ATA", async () => {
  const usdc = Keypair.generate().publicKey;
  const req: PaymentRequirements = { ...service.requirements(1_500_000n), asset: usdc.toBase58(), extra: { feePayer: wallet.publicKey.toBase58(), assetKind: "spl", decimals: 6 } };
  const bh = await chain.latestBlockhash();
  const tc = (dest: PublicKey, amount: bigint, decimals = 6) => new TransactionInstruction({
    programId: sf.TOKEN,
    keys: [m(sf.ata(client.publicKey, usdc, sf.TOKEN), true), m(usdc), m(dest, true), m(client.publicKey, false, true)],
    data: Buffer.concat([Buffer.from([12]), sf.u64(amount), Buffer.from([decimals])]),
  });
  const mk = (ixs: TransactionInstruction[]) => {
    const tx = new VersionedTransaction(new TransactionMessage({ payerKey: wallet.publicKey, recentBlockhash: bh, instructions: ixs }).compileToV0Message());
    tx.sign([client]);
    return { x402Version: 1, scheme: "exact", network: NETWORK, payload: { transaction: Buffer.from(tx.serialize()).toString("base64") } };
  };
  const dest = sf.ata(wallet.publicKey, usdc, sf.TOKEN);
  assert.equal(verifyExact(mk([tc(dest, 1_500_000n)]), req).amount, 1_500_000n);
  assert.throws(() => verifyExact(mk([tc(Keypair.generate().publicKey, 1_500_000n)]), req), /ATA/);
  assert.throws(() => verifyExact(mk([tc(dest, 1_500_000n, 9)]), req), /decimals/);
  assert.throws(() => verifyExact(mk([SystemProgram.transfer({ fromPubkey: client.publicKey, toPubkey: wallet.publicKey, lamports: 1_500_000n })]), req), /native/);
});

let jobId = "";
let price = 0n;
test("POST /jobs: 402 without payment, 201 with an x402 payment; the Order is on chain", async () => {
  const r402 = await call("POST", "/jobs", {}, jobBody());
  assert.equal(r402.status, 402);
  const b = r402.body as any;
  assert.equal(b.x402Version, 1);
  const req: PaymentRequirements = b.accepts[0];
  assert.equal(req.scheme, "exact");
  assert.equal(req.payTo, wallet.publicKey.toBase58());
  assert.equal(req.extra.feePayer, wallet.publicKey.toBase58());
  price = BigInt(req.maxAmountRequired);
  // underpay -> 402 again
  const under = buildNativePayment(client, { ...req, maxAmountRequired: (price - 1n).toString() }, await chain.latestBlockhash());
  const r2 = await call("POST", "/jobs", { "x-payment": under }, jobBody());
  assert.equal(r2.status, 402);
  assert.match((r2.body as any).error, /amount/);

  const c0 = svm.getBalance(client.publicKey)!, w0 = svm.getBalance(wallet.publicKey)!;
  const header = buildNativePayment(client, req, await chain.latestBlockhash());
  const r = await call("POST", "/jobs", { "x-payment": header }, jobBody());
  assert.equal(r.status, 201, JSON.stringify(r.body, (_k, v) => (typeof v === "bigint" ? v.toString() : v)));
  const j = r.body as any;
  jobId = j.id;
  assert.equal(j.status, "pending");
  assert.equal(j.venue, "curve");
  const xpr = JSON.parse(Buffer.from(r.headers["x-payment-response"], "base64").toString());
  assert.equal(xpr.success, true);
  assert.equal(xpr.payer, client.publicKey.toBase58());
  assert.equal(c0 - svm.getBalance(client.publicKey)!, price, "client paid exactly the price (service paid the fee)");
  const spentByService = w0 - svm.getBalance(wallet.publicKey)!;
  assert.ok(spentByService < 0n || spentByService < 1_000_000n, `service nets out (overhead covers rents): ${spentByService}`);
  // on chain
  const acc = svm.getAccount(sf.orderPda(BigInt(jobId)))!;
  const o = sf.decodeOrder(acc.data);
  assert.equal(o.status, "pending");
  assert.equal(o.buyer, wallet.publicKey.toBase58());
  assert.equal(o.recipient, client.publicKey.toBase58(), "payer's key is the chain poster");
  assert.equal(o.jobId, sf.orderJobId(BigInt(jobId)).toString("hex"));
  assert.equal(o.spec.k, 512);
  assert.equal(o.spec.dataUri, "ipfs://job-a-b");
  assert.deepEqual(o.spec.evaluators, attesters.map((k) => k.publicKey.toBase58()), "defaults to the storefront attester set");
  assert.equal(o.spec.threshold, 2);
  assert.equal(o.workerPay + o.protocolFee, o.tokens * 1000n);
  const spec: sf.JobSpec = { m: 256, n: 256, k: 512, config: Buffer.alloc(52, 0x11), hashA: Buffer.alloc(32, 0xaa), hashB: Buffer.alloc(32, 0xbb), dataUri: "ipfs://job-a-b", deadlineBlocks: 600n, maxBlocks: 20, evaluators: attesters.map((k) => k.publicKey), threshold: 2 };
  assert.equal(o.bodyHash, sf.sha256(sf.postJobBody(spec, o.workerPay, o.protocolFee)).toString("hex"));
  assert.equal(sf.tokenAmount(svm.getAccount(VAULT)!.data), o.tokens);
  // the buyback share sits in the vault: sol_in x 1000 / 10000, plus the one-time rent top-up
  const solIn = price - 14_000_000n;
  const share = (solIn * 1000n) / 10_000n;
  assert.equal(BigInt(svm.getBalance(A.buybackVault)!), sf.RENT_MIN + share);
  assert.ok(o.solSpent <= solIn - share, "the job spent at most sol_in - share");

  // replay of the same payment
  const again = await call("POST", "/jobs", { "x-payment": header }, jobBody());
  assert.equal(again.status, 409);
});

test("GET /buyback reports the vault and the pending orders from chain state", async () => {
  const r = await call("GET", "/buyback");
  assert.equal(r.status, 200);
  const b = r.body as any;
  assert.equal(b.vault, A.buybackVault.toBase58());
  assert.equal(b.buyback_bps, 1000);
  assert.equal(b.destination, "reserve");
  assert.equal(b.destination_account, sf.reserveAta(MINT).toBase58());
  assert.equal(b.reserve_program, sf.RESERVE_ID.toBase58());
  assert.equal(b.cranker, null, "permissionless");
  // the order is not finalized yet: its share is held, nothing is crankable
  assert.equal(b.held_lamports, b.vault_lamports - sf.RENT_MIN);
  assert.equal(b.available_lamports, 0n);
  assert.equal(b.totals.accrued_lamports, 0n);
  assert.equal(b.totals.buys, 0n);
  assert.equal(b.pending.orders, 0n);
  assert.ok(svm.getAccount(sf.sharePda(BigInt(jobId))), "the order's share record");
});

test("GET /jobs/:id returns pending, then the result once finalized", async () => {
  const r = await call("GET", `/jobs/${jobId}`);
  assert.equal(r.status, 200);
  assert.equal((r.body as any).status, "pending");
  assert.equal((r.body as any).result, null);
  assert.equal((await call("GET", `/jobs/${sf.orderPda(BigInt(jobId)).toBase58()}`)).status, 200);
  assert.equal((await call("GET", "/jobs/999999")).status, 404);

  const nonce = BigInt(jobId);
  const deliverer = Keypair.generate().publicKey;
  const rh = sf.sha256(enc("C"));
  const msg = Buffer.concat([enc("pearl-sol:result:v1"), sf.orderJobId(nonce), deliverer.toBuffer(), rh]);
  const uri = enc("ipfs://result-c");
  const fin = new TransactionInstruction({
    programId: A.storefront,
    keys: [m(attesters[0].publicKey, false, true), m(A.config), m(sf.orderPda(nonce), true), m(sf.IX_SYSVAR)],
    data: Buffer.concat([disc("finalize_order"), deliverer.toBuffer(), rh, sf.u32(uri.length), uri]),
  });
  svm.airdrop(attesters[0].publicKey, BigInt(LAMPORTS_PER_SOL));
  rawSend([Ed25519Program.createInstructionWithPrivateKey({ privateKey: attesters[0].secretKey, message: msg }), Ed25519Program.createInstructionWithPrivateKey({ privateKey: attesters[1].secretKey, message: msg }), fin], [attesters[0]]);
  const f = (await call("GET", `/jobs/${jobId}`)).body as any;
  assert.equal(f.status, "finalized");
  assert.equal(f.result.deliverer, deliverer.toBase58());
  assert.equal(f.result.resultHash, rh.toString("hex"));
  assert.equal(f.result.resultUri, "ipfs://result-c");
});

test("POST /jobs: a failed buy refunds the payment", async () => {
  // max_slippage_bps 0 is tighter than pump's 1.25% fee, so buy_job must fail.
  rawSend([updateConfigIx(configParams({ slippage: 0 }))], [admin]);
  const r402 = await call("POST", "/jobs", {}, jobBody());
  const req: PaymentRequirements = (r402.body as any).accepts[0];
  const c0 = svm.getBalance(client.publicKey)!;
  const r: HttpRes = await call("POST", "/jobs", { "x-payment": buildNativePayment(client, req, await chain.latestBlockhash()) }, jobBody());
  assert.equal(r.status, 502);
  assert.ok((r.body as any).payment.refund && !(r.body as any).payment.refund.startsWith("FAILED"));
  assert.equal(svm.getBalance(client.publicKey)!, c0, "client whole again");
  rawSend([updateConfigIx(configParams({ slippage: 300 }))], [admin]);
});

test("POST /jobs: bad specs are 400 before any payment", async () => {
  for (const o of [{ m: 0 }, { config: "11" }, { hash_a: "zz".repeat(32) }, { data_uri: "x".repeat(201) }, { deadline_blocks: 0 }, { evaluators: ["nope"] }, { threshold: 9 }]) {
    const r = await call("POST", "/jobs", {}, jobBody(o));
    assert.equal(r.status, 400, JSON.stringify(o));
  }
  assert.equal((await call("POST", "/jobs", {}, undefined)).status, 400);
});

test("POST /jobs: an inference ticket (SPEC 4.5) is priced by its registered model's MACs", async () => {
  const model = "ab".repeat(32);
  const macs = 1_235_746_816n;
  const svc = new Service({ chain, wallet, network: NETWORK, publicUrl: "http://storefront.test",
    pricing: { baseLamports: 10_000_000n, lamportsPerGflop: 100_000n, overheadLamports: 14_000_000n },
    getModel: async (id) => (id === model ? { macs_per_row: macs.toString(), row_period: 4 } : null) });
  const ticket = (o: Record<string, unknown> = {}) => ({ spec: { m: 6, n: 10, k: 1, config: Buffer.concat([Buffer.from("CADRE-T1"), Buffer.from(model, "hex"), Buffer.alloc(12)]).toString("hex"),
    hash_a: "aa".repeat(32), hash_b: "bb".repeat(32), data_uri: "blob:" + "cc".repeat(32), deadline_blocks: 400, max_blocks: 96, ...o } });
  const r = await svc.handle({ method: "POST", url: "/jobs", headers: {}, body: JSON.stringify(ticket()) });
  assert.equal(r.status, 402);
  // rows = round_up(6 + 10, 4) = 16; work = 2 x macs x 16 flops at 100k lamports per GFLOP
  const flops = 2n * macs * 16n;
  const job = 10_000_000n + (flops * 100_000n + 999_999_999n) / 1_000_000_000n;
  const solIn = (job * 10_000n + 8_999n) / 9_000n;
  assert.equal(BigInt((r.body as any).accepts[0].maxAmountRequired), solIn + 14_000_000n);
  const bad = await svc.handle({ method: "POST", url: "/jobs", headers: {}, body: JSON.stringify(ticket({ k: 2 })) });
  assert.equal(bad.status, 400);
  const other = { ...ticket() };
  other.spec.config = Buffer.concat([Buffer.from("CADRE-T1"), Buffer.alloc(32, 1), Buffer.alloc(12)]).toString("hex");
  const unk = await svc.handle({ method: "POST", url: "/jobs", headers: {}, body: JSON.stringify(other) });
  assert.equal(unk.status, 400);
  assert.match((unk.body as any).error, /not registered/);
  // GET /quote prices the same ticket without placing it (cadre-api reads this)
  const qr = await svc.handle({ method: "GET", url: `/quote?model_id=${model}&prompt_tokens=6&max_tokens=10`, headers: {} });
  assert.equal(qr.status, 200, String((qr.body as any).error));
  assert.equal(BigInt((qr.body as any).price_lamports), solIn + 14_000_000n);
  assert.equal((qr.body as any).work_flops, flops);
  assert.deepEqual((qr.body as any).ticket, { model_id: model, prompt_tokens: 6, max_tokens: 10 });
  assert.equal((await svc.handle({ method: "GET", url: `/quote?model_id=${"01".repeat(32)}&prompt_tokens=6&max_tokens=10`, headers: {} })).status, 400);
});

test("POST /jobs: buyer-named evaluators are posted as given (open evaluation)", async () => {
  const mine = [Keypair.generate().publicKey.toBase58(), Keypair.generate().publicKey.toBase58()];
  const body = jobBody({ evaluators: mine, threshold: 1, data_uri: "ipfs://own-evaluators" });
  const r402 = await call("POST", "/jobs", {}, body);
  assert.equal(r402.status, 402);
  const req: PaymentRequirements = (r402.body as any).accepts[0];
  const header = buildNativePayment(client, req, await chain.latestBlockhash());
  const r = await call("POST", "/jobs", { "x-payment": header }, body);
  assert.equal(r.status, 201, JSON.stringify(r.body, (_k, v) => (typeof v === "bigint" ? v.toString() : v)));
  const o = sf.decodeOrder(svm.getAccount(sf.orderPda(BigInt((r.body as any).id)))!.data);
  assert.deepEqual(o.spec.evaluators, mine);
  assert.equal(o.spec.threshold, 1);
});

// ---- payments left unserved by a crash or a failed refund (devnet 2026-10-20/21) ----

type Fault = "hang" | "land-then-hang" | "fail" | undefined;
/** ChainIO over the test chain that can stall or fail chosen txs (a "crash" mid-placement). */
class FaultChain {
  rule: (ixs: TransactionInstruction[]) => Fault = () => undefined;
  readonly inner: LiteSvmChain;
  constructor(inner: LiteSvmChain) {
    this.inner = inner;
  }
  getAccounts(keys: PublicKey[]) { return this.inner.getAccounts(keys); }
  sendRaw(raw: Uint8Array) { return this.inner.sendRaw(raw); }
  latestBlockhash() { return this.inner.latestBlockhash(); }
  async send(ixs: TransactionInstruction[], payer: Keypair, extra?: Keypair[], cu?: number): Promise<string> {
    const f = this.rule(ixs);
    if (f === "hang") return new Promise<string>(() => {});
    if (f === "fail") throw new Error("429 Too Many Requests (injected)");
    const sig = await this.inner.send(ixs, payer, extra, cu);
    if (f === "land-then-hang") return new Promise<string>(() => {});
    return sig;
  }
}
const isIx = (name: string) => (ixs: TransactionInstruction[]) => ixs.some((ix) => ix.data.subarray(0, 8).equals(disc(name)));
const isRefund = (ixs: TransactionInstruction[]) => ixs.some((ix) => ix.programId.equals(SystemProgram.programId));
const tmpState = () => join(mkdtempSync(join(tmpdir(), "sf-state-")), "state.json");
const pricing = { baseLamports: 10_000_000n, lamportsPerGflop: 100_000n, overheadLamports: 14_000_000n };
const quiet = () => {};

/** Pays a job through a service on `fc`; returns once the payment is recorded (the placement may hang). */
async function payThrough(svc: Service, stateFile: string) {
  const req: PaymentRequirements = ((await svc.handle({ method: "POST", url: "/jobs", headers: {}, body: JSON.stringify(jobBody()) })).body as any).accepts[0];
  const header = buildNativePayment(client, req, await chain.latestBlockhash());
  const pending = svc.handle({ method: "POST", url: "/jobs", headers: { "x-payment": header }, body: JSON.stringify(jobBody()) });
  for (let i = 0; i < 200 && !existsSync(stateFile); i++) await new Promise((r) => setTimeout(r, 5));
  await new Promise((r) => setTimeout(r, 20));
  return { price: BigInt(req.maxAmountRequired), pending };
}
/** The service after a restart on the same state file, its clock 10 min later. */
const restarted = (stateFile: string, inner = chain) =>
  new Service({ chain: inner, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, now: () => Date.now() + 600_000, log: quiet });

test("crash before open_order lands: the restarted service refunds the payment", async () => {
  const fc = new FaultChain(chain), stateFile = tmpState();
  fc.rule = (ixs) => (isIx("open_order")(ixs) ? "hang" : undefined);
  const svc = new Service({ chain: fc, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, log: quiet });
  const c0 = svm.getBalance(client.publicKey)!;
  const { price } = await payThrough(svc, stateFile);
  assert.equal(c0 - svm.getBalance(client.publicKey)!, price, "paid");
  const s2 = restarted(stateFile);
  const un = (await s2.handle({ method: "GET", url: "/payments/unserved", headers: {} })).body as any;
  assert.equal(un.payments.length, 1);
  assert.equal(un.payments[0].v, 2);
  const done = await s2.reconcile();
  assert.equal(done.length, 1);
  assert.equal(svm.getBalance(client.publicKey)!, c0, "client whole again");
  assert.equal(((await s2.handle({ method: "GET", url: "/payments/unserved", headers: {} })).body as any).payments.length, 0);
  assert.deepEqual(await s2.reconcile(), [], "refunded once");
});

test("crash after buy_job was sent but did not land: reconcile cancels the draft, then refunds", async () => {
  const fc = new FaultChain(chain), stateFile = tmpState();
  fc.rule = (ixs) => (isIx("buy_job")(ixs) ? "hang" : undefined);
  const svc = new Service({ chain: fc, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, log: quiet });
  const c0 = svm.getBalance(client.publicKey)!;
  await payThrough(svc, stateFile);
  const rec = JSON.parse(readFileSync(stateFile, "utf8")).payments;
  const r = Object.values(rec)[0] as any;
  assert.equal(r.buy_sent, true);
  const draft = sf.draftPda(wallet.publicKey, BigInt(r.draft));
  assert.ok(svm.getAccount(draft)?.data.length, "draft open");
  await restarted(stateFile).reconcile();
  const gone = svm.getAccount(draft);
  assert.ok(!gone || gone.data.length === 0, "draft cancelled");
  assert.equal(svm.getBalance(client.publicKey)!, c0, "client whole again");
});

test("crash after buy_job landed: no refund (an Order exists), payment held for review", async () => {
  const fc = new FaultChain(chain), stateFile = tmpState();
  fc.rule = (ixs) => (isIx("buy_job")(ixs) ? "land-then-hang" : undefined);
  const svc = new Service({ chain: fc, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, log: quiet });
  const c0 = svm.getBalance(client.publicKey)!;
  const { price } = await payThrough(svc, stateFile);
  const s2 = restarted(stateFile);
  const done = await s2.reconcile();
  assert.match(done[0].result, /^review/);
  assert.equal(c0 - svm.getBalance(client.publicKey)!, price, "no double payout: the Order serves the payment");
  const un = ((await s2.handle({ method: "GET", url: "/payments/unserved", headers: {} })).body as any).payments;
  assert.match(un[0].review, /Order may exist/);
  assert.deepEqual(await s2.reconcile(), [], "review is not retried");
});

test("a refund that failed (RPC 429) is retried by reconcile; records without v are left alone", async () => {
  rawSend([updateConfigIx(configParams({ slippage: 0 }))], [admin]); // buy_job fails
  const fc = new FaultChain(chain), stateFile = tmpState();
  fc.rule = (ixs) => (isRefund(ixs) ? "fail" : undefined);
  const svc = new Service({ chain: fc, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, log: quiet });
  const c0 = svm.getBalance(client.publicKey)!;
  const { pending } = await payThrough(svc, stateFile);
  const r = await pending;
  assert.equal(r.status, 502);
  assert.match((r.body as any).payment.refund, /^FAILED/);
  rawSend([updateConfigIx(configParams({ slippage: 300 }))], [admin]);
  // a legacy record (before v 2) with no Order: listed, never auto-refunded
  const st = JSON.parse(readFileSync(stateFile, "utf8"));
  st.payments["legacy"] = { payer: client.publicKey.toBase58(), amount: "1000", signature: "x", at: 0 };
  writeFileSync(stateFile, JSON.stringify(st));
  const s2 = restarted(stateFile);
  const done = await s2.reconcile();
  assert.equal(done.length, 1);
  assert.equal(svm.getBalance(client.publicKey)!, c0, "client whole again");
  const un = ((await s2.handle({ method: "GET", url: "/payments/unserved", headers: {} })).body as any).payments;
  assert.deepEqual(un.map((p: any) => p.id), ["legacy"]);
});

/** LiteSVM cannot list program accounts: list the drafts of the ids this test knows about. */
class ListingChain extends FaultChain {
  known = new Set<bigint>();
  async listDrafts(buyer: PublicKey, program: PublicKey) {
    const out = [];
    for (const id of this.known) {
      const address = sf.draftPda(buyer, id, program);
      const [a] = await this.inner.getAccounts([address]);
      if (a && a.owner.equals(program) && a.data.length) out.push({ address: address.toBase58(), draftId: id, recipient: client.publicKey.toBase58(), lamports: a.lamports });
    }
    return out;
  }
}

test("orphanDrafts keeps drafts an unserved payment names, cancels the rest", () => {
  const d = (id: bigint) => ({ address: `d${id}`, draftId: id, recipient: "r", lamports: 4_109_720n });
  const payments: Record<string, any> = {
    inflight: { payer: "p", amount: "1", signature: "s1", at: 0, v: 2, draft: "1" }, // placement running / reconcile decides
    review: { payer: "p", amount: "1", signature: "s2", at: 0, v: 2, draft: "2", buy_sent: true, review: "check" },
    served: { payer: "p", amount: "1", signature: "s3", at: 0, v: 2, draft: "3", nonce: "9" },
    refunded: { payer: "p", amount: "1", signature: "s4", at: 0, v: 2, draft: "4", refund: "sig" },
    legacy: { payer: "p", amount: "1", signature: "s5", at: 0 }, // crash before v 2: names no draft
  };
  assert.deepEqual(orphanDrafts([1n, 2n, 3n, 4n, 5n].map(d), payments).map((x) => x.draftId), [3n, 4n, 5n]);
  assert.deepEqual(orphanDrafts([d(1n)], { a: { ...payments.inflight, cancelled: true } }).map((x) => x.draftId), [1n], "our own cancel confirmed: not in use");
});

test("the running service cancels orphaned drafts, never one a placement or reconcile still needs", async () => {
  const lc = new ListingChain(chain), stateFile = tmpState();
  lc.rule = (ixs) => (isIx("buy_job")(ixs) ? "hang" : undefined);
  const svc = new Service({ chain: lc, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, log: quiet });
  // two placements stuck after buy_job was sent: both drafts open, both payments unserved
  await payThrough(svc, stateFile);
  const first = Object.values(JSON.parse(readFileSync(stateFile, "utf8")).payments)[0] as any;
  const s2 = restarted(stateFile, lc as any);
  await payThrough(s2, stateFile);
  const recs = JSON.parse(readFileSync(stateFile, "utf8")).payments;
  const ids = Object.values(recs).map((r: any) => BigInt(r.draft));
  assert.equal(ids.length, 2);
  ids.forEach((i) => lc.known.add(i));
  // the first one becomes what a pre-v-2 crash left: a record that names no draft
  const st = JSON.parse(readFileSync(stateFile, "utf8"));
  for (const r of Object.values(st.payments) as any[]) if (r.signature === first.signature) { delete r.v; delete r.draft; delete r.buy_sent; delete r.opened; }
  writeFileSync(stateFile, JSON.stringify(st));
  lc.rule = () => undefined;
  const live = new Service({ chain: lc, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, log: quiet });
  const w0 = svm.getBalance(wallet.publicKey)!;
  const done = await live.sweepDrafts();
  assert.equal(done.length, 1, JSON.stringify(done, (_k, x) => (typeof x === "bigint" ? x.toString() : x)));
  assert.ok(done[0].sig);
  const orphan = sf.draftPda(wallet.publicKey, BigInt(first.draft));
  const g = svm.getAccount(orphan);
  assert.ok(!g || g.data.length === 0, "orphan cancelled");
  assert.ok(svm.getBalance(wallet.publicKey)! > w0, "rent back to the wallet");
  const other = ids.find((i) => i !== BigInt(first.draft))!;
  assert.ok(svm.getAccount(sf.draftPda(wallet.publicKey, other))?.data.length, "the unserved payment's draft is kept");
  assert.deepEqual(await live.sweepDrafts(), [], "nothing left to sweep");
  // reconcile still sees its own draft and cancels it before refunding
  const c0 = svm.getBalance(client.publicKey)!;
  // (the second payment was taken on restarted()'s clock, 10 min ahead: reconcile 20 min ahead)
  const later = new Service({ chain: lc, wallet, network: NETWORK, publicUrl: "http://storefront.test", pricing, stateFile, now: () => Date.now() + 1_200_000, log: quiet });
  const r = await later.reconcile();
  assert.equal(r.filter((x) => !x.result.startsWith("review")).length, 1);
  assert.ok(svm.getBalance(client.publicKey)! > c0, "refunded");
});

test("CU of the service's buy_job tx", () => {
  const cus = chain.sent.map((s) => s.cu);
  console.log(`  service txs CU: ${cus.join(", ")}`);
  assert.ok(cus.length >= 4);
});
