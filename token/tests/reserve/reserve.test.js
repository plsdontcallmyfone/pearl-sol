// LiteSVM tests (in-process, no ports) for the reserve program against the
// pinned pump.fun mainnet bytecode and accounts in fixtures/pump.
// Run: cd token && anchor build -p reserve && cd tests/reserve && npm i && npm test
// fund_mining also needs token/target/deploy/bridge.so + target/idl/bridge.json
// (the bridge workstream's build); that test is skipped when they are missing.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');
const { PublicKey, Keypair, Transaction, SystemProgram, ComputeBudgetProgram, LAMPORTS_PER_SOL, Connection } = require('@solana/web3.js');
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
const MAY = P('MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e');
const WSOL = spl.NATIVE_MINT, TOK = spl.TOKEN_PROGRAM_ID, T22 = spl.TOKEN_2022_PROGRAM_ID, ATAP = spl.ASSOCIATED_TOKEN_PROGRAM_ID;
const pda = (seeds, prog) => PublicKey.findProgramAddressSync(seeds.map((s) => (typeof s === 'string' ? Buffer.from(s) : Buffer.isBuffer(s) ? s : s.toBuffer())), prog)[0];
const ata = (owner, mint, tp) => spl.getAssociatedTokenAddressSync(mint, owner, true, tp);

const reserveIdl = JSON.parse(fs.readFileSync(path.join(TARGET, 'idl/reserve.json')));
const RESERVE = P(reserveIdl.address);
const CONFIG = pda(['config'], RESERVE);
const CREATOR = pda(['creator'], RESERVE);

const svm = new LiteSVM();
const admin = Keypair.generate(), trader = Keypair.generate(), cranker = Keypair.generate(), mintKp = Keypair.generate();
const MINT = mintKp.publicKey;
const provider = new anchor.AnchorProvider(new Connection('http://127.0.0.1:1'), new anchor.Wallet(admin), {});
const program = new anchor.Program(reserveIdl, provider);

// ---------- svm helpers
function loadFixtures() {
  for (const [n, id] of [['pump', PUMP], ['pump_amm', AMM], ['pump_fees', FEE]]) svm.addProgramFromFile(id, path.join(FIX, 'programs', n + '.so'));
  let n = 0;
  for (const f of fs.readdirSync(path.join(FIX, 'accounts'))) {
    const j = JSON.parse(fs.readFileSync(path.join(FIX, 'accounts', f)));
    const a = j.account;
    svm.setAccount(P(j.pubkey), { lamports: a.lamports, data: Buffer.from(a.data[0], 'base64'), owner: P(a.owner), executable: a.executable, rentEpoch: 0 });
    n++;
  }
  return n;
}
let now = BigInt(Math.floor(Date.now() / 1000));
function setTime(t) { now = t; const c = svm.getClock(); svm.setClock(new Clock(c.slot + 10n, c.epochStartTimestamp, c.epoch, c.leaderScheduleEpoch, t)); }
function send(ixs, signers) {
  const tx = new Transaction();
  tx.recentBlockhash = svm.latestBlockhash();
  tx.feePayer = signers[0].publicKey;
  tx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }), ...ixs);
  tx.sign(...signers);
  const r = svm.sendTransaction(tx);
  svm.expireBlockhash();
  if (typeof r.err === 'function') return { ok: false, err: String(r.err()), logs: r.meta().logs() };
  return { ok: true, cu: Number(r.computeUnitsConsumed()), logs: r.logs() };
}
function ok(ixs, signers, label) {
  const r = send(ixs, signers);
  if (!r.ok) assert.fail(`${label} failed: ${r.err}\n  ${r.logs.slice(-15).join('\n  ')}`);
  return r;
}
function fails(ixs, signers, pattern, label) {
  const r = send(ixs, signers);
  assert.equal(r.ok, false, `${label} should fail`);
  const text = r.logs.join('\n');
  if (pattern) assert.match(text, pattern, `${label}: expected ${pattern}, logs:\n${r.logs.slice(-10).join('\n')}`);
  return text;
}
const info = (k) => { const a = svm.getAccount(k); return a ? { lamports: Number(a.lamports), data: Buffer.from(a.data), owner: a.owner, executable: a.executable, rentEpoch: 0 } : null; };
const tokenAmount = (k) => { const a = svm.getAccount(k); return a ? Buffer.from(a.data).readBigUInt64LE(64) : 0n; };
const lamports = (k) => BigInt(svm.getBalance(k) ?? 0n);
const config = () => program.coder.accounts.decode('config', Buffer.from(svm.getAccount(CONFIG).data));

