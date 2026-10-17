// LiteSVM tests (in-process, no ports) for the storefront program (SPEC 5.1)
// against the pinned pump.fun mainnet bytecode in fixtures/pump and the real
// bridge program.
// Run: cd token && anchor build -p storefront (and bridge) && cd tests/storefront && npm ci && npm test
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { PublicKey, Keypair, Transaction, SystemProgram, ComputeBudgetProgram, LAMPORTS_PER_SOL, Connection, Ed25519Program, TransactionInstruction, SYSVAR_INSTRUCTIONS_PUBKEY } = require('@solana/web3.js');
const { LiteSVM, Clock } = require('litesvm');
const anchor = require('@coral-xyz/anchor');
const BN = require('bn.js');
const sdk = require('@pump-fun/pump-sdk');
const swap = require('@pump-fun/pump-swap-sdk');
const spl = require('@solana/spl-token');

const ROOT = path.resolve(__dirname, '../../..');
const FIX = path.join(ROOT, 'fixtures/pump');
const TARGET = path.join(ROOT, 'token/target');
const P = (s) => new PublicKey(s);
const PUMP = P('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P');
const AMM = P('pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA');
const FEE = P('pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ');
const WSOL = spl.NATIVE_MINT, TOK = spl.TOKEN_PROGRAM_ID, T22 = spl.TOKEN_2022_PROGRAM_ID, ATAP = spl.ASSOCIATED_TOKEN_PROGRAM_ID;
const pda = (seeds, prog) => PublicKey.findProgramAddressSync(seeds.map((s) => (typeof s === 'string' ? Buffer.from(s) : Buffer.isBuffer(s) ? s : s.toBuffer())), prog)[0];
const ata = (owner, mint, tp) => spl.getAssociatedTokenAddressSync(mint, owner, true, tp);
const le64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const sha256 = (...parts) => crypto.createHash('sha256').update(Buffer.concat(parts.map((p) => Buffer.from(p)))).digest();
const bn = (x) => new BN(x.toString());

const sfIdl = JSON.parse(fs.readFileSync(path.join(TARGET, 'idl/storefront.json')));
const brIdl = JSON.parse(fs.readFileSync(path.join(TARGET, 'idl/bridge.json')));
const SF = P(sfIdl.address), BRIDGE = P(brIdl.address);
const CONFIG = pda(['config'], SF), TRADER = pda(['trader'], SF);
const BCFG = pda(['config'], BRIDGE);

const svm = new LiteSVM();
const admin = Keypair.generate(), buyer = Keypair.generate(), trader = Keypair.generate(), creator = Keypair.generate(), mintKp = Keypair.generate();
const attesters = [Keypair.generate(), Keypair.generate(), Keypair.generate()];
const MINT = mintKp.publicKey;
const provider = new anchor.AnchorProvider(new Connection('http://127.0.0.1:1'), new anchor.Wallet(admin), {});
const sf = new anchor.Program(sfIdl, provider);
const br = new anchor.Program(brIdl, provider);
const VAULT = ata(BCFG, MINT, T22);
const TRADER_ATA = ata(TRADER, MINT, T22), TRADER_WSOL = ata(TRADER, WSOL, TOK);
const BC = sdk.bondingCurvePda(MINT);

// ---------- svm helpers (same shape as token/tests/reserve)
function loadFixtures() {
  for (const [n, id] of [['pump', PUMP], ['pump_amm', AMM], ['pump_fees', FEE]]) svm.addProgramFromFile(id, path.join(FIX, 'programs', n + '.so'));
  for (const f of fs.readdirSync(path.join(FIX, 'accounts'))) {
    const j = JSON.parse(fs.readFileSync(path.join(FIX, 'accounts', f)));
    const a = j.account;
    svm.setAccount(P(j.pubkey), { lamports: a.lamports, data: Buffer.from(a.data[0], 'base64'), owner: P(a.owner), executable: a.executable, rentEpoch: 0 });
  }
}
function setTime(t) { const c = svm.getClock(); svm.setClock(new Clock(c.slot + 10n, c.epochStartTimestamp, c.epoch, c.leaderScheduleEpoch, t)); }
function send(ixs, signers) {
  const tx = new Transaction();
  tx.recentBlockhash = svm.latestBlockhash();
  tx.feePayer = signers[0].publicKey;
  tx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }), ...ixs);
  tx.sign(...signers);
  const size = tx.serialize().length;
  const r = svm.sendTransaction(tx);
  svm.expireBlockhash();
  if (typeof r.err === 'function') return { ok: false, err: String(r.err()), logs: r.meta().logs(), size };
  return { ok: true, cu: Number(r.computeUnitsConsumed()), logs: r.logs(), size };
}
function ok(ixs, signers, label) {
  const r = send(ixs, signers);
  if (!r.ok) assert.fail(`${label} failed: ${r.err}\n  ${r.logs.slice(-15).join('\n  ')}`);
  return r;
}
function fails(ixs, signers, pattern, label) {
  const r = send(ixs, signers);
  assert.equal(r.ok, false, `${label} should fail`);
  const text = r.logs.join('\n') + '\n' + r.err;
  if (pattern) assert.match(text, pattern, `${label}: expected ${pattern}, logs:\n${r.logs.slice(-10).join('\n')}\n${r.err}`);
  if (process.env.SHOW_ERRORS) console.log(`  [${label}] ${(text.match(/Error Code: \w+|custom program error: 0x[0-9a-f]+|Error: [^\n]+/g) || [r.err]).slice(-1)[0]}`);
  return text;
}
const info = (k) => { const a = svm.getAccount(k); return a ? { lamports: Number(a.lamports), data: Buffer.from(a.data), owner: a.owner, executable: a.executable, rentEpoch: 0 } : null; };
const tokenAmount = (k) => { const a = svm.getAccount(k); return a ? Buffer.from(a.data).readBigUInt64LE(64) : 0n; };
const lamports = (k) => BigInt(svm.getBalance(k) ?? 0n);
const config = () => sf.coder.accounts.decode('config', Buffer.from(svm.getAccount(CONFIG).data));
const bridgeNonce = () => BigInt(br.coder.accounts.decode('config', Buffer.from(svm.getAccount(BCFG).data)).depositNonce.toString());
const orderPda = (nonce) => pda(['order', le64(nonce)], SF);
const orderRaw = (nonce) => Buffer.from(svm.getAccount(orderPda(nonce)).data);
const order = (nonce) => sf.coder.accounts.decode('order', orderRaw(nonce));
const curve = () => sdk.PUMP_SDK.decodeBondingCurve(info(BC));