// ---------- pump account sets
const BC = sdk.bondingCurvePda(MINT);
const RESERVE_ATA = ata(CREATOR, MINT, T22);
const CREATOR_WSOL = ata(CREATOR, WSOL, TOK);
let BUYBACK; // set after fixtures load (from pump Global)
const pumpEvent = pda(['__event_authority'], PUMP);
function curveAccounts(signer) {
  const cv = sdk.creatorVaultPda(CREATOR);
  return {
    cranker: signer, config: CONFIG, creator: CREATOR, mint: MINT, reserveAta: RESERVE_ATA, creatorWsol: CREATOR_WSOL,
    pumpGlobal: pda(['global'], PUMP), bondingCurve: BC, curveBaseVault: ata(BC, MINT, T22), curveQuoteVault: ata(BC, WSOL, TOK),
    creatorVault: cv, creatorVaultWsol: ata(cv, WSOL, TOK), userVolumeAccumulator: pda(['user_volume_accumulator', CREATOR], PUMP),
    feeConfig: pda(['fee_config', PUMP], FEE), buybackFeeRecipient: BUYBACK, wsolMint: WSOL, tokenProgram: TOK, token2022Program: T22,
    associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, pumpEventAuthority: pumpEvent, pumpProgram: PUMP,
  };
}
const off = new swap.PumpAmmSdk();
function ammAccounts(signer) {
  const pool = sdk.canonicalPumpPoolPda(MINT);
  const pl = off.decodePool(info(pool));
  const gc = off.decodeGlobalConfig(info(pda(['global_config'], AMM)));
  const vaultAuth = pda(['creator_vault', CREATOR], AMM);
  return {
    cranker: signer, config: CONFIG, creator: CREATOR, mint: MINT, reserveAta: RESERVE_ATA, creatorWsol: CREATOR_WSOL, bondingCurve: BC,
    pool, poolBaseVault: pl.poolBaseTokenAccount, poolQuoteVault: pl.poolQuoteTokenAccount, ammGlobalConfig: pda(['global_config'], AMM),
    coinCreatorVaultAuthority: vaultAuth, coinCreatorVaultAta: ata(vaultAuth, WSOL, TOK),
    ammUserVolumeAccumulator: pda(['user_volume_accumulator', CREATOR], AMM), ammFeeConfig: pda(['fee_config', AMM], FEE),
    ammBuybackFeeRecipient: ata(gc.buybackFeeRecipients[0], WSOL, TOK), wsolMint: WSOL, tokenProgram: TOK, token2022Program: T22,
    associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, ammEventAuthority: pda(['__event_authority'], AMM), ammProgram: AMM,
  };
}
const crankIx = (maxIn, minOut, signer = cranker.publicKey) => program.methods.crankBuyback(new BN(maxIn.toString()), new BN(minOut.toString())).accountsStrict(curveAccounts(signer)).instruction();
const crankAmmIx = (maxIn, minOut, signer = cranker.publicKey) => program.methods.crankBuybackAmm(new BN(maxIn.toString()), new BN(minOut.toString())).accountsStrict(ammAccounts(signer)).instruction();
const params = (p) => ({ mint: null, cranker: null, funder: null, bridgeProgram: null, miningPool: null, maxBuyPerCall: null, minBuy: null, minIntervalSecs: null, maxSlippageBps: null, solFloor: null, maxFundPerCall: null, ...p });
const updateIx = (p) => program.methods.updateConfig(params(p)).accountsStrict({ admin: admin.publicKey, config: CONFIG }).instruction();
const bn = (x) => new BN(x.toString());
let global, feeConfig;
const curve = () => sdk.PUMP_SDK.decodeBondingCurve(info(BC));

// ---------- tests (sequential, shared chain state)
test('fixtures load and reserve deploys', () => {
  const n = loadFixtures();
  assert.ok(n >= 40, `loaded ${n} fixture accounts`);
  svm.addProgramFromFile(RESERVE, path.join(TARGET, 'deploy/reserve.so'));
  setTime(now);
  for (const k of [admin, trader, cranker]) svm.airdrop(k.publicKey, BigInt(200 * LAMPORTS_PER_SOL));
  global = sdk.PUMP_SDK.decodeGlobal(info(pda(['global'], PUMP)));
  feeConfig = sdk.PUMP_SDK.decodeFeeConfig(info(pda(['fee_config', PUMP], FEE)));
  BUYBACK = global.buybackFeeRecipients[0];
});

test('initialize config', async () => {
  const ix = await program.methods
    .initialize(params({ maxBuyPerCall: bn(LAMPORTS_PER_SOL / 2), minIntervalSecs: bn(600), maxSlippageBps: 300 }))
    .accountsStrict({ admin: admin.publicKey, config: CONFIG, creator: CREATOR, systemProgram: SystemProgram.programId })
    .instruction();
  ok([ix], [admin], 'initialize');
  const c = config();
  assert.equal(c.admin.toBase58(), admin.publicKey.toBase58());
  assert.equal(c.maxBuyPerCall.toString(), String(LAMPORTS_PER_SOL / 2));
  assert.equal(c.maxSlippageBps, 300);
  assert.equal(c.mint.toBase58(), PublicKey.default.toBase58());
});

test('non-admin cannot update config', async () => {
  const ix = await program.methods.updateConfig(params({ maxBuyPerCall: bn(1) })).accountsStrict({ admin: trader.publicKey, config: CONFIG }).instruction();
  fails([ix], [trader], /Unauthorized|signer is not allowed/, 'trader update_config');
});

const DEV_BUY = 3n * BigInt(LAMPORTS_PER_SOL);
test('launch: create_v2 with the creator PDA as creator + 3 SOL dev buy into the reserve', async () => {
  const expected = sdk.getBuyTokenAmountFromSolAmount({ global, feeConfig, mintSupply: null, bondingCurve: null, amount: bn(DEV_BUY), quoteMint: WSOL });
  const minOut = expected.muln(99).divn(100);
  const solVault = pda(['sol-vault'], MAY);
  const ix = await program.methods
    .launch('Pearl Test', 'PRLT', 'https://example.com/pearl.json', bn(DEV_BUY), minOut)
    .accountsStrict({
      admin: admin.publicKey, config: CONFIG, creator: CREATOR, mint: MINT, mintAuthority: pda(['mint-authority'], PUMP), bondingCurve: BC,
      curveBaseVault: ata(BC, MINT, T22), curveQuoteVault: ata(BC, WSOL, TOK), pumpGlobal: pda(['global'], PUMP), reserveAta: RESERVE_ATA,
      creatorWsol: CREATOR_WSOL, userVolumeAccumulator: pda(['user_volume_accumulator', CREATOR], PUMP), feeConfig: pda(['fee_config', PUMP], FEE),
      buybackFeeRecipient: BUYBACK, mayhemProgram: MAY, mayhemGlobalParams: pda(['global-params'], MAY), mayhemSolVault: solVault,
      mayhemState: pda(['mayhem-state', MINT], MAY), mayhemTokenVault: ata(solVault, MINT, T22), wsolMint: WSOL, tokenProgram: TOK,
      token2022Program: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, pumpEventAuthority: pumpEvent, pumpProgram: PUMP,
    })
    .instruction();
  const r = ok([ix], [admin, mintKp], 'launch');
  const got = tokenAmount(RESERVE_ATA);
  console.log(`  launch CU ${r.cu}, reserve got ${got} base units (SDK quote ${expected})`);
  assert.ok(got >= BigInt(minOut.toString()), 'dev buy tokens in reserve');
  assert.ok(got > 90_000_000_000_000n && got < 100_000_000_000_000n, 'about 96M tokens for 3 SOL');
  const bc = curve();
  assert.equal(bc.creator.toBase58(), CREATOR.toBase58(), 'curve creator = reserve PDA');
  const m = spl.unpackMint(MINT, { ...svm.getAccount(MINT), data: Buffer.from(svm.getAccount(MINT).data) }, T22);
  assert.equal(m.supply, 1_000_000_000_000_000n);
  assert.equal(m.mintAuthority, null);
  assert.equal(m.freezeAuthority, null);
  const c = config();
  assert.equal(c.mint.toBase58(), MINT.toBase58());
  const cr = svm.getAccount(CREATOR);
  assert.equal(cr.owner.toBase58(), SystemProgram.programId.toBase58(), 'creator PDA stays system-owned');
  assert.equal(cr.data.length, 0, 'creator PDA stays data-less');
});