const params = (p = {}) => ({ mint: null, bridgeProgram: null, paused: null, protocolFeeBps: null, maxSlippageBps: null, minSolIn: null, maxSolIn: null, maxDim: null, maxBlocksCap: null, maxDeadlineBlocks: null, ...p });
const updateIx = (p, who = admin.publicKey) => sf.methods.updateConfig(params(p)).accountsStrict({ admin: who, config: CONFIG }).instruction();

let global, BUYBACK;
const pumpEvent = pda(['__event_authority'], PUMP), ammEvent = pda(['__event_authority'], AMM);

function specArgs(o = {}) {
  return {
    m: 256, n: 256, k: 1024,
    config: Array.from({ length: 52 }, (_, i) => i),
    hashA: Array(32).fill(0xaa), hashB: Array(32).fill(0xbb),
    dataUri: 'ipfs://bafy-job-matrices',
    deadlineBlocks: bn(500), maxBlocks: 100,
    evaluators: attesters.map((k) => k.publicKey), threshold: 2,
    ...o,
  };
}
let draftSeq = 0n;
async function openIx(spec = specArgs(), recipient = buyer.publicKey, who = buyer) {
  const id = ++draftSeq;
  const draft = pda(['draft', who.publicKey, le64(id)], SF);
  const ix = await sf.methods.openOrder(bn(id), recipient, spec).accountsStrict({ buyer: who.publicKey, config: CONFIG, draft, systemProgram: SystemProgram.programId }).instruction();
  return { ix, draft };
}
const bridgeAccts = (nonce) => ({ bridgeConfig: BCFG, bridgeVault: VAULT, bridgeDeposit: pda(['deposit', le64(nonce)], BRIDGE), bridgeProgram: BRIDGE });
function curveAccts(draft, nonce, who = buyer.publicKey) {
  return {
    buyer: who, config: CONFIG, draft, order: orderPda(nonce), trader: TRADER, mint: MINT, traderAta: TRADER_ATA, traderWsol: TRADER_WSOL,
    pumpGlobal: pda(['global'], PUMP), bondingCurve: BC, curveBaseVault: ata(BC, MINT, T22), curveQuoteVault: ata(BC, WSOL, TOK),
    userVolumeAccumulator: pda(['user_volume_accumulator', TRADER], PUMP), feeConfig: pda(['fee_config', PUMP], FEE), buybackFeeRecipient: BUYBACK,
    wsolMint: WSOL, tokenProgram: TOK, token2022Program: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId,
    pumpEventAuthority: pumpEvent, pumpProgram: PUMP, ...bridgeAccts(nonce),
  };
}
const off = new swap.PumpAmmSdk();
function ammAccts(draft, nonce, who = buyer.publicKey) {
  const pool = sdk.canonicalPumpPoolPda(MINT);
  const pl = off.decodePool(info(pool));
  const gc = off.decodeGlobalConfig(info(pda(['global_config'], AMM)));
  return {
    buyer: who, config: CONFIG, draft, order: orderPda(nonce), trader: TRADER, mint: MINT, traderAta: TRADER_ATA, traderWsol: TRADER_WSOL,
    pool, poolBaseVault: pl.poolBaseTokenAccount, poolQuoteVault: pl.poolQuoteTokenAccount, ammGlobalConfig: pda(['global_config'], AMM),
    ammUserVolumeAccumulator: pda(['user_volume_accumulator', TRADER], AMM), ammFeeConfig: pda(['fee_config', AMM], FEE),
    ammBuybackFeeRecipient: ata(gc.buybackFeeRecipients[0], WSOL, TOK), wsolMint: WSOL, tokenProgram: TOK, token2022Program: T22,
    associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, ammEventAuthority: ammEvent, ammProgram: AMM, ...bridgeAccts(nonce),
  };
}
const buyIx = (draft, nonce, solIn, minOut = 1) => sf.methods.buyJob(bn(nonce), bn(solIn), bn(minOut)).accountsStrict(curveAccts(draft, nonce)).instruction();
const buyAmmIx = (draft, nonce, solIn, minOut = 1) => sf.methods.buyJobAmm(bn(nonce), bn(solIn), bn(minOut)).accountsStrict(ammAccts(draft, nonce)).instruction();