test('launch cannot run twice', async () => {
  // reuse the same accounts with a fresh mint: the config already has a mint
  const m2 = Keypair.generate();
  const solVault = pda(['sol-vault'], MAY);
  const bc2 = sdk.bondingCurvePda(m2.publicKey);
  const ix = await program.methods
    .launch('X', 'X', 'https://x', bn(0), bn(0))
    .accountsStrict({
      admin: admin.publicKey, config: CONFIG, creator: CREATOR, mint: m2.publicKey, mintAuthority: pda(['mint-authority'], PUMP), bondingCurve: bc2,
      curveBaseVault: ata(bc2, m2.publicKey, T22), curveQuoteVault: ata(bc2, WSOL, TOK), pumpGlobal: pda(['global'], PUMP), reserveAta: ata(CREATOR, m2.publicKey, T22),
      creatorWsol: CREATOR_WSOL, userVolumeAccumulator: pda(['user_volume_accumulator', CREATOR], PUMP), feeConfig: pda(['fee_config', PUMP], FEE),
      buybackFeeRecipient: BUYBACK, mayhemProgram: MAY, mayhemGlobalParams: pda(['global-params'], MAY), mayhemSolVault: solVault,
      mayhemState: pda(['mayhem-state', m2.publicKey], MAY), mayhemTokenVault: ata(solVault, m2.publicKey, T22), wsolMint: WSOL, tokenProgram: TOK,
      token2022Program: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId, pumpEventAuthority: pumpEvent, pumpProgram: PUMP,
    })
    .instruction();
  fails([ix], [admin, m2], /AlreadyLaunched|already launched/, 'second launch');
});

async function traderVolume(sol) {
  // buy_v3 then sell_v3 half: curve keeps the creator fee until swept
  let bc = curve();
  const tokens = sdk.getBuyTokenAmountFromSolAmount({ global, feeConfig, mintSupply: null, bondingCurve: bc, amount: bn(sol), quoteMint: WSOL });
  const tAta = ata(trader.publicKey, MINT, T22);
  ok(await sdk.PUMP_SDK.buyV3Instructions({ bondingCurve: bc, associatedUserAccountInfo: info(tAta), mint: MINT, user: trader.publicKey, amount: tokens, quoteAmount: bn(sol).muln(3).divn(2), slippage: 50, tokenProgram: T22 }), [trader], 'trader buy_v3');
  bc = curve();
  ok(await sdk.PUMP_SDK.sellV3Instructions({ bondingCurve: bc, mint: MINT, user: trader.publicKey, amount: tokens.divn(2), quoteAmount: new BN(0), slippage: 100, tokenProgram: T22 }), [trader], 'trader sell_v3');
}

test('trades accrue creator fees on the curve', async () => {
  const before = BigInt(curve().creatorFee.toString());
  await traderVolume(5n * BigInt(LAMPORTS_PER_SOL));
  const after = BigInt(curve().creatorFee.toString());
  console.log(`  curve creator_fee waiting ${after}`);
  assert.ok(after > before + 10_000_000n, 'about 0.3% of ~7.5 SOL volume');
});

test('crank_buyback on the curve: collects fees and buys into the reserve', async () => {
  const tokens0 = tokenAmount(RESERVE_ATA), lam0 = lamports(CREATOR), fees = BigInt(curve().creatorFee.toString());
  const r = ok([await crankIx(LAMPORTS_PER_SOL, 1)], [cranker], 'crank_buyback');
  const got = tokenAmount(RESERVE_ATA) - tokens0;
  const c = config();
  console.log(`  crank CU ${r.cu}, swept ${fees} lamports, bought ${got} base units, spent ${c.totalSpent.toString()} total`);
  assert.ok(got > 0n, 'reserve grew');
  // what is left is the 0.30% creator fee of the buyback itself (comes back next round)
  const spentNow = BigInt(c.totalSpent.toString()) - DEV_BUY;
  assert.ok(BigInt(curve().creatorFee.toString()) <= (spentNow * 40n) / 10_000n, 'curve fee swept (only our own buy fee left)');
  assert.ok(lamports(CREATOR) >= 5_000_000n - 1n && lamports(CREATOR) <= lam0 + fees, 'PDA keeps sol_floor');
  assert.ok(Number(c.lastCrankTs) > 0);
  assert.equal(svm.getAccount(CREATOR).data.length, 0);
});

test('crank_buyback rejects inside the minimum interval', async () => {
  await traderVolume(2n * BigInt(LAMPORTS_PER_SOL));
  fails([await crankIx(LAMPORTS_PER_SOL, 1)], [cranker], /TooSoon|min_interval/, 'second crank');
  setTime(now + 601n);
});

test('crank_buyback rejects a min_tokens_out above the fill', async () => {
  fails([await crankIx(LAMPORTS_PER_SOL, 10n ** 15n)], [cranker], null, 'huge min_tokens_out');
});

test('crank_buyback rejects when max_slippage_bps is tighter than the fill (pump fee 1.25%)', async () => {
  ok([await updateIx({ maxSlippageBps: 50 })], [admin], 'set slippage 50');
  const t = fails([await crankIx(LAMPORTS_PER_SOL, 1)], [cranker], null, 'tight slippage');
  console.log('  rejected with:', (t.match(/Error Code: \w+|custom program error: 0x[0-9a-f]+/g) || []).slice(-1)[0]);
  ok([await updateIx({ maxSlippageBps: 300 })], [admin], 'restore slippage');
});

test('crank_buyback rejects a non-cranker when cranker is set', async () => {
  ok([await updateIx({ cranker: admin.publicKey })], [admin], 'set cranker');
  fails([await crankIx(LAMPORTS_PER_SOL, 1)], [cranker], /Unauthorized|signer is not allowed/, 'wrong cranker');
  ok([await updateIx({ cranker: PublicKey.default })], [admin], 'permissionless again');
});

test('per-call cap bounds the spend', async () => {
  const CAP = 5_000_000n;
  ok([await updateIx({ maxBuyPerCall: bn(CAP) })], [admin], 'set cap');
  const s0 = BigInt(config().totalSpent.toString()), t0 = tokenAmount(RESERVE_ATA);
  ok([await crankIx(LAMPORTS_PER_SOL, 1)], [cranker], 'capped crank');
  const spent = BigInt(config().totalSpent.toString()) - s0;
  console.log(`  capped crank spent ${spent} (cap ${CAP}), bought ${tokenAmount(RESERVE_ATA) - t0}`);
  assert.ok(spent > 0n && spent <= CAP);
  assert.ok(lamports(CREATOR) > 5_000_000n, 'uncapped remainder stays for the next crank');
  setTime(now + 601n);
  // max_sol_in below min_buy -> NothingToBuy
  fails([await crankIx(1000, 1)], [cranker], /NothingToBuy/, 'dust crank');
  ok([await updateIx({ maxBuyPerCall: bn(LAMPORTS_PER_SOL) })], [admin], 'restore cap');
});