// SPEC 4.1 PostJob body for an order, as documented in programs/storefront/src/lib.rs.
function postJobBody(spec, w, p) {
  const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
  const u16 = (n) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
  return Buffer.concat([
    u32(spec.m), u32(spec.n), u32(spec.k), Buffer.from(spec.config), Buffer.from(spec.hashA), Buffer.from(spec.hashB),
    u16(Buffer.byteLength(spec.dataUri)), Buffer.from(spec.dataUri), le64(w), le64(p), le64(spec.deadlineBlocks.toString()), u32(spec.maxBlocks),
    Buffer.from([spec.evaluators.length]), ...spec.evaluators.map((e) => e.toBuffer()), Buffer.from([spec.threshold]),
  ]);
}
const jobId = (nonce) => sha256(Buffer.from('pearl-sol:order-job:v1'), le64(nonce));
const resultMsg = (jid, deliverer, rh) => Buffer.concat([Buffer.from('pearl-sol:result:v1'), jid, deliverer.toBuffer(), rh]);
const expiredMsg = (jid) => Buffer.concat([Buffer.from('pearl-sol:expired:v1'), jid]);
const edIx = (kp, msg) => Ed25519Program.createInstructionWithPrivateKey({ privateKey: kp.secretKey, message: msg });
const finalizeIx = (nonce, deliverer, rh, uri, caller = buyer.publicKey) =>
  sf.methods.finalizeOrder(deliverer, Array.from(rh), uri).accountsStrict({ caller, config: CONFIG, order: orderPda(nonce), instructions: SYSVAR_INSTRUCTIONS_PUBKEY }).instruction();
const expireIx = (nonce, caller = buyer.publicKey) =>
  sf.methods.expireOrder().accountsStrict({ caller, config: CONFIG, order: orderPda(nonce), instructions: SYSVAR_INSTRUCTIONS_PUBKEY }).instruction();

// "Reader program" view: decode an Order from raw bytes at the fixed offsets
// published in storefront::layout, without the IDL.
const L = { STATUS: 9, VENUE: 10, NONCE: 12, JOB_ID: 20, BUYER: 52, RECIPIENT: 84, BODY_HASH: 116, SOL_SPENT: 148, TOKENS: 156, W: 164, P: 172,
  DELIVERER: 196, RESULT_HASH: 228, CLOSED_SLOT: 260, RESULT_URI_LEN: 268, RESULT_URI: 270, SPEC: 470, LEN: 1070 };
function readOrderRaw(d) {
  assert.ok(d.length >= L.LEN, 'order length');
  assert.deepEqual(d.subarray(0, 8), sha256(Buffer.from('account:Order')).subarray(0, 8), 'Order discriminator');
  const S = L.SPEC;
  const evc = d[S + 141];
  return {
    status: d[L.STATUS], venue: d[L.VENUE], nonce: d.readBigUInt64LE(L.NONCE), jobId: d.subarray(L.JOB_ID, L.JOB_ID + 32),
    buyer: new PublicKey(d.subarray(L.BUYER, L.BUYER + 32)), recipient: new PublicKey(d.subarray(L.RECIPIENT, L.RECIPIENT + 32)),
    bodyHash: d.subarray(L.BODY_HASH, L.BODY_HASH + 32), solSpent: d.readBigUInt64LE(L.SOL_SPENT), tokens: d.readBigUInt64LE(L.TOKENS),
    w: d.readBigUInt64LE(L.W), p: d.readBigUInt64LE(L.P), deliverer: new PublicKey(d.subarray(L.DELIVERER, L.DELIVERER + 32)),
    resultHash: d.subarray(L.RESULT_HASH, L.RESULT_HASH + 32), closedSlot: d.readBigUInt64LE(L.CLOSED_SLOT),
    resultUri: d.subarray(L.RESULT_URI, L.RESULT_URI + d.readUInt16LE(L.RESULT_URI_LEN)).toString(),
    m: d.readUInt32LE(S), n: d.readUInt32LE(S + 4), k: d.readUInt32LE(S + 8), config: d.subarray(S + 12, S + 64),
    hashA: d.subarray(S + 64, S + 96), hashB: d.subarray(S + 96, S + 128), deadlineBlocks: d.readBigUInt64LE(S + 128), maxBlocks: d.readUInt32LE(S + 136),
    threshold: d[S + 140], evaluators: Array.from({ length: evc }, (_, i) => new PublicKey(d.subarray(S + 142 + 32 * i, S + 174 + 32 * i))),
    dataUri: d.subarray(S + 400, S + 400 + d.readUInt16LE(S + 398)).toString(),
  };
}

const CU = {};

// ---------- tests (sequential, shared chain state)
test('setup: fixtures, coin on pump, bridge, storefront config, trader volume accumulators', async () => {
  loadFixtures();
  svm.addProgramFromFile(BRIDGE, path.join(TARGET, 'deploy/bridge.so'));
  svm.addProgramFromFile(SF, path.join(TARGET, 'deploy/storefront.so'));
  setTime(BigInt(Math.floor(Date.now() / 1000)));
  for (const k of [admin, buyer, trader, creator, ...attesters]) svm.airdrop(k.publicKey, BigInt(300 * LAMPORTS_PER_SOL));
  global = sdk.PUMP_SDK.decodeGlobal(info(pda(['global'], PUMP)));
  BUYBACK = global.buybackFeeRecipients[0];
  ok([await sdk.PUMP_SDK.createV2Instruction({ mint: MINT, name: 'Pearl Test', symbol: 'PRLT', uri: 'https://example.com/p.json', creator: creator.publicKey, user: creator.publicKey, mayhemMode: false })], [creator, mintKp], 'create_v2');
  ok([await br.methods.initialize([Keypair.generate().publicKey], 1).accountsStrict({ admin: admin.publicKey, config: BCFG, mint: MINT, vault: VAULT, tokenProgram: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId }).instruction()], [admin], 'bridge initialize');
  ok([await sf.methods.initialize(params({ mint: MINT, bridgeProgram: BRIDGE, minSolIn: bn(LAMPORTS_PER_SOL / 100), maxSolIn: bn(5 * LAMPORTS_PER_SOL) }), attesters.map((k) => k.publicKey), 2)
    .accountsStrict({ admin: admin.publicKey, config: CONFIG, trader: TRADER, systemProgram: SystemProgram.programId }).instruction()], [admin], 'storefront initialize');
  const c = config();
  assert.equal(c.admin.toBase58(), admin.publicKey.toBase58());
  assert.equal(c.threshold, 2);
  assert.equal(c.attesters.length, 3);
  assert.equal(c.protocolFeeBps, 2000);
  assert.equal(c.maxSlippageBps, 300);
  assert.ok(lamports(TRADER) >= 890_880n, 'trader PDA rent exempt');
  // Volume accumulators for the trader PDA (both venues), paid by the admin once.
  for (const [prog, ev] of [[PUMP, pumpEvent], [AMM, ammEvent]]) {
    const ix = new TransactionInstruction({
      programId: prog,
      keys: [
        { pubkey: admin.publicKey, isSigner: true, isWritable: true }, { pubkey: TRADER, isSigner: false, isWritable: false },
        { pubkey: pda(['user_volume_accumulator', TRADER], prog), isSigner: false, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false }, { pubkey: ev, isSigner: false, isWritable: false }, { pubkey: prog, isSigner: false, isWritable: false },
      ],
      data: Buffer.from([94, 6, 202, 115, 255, 96, 232, 183]),
    });
    ok([ix], [admin], 'init_user_volume_accumulator');
  }
});

test('admin-only config: a stranger cannot update config or attesters', async () => {
  fails([await updateIx({ maxSolIn: bn(1) }, buyer.publicKey)], [buyer], /Unauthorized/, 'stranger update_config');
  const ix = await sf.methods.setAttesters([buyer.publicKey], 1).accountsStrict({ admin: buyer.publicKey, config: CONFIG }).instruction();
  fails([ix], [buyer], /Unauthorized/, 'stranger set_attesters');
  fails([await updateIx({ protocolFeeBps: 10_000 })], [admin], /BadConfig/, 'fee bps 100%');
  fails([await updateIx({ minSolIn: bn(10 * LAMPORTS_PER_SOL) })], [admin], /BadConfig/, 'min above max');
});