test('collect_curve_fees is permissionless and only collects', async () => {
  await traderVolume(1n * BigInt(LAMPORTS_PER_SOL));
  const t0 = tokenAmount(RESERVE_ATA), l0 = lamports(CREATOR);
  const ix = await program.methods.collectCurveFees().accountsStrict(curveAccounts(trader.publicKey)).instruction();
  ok([ix], [trader], 'collect_curve_fees');
  assert.equal(tokenAmount(RESERVE_ATA), t0);
  assert.ok(lamports(CREATOR) > l0);
});

let graduated = false;
test('graduation: buy past the curve end + migrate_v2; curve crank then refuses', async () => {
  let bc = curve();
  const big = bc.realTokenReserves.add(new BN('20000000000000'));
  const tAta = ata(trader.publicKey, MINT, T22);
  ok(await sdk.PUMP_SDK.buyV3Instructions({ bondingCurve: bc, associatedUserAccountInfo: info(tAta), mint: MINT, user: trader.publicKey, amount: big, quoteAmount: new BN(120 * LAMPORTS_PER_SOL), slippage: 0, tokenProgram: T22 }), [trader], 'buy past curve end');
  assert.equal(curve().complete, true);
  ok([await sdk.PUMP_SDK.migrateV2Instruction({ withdrawAuthority: global.withdrawAuthority, mint: MINT, user: trader.publicKey, quoteMint: WSOL, baseTokenProgram: T22, quoteTokenProgram: TOK })], [trader], 'migrate_v2');
  const pl = off.decodePool(info(sdk.canonicalPumpPoolPda(MINT)));
  assert.equal(pl.coinCreator.toBase58(), CREATOR.toBase58(), 'pool coin_creator = reserve PDA');
  setTime(now + 601n);
  fails([await crankIx(LAMPORTS_PER_SOL, 1)], [cranker], /CurveComplete|complete/, 'curve crank after graduation');
  graduated = true;
});

test('crank_buyback_amm on PumpSwap after graduation', async (t) => {
  if (!graduated) return t.skip('no graduation');
  const fake = { getAccountInfo: async (k) => info(k), getMultipleAccountsInfo: async (ks) => ks.map(info) };
  const online = new swap.OnlinePumpAmmSdk(fake);
  const poolKey = sdk.canonicalPumpPoolPda(MINT);
  let st = await online.swapSolanaState(poolKey, trader.publicKey);
  ok(await off.buyV2Instructions(st, new BN('1000000000000'), new BN(5 * LAMPORTS_PER_SOL)), [trader], 'trader PumpSwap buy_v2');
  st = await online.swapSolanaState(poolKey, trader.publicKey);
  ok(await off.buyQuoteInput(st, new BN(2 * LAMPORTS_PER_SOL), 5), [trader], 'trader PumpSwap buy (legacy, fees per trade)');
  const poolFees = off.decodePool(info(poolKey)).creatorFees.toString();
  const t0 = tokenAmount(RESERVE_ATA), s0 = BigInt(config().totalSpent.toString());
  const r = ok([await crankAmmIx(LAMPORTS_PER_SOL, 1)], [cranker], 'crank_buyback_amm');
  const got = tokenAmount(RESERVE_ATA) - t0, spent = BigInt(config().totalSpent.toString()) - s0;
  console.log(`  amm crank CU ${r.cu}, pool creator_fees were ${poolFees}, spent ${spent} wSOL, bought ${got}`);
  assert.ok(got > 0n && spent > 0n && spent <= BigInt(LAMPORTS_PER_SOL));
  assert.ok(BigInt(off.decodePool(info(poolKey)).creatorFees.toString()) <= (spent * 100n) / 10_000n, 'pool creator fees swept (only our own buy fee left)');
  assert.ok(lamports(CREATOR) >= 890_880n, 'PDA rent exempt');
  // interval also applies on the pool path
  fails([await crankAmmIx(LAMPORTS_PER_SOL, 1)], [cranker], /TooSoon/, 'amm crank too soon');
  setTime(now + 601n);
  // slippage tighter than the PumpSwap fee tier
  st = await online.swapSolanaState(poolKey, trader.publicKey);
  ok(await off.buyV2Instructions(st, new BN('1000000000000'), new BN(5 * LAMPORTS_PER_SOL)), [trader], 'more volume');
  ok([await updateIx({ maxSlippageBps: 10 })], [admin], 'tight slippage');
  fails([await crankAmmIx(LAMPORTS_PER_SOL, 1)], [cranker], null, 'amm tight slippage');
  ok([await updateIx({ maxSlippageBps: 300 })], [admin], 'restore slippage');
  ok([await crankAmmIx(LAMPORTS_PER_SOL, 1)], [cranker], 'amm crank again');
});