let nonce0, spec0;
test('buy_job on the curve: buys, splits W/P, deposits to the bridge, writes the Order', async () => {
  spec0 = specArgs();
  const { ix: open, draft } = await openIx(spec0);
  ok([open], [buyer], 'open_order');
  const SOL_IN = 1n * BigInt(LAMPORTS_PER_SOL);
  const n = bridgeNonce();
  nonce0 = n;
  const bc = curve();
  const expected = BigInt(sdk.getBuyTokenAmountFromSolAmount({ global, feeConfig: sdk.PUMP_SDK.decodeFeeConfig(info(pda(['fee_config', PUMP], FEE))), mintSupply: null, bondingCurve: bc, amount: bn(SOL_IN), quoteMint: WSOL }).toString());
  const b0 = lamports(buyer.publicKey), t0 = lamports(TRADER);
  const r = ok([await buyIx(draft, n, SOL_IN, (expected * 98n) / 100n)], [buyer], 'buy_job');
  CU.buy_job = r.cu;
  console.log(`  buy_job CU ${r.cu}, tx ${r.size} bytes`);
  assert.equal(svm.getAccount(draft), null, 'draft closed');
  assert.equal(lamports(TRADER), t0, 'trader PDA keeps exactly its balance');
  assert.equal(tokenAmount(TRADER_ATA), 0n, 'nothing left in the trader ATA');
  const o = order(n);
  const tokens = BigInt(o.tokens.toString());
  assert.equal(tokenAmount(VAULT), tokens, 'all bought tokens in the bridge vault');
  assert.ok(tokens >= (expected * 99n) / 100n && tokens <= (expected * 101n) / 100n, `tokens ${tokens} vs SDK ${expected}`);
  const spent = BigInt(o.solSpent.toString());
  assert.ok(spent > 0n && spent <= SOL_IN);
  assert.ok(b0 - lamports(buyer.publicKey) < SOL_IN + 20_000_000n, 'buyer paid sol_in + rents only');
  // fields
  assert.equal(o.version, 1);
  assert.equal(o.status, 1);
  assert.equal(o.venue, 0);
  assert.equal(o.depositNonce.toString(), n.toString());
  assert.deepEqual(Buffer.from(o.jobId), jobId(n));
  assert.equal(o.buyer.toBase58(), buyer.publicKey.toBase58());
  assert.equal(o.recipient.toBase58(), buyer.publicKey.toBase58());
  const p = (tokens * 2000n) / 10_000n, w = tokens - p;
  assert.equal(o.workerPay.toString(), (w * 1000n).toString(), 'W in chain units');
  assert.equal(o.protocolFee.toString(), (p * 1000n).toString(), 'P in chain units');
  assert.equal(BigInt(o.workerPay.toString()) + BigInt(o.protocolFee.toString()), tokens * 1000n, 'W + P = amount x 1000');
  assert.deepEqual(Buffer.from(o.bodyHash), sha256(postJobBody(spec0, w * 1000n, p * 1000n)), 'body hash = sha256(PostJob body)');
  assert.equal(o.spec.m, 256); assert.equal(o.spec.n, 256); assert.equal(o.spec.k, 1024);
  assert.deepEqual(o.spec.config, spec0.config);
  assert.deepEqual(o.spec.hashA, spec0.hashA);
  assert.equal(o.spec.deadlineBlocks.toString(), '500');
  assert.equal(o.spec.maxBlocks, 100);
  assert.equal(o.spec.threshold, 2);
  assert.equal(o.spec.evaluatorCount, 3);
  assert.equal(Buffer.from(o.spec.dataUri).subarray(0, o.spec.dataUriLen).toString(), spec0.dataUri);
  // bridge Deposit links back to the Order
  const d = br.coder.accounts.decode('deposit', Buffer.from(svm.getAccount(pda(['deposit', le64(n)], BRIDGE)).data));
  assert.equal(d.sender.toBase58(), TRADER.toBase58(), 'deposit sender = storefront trader PDA');
  assert.equal(d.recipient.toBase58(), buyer.publicKey.toBase58());
  assert.equal(d.amount.toString(), tokens.toString());
  assert.equal(bridgeNonce(), n + 1n);
  const c = config();
  assert.equal(c.orders.toString(), '1');
  console.log(`  order: ${tokens} SPL units for ${spent} lamports, W ${o.workerPay} P ${o.protocolFee}`);
});

test('reader: another program can read the Order from raw bytes at fixed offsets', () => {
  const raw = orderRaw(nonce0);
  const o = order(nonce0);
  const v = readOrderRaw(raw);
  assert.equal(raw.length, 1070);
  assert.equal(v.status, o.status);
  assert.equal(v.nonce, BigInt(o.depositNonce.toString()));
  assert.deepEqual(v.jobId, Buffer.from(o.jobId));
  assert.equal(v.buyer.toBase58(), o.buyer.toBase58());
  assert.equal(v.recipient.toBase58(), o.recipient.toBase58());
  assert.deepEqual(v.bodyHash, Buffer.from(o.bodyHash));
  assert.equal(v.tokens, BigInt(o.tokens.toString()));
  assert.equal(v.w, BigInt(o.workerPay.toString()));
  assert.equal(v.p, BigInt(o.protocolFee.toString()));
  assert.equal(v.solSpent, BigInt(o.solSpent.toString()));
  assert.equal(v.m, o.spec.m); assert.equal(v.n, o.spec.n); assert.equal(v.k, o.spec.k);
  assert.deepEqual(v.config, Buffer.from(o.spec.config));
  assert.deepEqual(v.hashB, Buffer.from(o.spec.hashB));
  assert.equal(v.deadlineBlocks, 500n);
  assert.equal(v.maxBlocks, o.spec.maxBlocks);
  assert.equal(v.threshold, 2);
  assert.deepEqual(v.evaluators.map(String), attesters.map((k) => k.publicKey.toBase58()));
  assert.equal(v.dataUri, spec0.dataUri);
  assert.equal(svm.getAccount(orderPda(nonce0)).owner.toBase58(), SF.toBase58(), 'owner check a reader must do');
});