test('fund_mining deposits reserve tokens into the bridge vault for the mining pool', async (t) => {
  const so = path.join(TARGET, 'deploy/bridge.so'), idlPath = path.join(TARGET, 'idl/bridge.json');
  if (!fs.existsSync(so) || !fs.existsSync(idlPath)) return t.skip('bridge not built');
  const bidl = JSON.parse(fs.readFileSync(idlPath));
  const BRIDGE = P(bidl.address);
  svm.addProgramFromFile(BRIDGE, so);
  const bridge = new anchor.Program(bidl, provider);
  const bcfg = pda(['config'], BRIDGE);
  const vault = ata(bcfg, MINT, T22);
  const relayer = Keypair.generate().publicKey;
  ok([await bridge.methods.initialize([relayer], 1).accountsStrict({ admin: admin.publicKey, config: bcfg, mint: MINT, vault, tokenProgram: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId }).instruction()], [admin], 'bridge initialize');
  const miningPool = Keypair.generate().publicKey;
  const funder = Keypair.generate();
  svm.airdrop(funder.publicKey, BigInt(LAMPORTS_PER_SOL));
  ok([await updateIx({ bridgeProgram: BRIDGE, miningPool, funder: funder.publicKey, maxFundPerCall: bn(50_000_000_000_000n) })], [admin], 'wire bridge');
  const fundIx = async (signer, amount) => {
    const bc = bridge.coder.accounts.decode('config', Buffer.from(svm.getAccount(bcfg).data));
    const dep = pda([Buffer.from('deposit'), new BN(bc.depositNonce.toString()).toArrayLike(Buffer, 'le', 8)], BRIDGE);
    return { dep, ix: await program.methods.fundMining(bn(amount)).accountsStrict({ authority: signer, config: CONFIG, creator: CREATOR, mint: MINT, reserveAta: RESERVE_ATA, bridgeConfig: bcfg, bridgeVault: vault, bridgeDeposit: dep, token2022Program: T22, systemProgram: SystemProgram.programId, bridgeProgram: BRIDGE }).instruction() };
  };
  const r0 = tokenAmount(RESERVE_ATA);
  const AMT = 10_000_000_000_000n; // 10M tokens
  const { dep, ix } = await fundIx(funder.publicKey, AMT);
  ok([ix], [funder], 'fund_mining by funder');
  assert.equal(tokenAmount(RESERVE_ATA), r0 - AMT);
  assert.equal(tokenAmount(vault), AMT);
  const d = bridge.coder.accounts.decode('deposit', Buffer.from(svm.getAccount(dep).data));
  assert.equal(d.recipient.toBase58(), miningPool.toBase58());
  assert.equal(d.sender.toBase58(), CREATOR.toBase58());
  assert.equal(d.amount.toString(), AMT.toString());
  assert.equal(config().totalFunded.toString(), AMT.toString());
  fails([(await fundIx(trader.publicKey, 1n)).ix], [trader], /Unauthorized/, 'fund_mining by stranger');
  fails([(await fundIx(admin.publicKey, 60_000_000_000_000n)).ix], [admin], /OverFundCap/, 'fund over cap');
  console.log(`  funded ${AMT}, reserve left ${tokenAmount(RESERVE_ATA)}`);
});