test('buy_job rejections: caps, pause, slippage, min-out, wrong nonce, bad specs', async () => {
  const { ix: open, draft } = await openIx();
  ok([open], [buyer], 'open_order');
  const n = bridgeNonce();
  fails([await buyIx(draft, n, LAMPORTS_PER_SOL / 1000)], [buyer], /BelowMinOrder/, 'below min');
  fails([await buyIx(draft, n, 6 * LAMPORTS_PER_SOL)], [buyer], /AboveMaxOrder/, 'above max');
  ok([await updateIx({ maxSolIn: bn(LAMPORTS_PER_SOL / 2) })], [admin], 'lower max');
  fails([await buyIx(draft, n, LAMPORTS_PER_SOL)], [buyer], /AboveMaxOrder/, 'above edited max');
  ok([await updateIx({ maxSolIn: bn(5 * LAMPORTS_PER_SOL), paused: true })], [admin], 'pause');
  fails([await buyIx(draft, n, LAMPORTS_PER_SOL / 10)], [buyer], /Paused/, 'paused');
  ok([await updateIx({ paused: false, maxSlippageBps: 50 })], [admin], 'unpause, slippage 0.5%');
  fails([await buyIx(draft, n, LAMPORTS_PER_SOL / 10)], [buyer], /Slippage|TooLittle|slippage|0x1772|custom program error/, 'slippage tighter than the 1.25% pump fee');
  ok([await updateIx({ maxSlippageBps: 300 })], [admin], 'restore slippage');
  fails([await buyIx(draft, n, LAMPORTS_PER_SOL / 10, 10n ** 15n)], [buyer], null, 'min_tokens_out above the fill');
  fails([await buyIx(draft, n + 1n, LAMPORTS_PER_SOL / 10)], [buyer], /WrongNonce/, 'wrong nonce');
  // someone else cannot spend my draft
  const other = Keypair.generate();
  svm.airdrop(other.publicKey, BigInt(5 * LAMPORTS_PER_SOL));
  const ixOther = await sf.methods.buyJob(bn(n), bn(LAMPORTS_PER_SOL / 10), bn(1)).accountsStrict(curveAccts(draft, n, other.publicKey)).instruction();
  fails([ixOther], [other], /ConstraintSeeds|Unauthorized|seeds constraint/, 'stranger spends draft');
  // bad specs rejected at open_order
  for (const [label, s] of [
    ['m = 0', specArgs({ m: 0 })],
    ['k above max_dim', specArgs({ k: 9000 })],
    ['threshold above evaluators', specArgs({ threshold: 4 })],
    ['no evaluators', specArgs({ evaluators: [], threshold: 1 })],
    ['duplicate evaluators', specArgs({ evaluators: [attesters[0].publicKey, attesters[0].publicKey] })],
    ['max_blocks 0', specArgs({ maxBlocks: 0 })],
    ['deadline above cap', specArgs({ deadlineBlocks: bn(100_001) })],
  ]) fails([(await openIx(s)).ix], [buyer], /BadSpec/, label);
  fails([(await openIx(specArgs({ dataUri: 'x'.repeat(201) }))).ix], [buyer], /UriTooLong|BadSpec|exceed|too large/i, 'uri too long');
  ok([await updateIx({ maxDim: 16384 })], [admin], 'raise max_dim');
  ok([(await openIx(specArgs({ k: 9000 }))).ix], [buyer], 'k 9000 ok after edit');
  // a good buy still works with the first draft afterwards
  ok([await buyIx(draft, n, LAMPORTS_PER_SOL / 10)], [buyer], 'buy after rejections');
  assert.equal(order(n).status, 1);
});

test('finalize_order: missing, bad and insufficient signatures are rejected', async () => {
  const jid = jobId(nonce0);
  const deliverer = Keypair.generate().publicKey;
  const rh = sha256(Buffer.from('result C'));
  const msg = resultMsg(jid, deliverer, rh);
  const fin = await finalizeIx(nonce0, deliverer, rh, 'ipfs://result');
  fails([fin], [buyer], /NotEnoughSignatures/, 'no signatures');
  fails([edIx(attesters[0], msg), fin], [buyer], /NotEnoughSignatures/, '1 of 2');
  fails([edIx(attesters[0], msg), edIx(attesters[0], msg), fin], [buyer], /NotEnoughSignatures/, 'same attester twice');
  const outsider = Keypair.generate();
  fails([edIx(attesters[0], msg), edIx(outsider, msg), fin], [buyer], /NotEnoughSignatures/, 'non-attester signer');
  const wrong = resultMsg(jid, deliverer, sha256(Buffer.from('other')));
  fails([edIx(attesters[0], wrong), edIx(attesters[1], wrong), fin], [buyer], /NotEnoughSignatures/, 'sigs over another result hash');
  const wrongJob = resultMsg(jobId(nonce0 + 1n), deliverer, rh);
  fails([edIx(attesters[0], wrongJob), edIx(attesters[1], wrongJob), fin], [buyer], /NotEnoughSignatures/, 'sigs for another job');
  // a corrupted signature fails in the precompile itself
  const bad = edIx(attesters[1], msg);
  bad.data[bad.data.length - msg.length - 1] ^= 1;
  fails([edIx(attesters[0], msg), bad, fin], [buyer], /signature|Ed25519|InvalidSignature|PrecompileVerification|custom/i, 'corrupted signature');
  // caller must be buyer, admin or an attester
  const stranger = Keypair.generate();
  svm.airdrop(stranger.publicKey, BigInt(LAMPORTS_PER_SOL));
  fails([edIx(attesters[0], msg), edIx(attesters[1], msg), await finalizeIx(nonce0, deliverer, rh, 'ipfs://junk', stranger.publicKey)], [stranger], /Unauthorized/, 'stranger caller');
  assert.equal(order(nonce0).status, 1, 'still pending');
});

test('finalize_order: 2-of-3 attesters record the result; replay is refused', async () => {
  const jid = jobId(nonce0);
  const deliverer = Keypair.generate().publicKey;
  const rh = sha256(Buffer.from('result C'));
  const msg = resultMsg(jid, deliverer, rh);
  const r = ok([edIx(attesters[0], msg), edIx(attesters[2], msg), await finalizeIx(nonce0, deliverer, rh, 'ipfs://result-c')], [buyer], 'finalize');
  CU.finalize_order = r.cu;
  console.log(`  finalize_order CU ${r.cu} (2 sigs, incl. compute budget ix)`);
  const o = order(nonce0);
  assert.equal(o.status, 2);
  assert.equal(o.deliverer.toBase58(), deliverer.toBase58());
  assert.deepEqual(Buffer.from(o.resultHash), rh);
  assert.ok(BigInt(o.closedSlot.toString()) > 0n);
  const v = readOrderRaw(orderRaw(nonce0));
  assert.equal(v.status, 2);
  assert.equal(v.resultUri, 'ipfs://result-c');
  assert.deepEqual(v.resultHash, rh);
  assert.equal(v.deliverer.toBase58(), deliverer.toBase58());
  // replay: same sigs again, or another result, or expire
  fails([edIx(attesters[0], msg), edIx(attesters[2], msg), await finalizeIx(nonce0, deliverer, rh, 'ipfs://result-c')], [buyer], /NotPending/, 'replay finalize');
  fails([edIx(attesters[0], expiredMsg(jid)), edIx(attesters[1], expiredMsg(jid)), await expireIx(nonce0)], [buyer], /NotPending/, 'expire after finalize');
  // the same signatures do not finalize another order
  const n2 = nonce0 + 1n;
  fails([edIx(attesters[0], msg), edIx(attesters[2], msg), await finalizeIx(n2, deliverer, rh, 'ipfs://result-c')], [buyer], /NotEnoughSignatures/, 'sigs reused on another order');
});

test('expire_order: attested expiry is recorded; finalize afterwards is refused', async () => {
  const n2 = nonce0 + 1n;
  const jid = jobId(n2);
  fails([edIx(attesters[0], expiredMsg(jid)), await expireIx(n2)], [buyer], /NotEnoughSignatures/, 'expire 1 of 2');
  const r = ok([edIx(attesters[1], expiredMsg(jid)), edIx(attesters[2], expiredMsg(jid)), await expireIx(n2, attesters[1].publicKey)], [attesters[1]], 'expire by attester');
  console.log(`  expire_order CU ${r.cu}`);
  assert.equal(order(n2).status, 3);
  const d = Keypair.generate().publicKey, rh = sha256(Buffer.from('late'));
  const msg = resultMsg(jid, d, rh);
  fails([edIx(attesters[0], msg), edIx(attesters[1], msg), await finalizeIx(n2, d, rh, '')], [buyer], /NotPending/, 'finalize after expire');
});

test('admin rotates the attester set; old attesters no longer count', async () => {
  const fresh = [Keypair.generate(), Keypair.generate()];
  ok([await sf.methods.setAttesters(fresh.map((k) => k.publicKey), 2).accountsStrict({ admin: admin.publicKey, config: CONFIG }).instruction()], [admin], 'set_attesters');
  const { ix: open, draft } = await openIx();
  ok([open], [buyer], 'open');
  const n = bridgeNonce();
  ok([await buyIx(draft, n, LAMPORTS_PER_SOL / 10)], [buyer], 'buy');
  const d = Keypair.generate().publicKey, rh = sha256(Buffer.from('r'));
  const msg = resultMsg(jobId(n), d, rh);
  fails([edIx(attesters[0], msg), edIx(attesters[1], msg), await finalizeIx(n, d, rh, '')], [buyer], /NotEnoughSignatures/, 'old set');
  ok([edIx(fresh[0], msg), edIx(fresh[1], msg), await finalizeIx(n, d, rh, '')], [buyer], 'new set');
  ok([await sf.methods.setAttesters(attesters.map((k) => k.publicKey), 2).accountsStrict({ admin: admin.publicKey, config: CONFIG }).instruction()], [admin], 'restore');
});

let graduated = false;
test('graduation: curve completes + migrate_v2; buy_job then refuses', async () => {
  const { ix: open, draft } = await openIx();
  ok([open], [buyer], 'open');
  let bc = curve();
  const big = bc.realTokenReserves.add(new BN('20000000000000'));
  const tAta = ata(trader.publicKey, MINT, T22);
  ok(await sdk.PUMP_SDK.buyV3Instructions({ bondingCurve: bc, associatedUserAccountInfo: info(tAta), mint: MINT, user: trader.publicKey, amount: big, quoteAmount: new BN(150 * LAMPORTS_PER_SOL), slippage: 0, tokenProgram: T22 }), [trader], 'buy past curve end');
  assert.equal(curve().complete, true);
  ok([await sdk.PUMP_SDK.migrateV2Instruction({ withdrawAuthority: global.withdrawAuthority, mint: MINT, user: trader.publicKey, quoteMint: WSOL, baseTokenProgram: T22, quoteTokenProgram: TOK })], [trader], 'migrate_v2');
  fails([await buyIx(draft, bridgeNonce(), LAMPORTS_PER_SOL / 10)], [buyer], /CurveComplete/, 'curve buy after graduation');
  graduated = true;
});

test('buy_job_amm after graduation: PumpSwap buy, W/P split, deposit, Order venue 1', async (t) => {
  if (!graduated) return t.skip('no graduation');
  const { ix: open, draft } = await openIx(specArgs({ dataUri: '', evaluators: [attesters[0].publicKey], threshold: 1 }), Keypair.generate().publicKey);
  ok([open], [buyer], 'open');
  const n = bridgeNonce();
  const v0 = tokenAmount(VAULT), t0 = lamports(TRADER), b0 = lamports(buyer.publicKey);
  const SOL_IN = 2n * BigInt(LAMPORTS_PER_SOL);
  const r = ok([await buyAmmIx(draft, n, SOL_IN)], [buyer], 'buy_job_amm');
  CU.buy_job_amm = r.cu;
  console.log(`  buy_job_amm CU ${r.cu}, tx ${r.size} bytes`);
  const o = order(n);
  assert.equal(o.venue, 1);
  assert.equal(o.status, 1);
  const tokens = BigInt(o.tokens.toString());
  assert.ok(tokens > 0n);
  assert.equal(tokenAmount(VAULT) - v0, tokens);
  assert.equal(lamports(TRADER), t0, 'trader PDA balance unchanged');
  assert.equal(svm.getAccount(TRADER_WSOL), null, 'wSOL ATA closed');
  const spent = BigInt(o.solSpent.toString());
  assert.ok(spent > 0n && spent <= SOL_IN);
  assert.ok(b0 - lamports(buyer.publicKey) < spent + 20_000_000n, 'unspent SOL and ATA rent back to the buyer');
  assert.equal(BigInt(o.workerPay.toString()) + BigInt(o.protocolFee.toString()), tokens * 1000n);
  const d = br.coder.accounts.decode('deposit', Buffer.from(svm.getAccount(pda(['deposit', le64(n)], BRIDGE)).data));
  assert.equal(d.sender.toBase58(), TRADER.toBase58());
  assert.equal(d.recipient.toBase58(), o.recipient.toBase58());
  // slippage on the pool path
  const { ix: open2, draft: d2 } = await openIx();
  ok([open2], [buyer], 'open 2');
  ok([await updateIx({ maxSlippageBps: 10 })], [admin], 'tight');
  fails([await buyAmmIx(d2, bridgeNonce(), LAMPORTS_PER_SOL / 10)], [buyer], null, 'amm tight slippage');
  ok([await updateIx({ maxSlippageBps: 300 })], [admin], 'restore');
  ok([await buyAmmIx(d2, bridgeNonce(), LAMPORTS_PER_SOL / 10)], [buyer], 'amm small buy');
  console.log(`  amm order: ${tokens} SPL units for ${spent} lamports`);
});

test('cancel_draft returns the rent', async () => {
  const { ix: open, draft } = await openIx();
  ok([open], [buyer], 'open');
  const rent = lamports(draft);
  assert.ok(rent > 0n);
  ok([await sf.methods.cancelDraft().accountsStrict({ buyer: buyer.publicKey, draft }).instruction()], [buyer], 'cancel');
  assert.equal(svm.getAccount(draft), null);
});

test('CU summary', () => {
  console.log('  CU', JSON.stringify(CU));
  assert.ok(CU.buy_job < 400_000 && CU.finalize_order < 100_000);
});
