// LiteSVM tests (in-process, no ports) for the mandate program (docs/TRADING.md, ECOSYSTEM 2.5,
// decision D7) against the pinned pump.fun / PumpSwap mainnet bytecode in fixtures/pump.
// Flow: a real pump.fun coin is created and graduated to its canonical PumpSwap pool; a user
// opens a trading account for an agent; the agent buys and sells inside the limits; then the
// exploit set: the agent tries to withdraw, to exceed every limit, to route output to itself,
// to call a fake venue, and to trade after pause, expiry, the kill switch, revoke and rotation.
// Withdrawals must work in every one of those states. Fees: realized SOL profit only, above a
// high-water mark, 1,000 bps of the fee to the buyback vault.
// Run: cd token && anchor build -p mandate && cd tests/mandate && npm ci && npm test
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');
const { PublicKey, Keypair, Transaction, SystemProgram, ComputeBudgetProgram, LAMPORTS_PER_SOL, Connection, TransactionInstruction } = require('@solana/web3.js');
const { LiteSVM, Clock } = require('litesvm');
const anchor = require('@coral-xyz/anchor');
const BN = require('bn.js');
const sdk = require('@pump-fun/pump-sdk');
const swap = require('@pump-fun/pump-swap-sdk');
const spl = require('@solana/spl-token');

const ROOT = path.resolve(__dirname, '../../..');
const FIX = path.join(ROOT, 'fixtures/pump');
const TARGET = process.env.TOKEN_TARGET || path.join(ROOT, 'token/target');
const P = (s) => new PublicKey(s);
const PUMP = P('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P');
const AMM = P('pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA');
const FEE = P('pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ');
const WSOL = spl.NATIVE_MINT, TOK = spl.TOKEN_PROGRAM_ID, T22 = spl.TOKEN_2022_PROGRAM_ID, ATAP = spl.ASSOCIATED_TOKEN_PROGRAM_ID;
const pda = (seeds, prog) => PublicKey.findProgramAddressSync(seeds.map((s) => (typeof s === 'string' ? Buffer.from(s) : Buffer.isBuffer(s) ? s : s.toBuffer())), prog)[0];
const ata = (owner, mint, tp) => spl.getAssociatedTokenAddressSync(mint, owner, true, tp);
const le64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const bn = (x) => new BN(x.toString());
const SOL = BigInt(LAMPORTS_PER_SOL);
const VAULT_RENT = 890_880n;

const idl = JSON.parse(fs.readFileSync(path.join(TARGET, 'idl/mandate.json')));
const MP = P(idl.address);
const CONFIG = pda(['mconfig'], MP);

const svm = new LiteSVM();
const admin = Keypair.generate(), user = Keypair.generate(), agent = Keypair.generate(), agent2 = Keypair.generate(), stranger = Keypair.generate();
const creator = Keypair.generate(), whale = Keypair.generate(), mintKp = Keypair.generate(), buybackVault = Keypair.generate(), feeTo = Keypair.generate();
const MINT = mintKp.publicKey;
const provider = new anchor.AnchorProvider(new Connection('http://127.0.0.1:1'), new anchor.Wallet(admin), {});
const mp = new anchor.Program(idl, provider);
const off = new swap.PumpAmmSdk();
const ammEvent = pda(['__event_authority'], AMM);

function loadFixtures() {
  for (const [n, id] of [['pump', PUMP], ['pump_amm', AMM], ['pump_fees', FEE]]) svm.addProgramFromFile(id, path.join(FIX, 'programs', n + '.so'));
  for (const f of fs.readdirSync(path.join(FIX, 'accounts'))) {
    const j = JSON.parse(fs.readFileSync(path.join(FIX, 'accounts', f)));
    const a = j.account;
    svm.setAccount(P(j.pubkey), { lamports: a.lamports, data: Buffer.from(a.data[0], 'base64'), owner: P(a.owner), executable: a.executable, rentEpoch: 0 });
  }
}
let now = BigInt(Math.floor(Date.now() / 1000));
function setTime(t) { now = t; const c = svm.getClock(); svm.setClock(new Clock(c.slot + 10n, c.epochStartTimestamp, c.epoch, c.leaderScheduleEpoch, t)); }
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
  return text;
}
const info = (k) => { const a = svm.getAccount(k); return a ? { lamports: Number(a.lamports), data: Buffer.from(a.data), owner: a.owner, executable: a.executable, rentEpoch: 0 } : null; };
const tokenAmount = (k) => { const a = svm.getAccount(k); return a && a.data.length >= 72 ? Buffer.from(a.data).readBigUInt64LE(64) : 0n; };
const lamports = (k) => BigInt(svm.getBalance(k) ?? 0n);
function events(logs) {
  const out = [];
  for (const l of logs) {
    const m = l.match(/^Program data: (.+)$/);
    if (!m) continue;
    const e = mp.coder.events.decode(m[1]);
    if (e) out.push(e);
  }
  return out;
}
const camel = (o) => Object.fromEntries(Object.entries(o).map(([k, v]) => [k.replace(/_([a-z])/g, (_, c) => c.toUpperCase()), v]));
const evOf = (r, name) => events(r.logs).filter((e) => e.name.toLowerCase() === name.toLowerCase()).map((e) => camel(e.data));

const mandatePda = (u, id) => pda(['mandate', u, le64(id)], MP);
const vaultPda = (m) => pda(['vault', m], MP);
const decodeMandate = (m) => mp.coder.accounts.decode('mandate', Buffer.from(svm.getAccount(m).data));

function limits(o = {}) {
  const mints = o.mints ?? [MINT];
  return {
    venues: 1 << 1, anyMint: false, nMints: mints.length, mints: [...mints, ...Array(4 - mints.length).fill(PublicKey.default)],
    maxInPerTrade: bn(SOL / 2n), windowCap: bn(2n * SOL), windowSecs: 86_400, maxTradesPerWindow: 20,
    maxSlippageBps: 500, minPoolQuoteReserve: bn(SOL), maxTradeShareBps: 1_000, expiryTs: bn(0), paused: false, ...o.l,
  };
}
const terms = (o = {}) => ({ perfFeeBps: 2_000, settleIntervalSecs: 3_600, ...o });

let M, V, global, gc, POOL, PL;
const ID = 7n;
function ammMetas(vault) {
  return [
    [POOL, true], [vault, true], [pda(['global_config'], AMM), false], [MINT, false], [WSOL, false],
    [ata(vault, MINT, T22), true], [ata(vault, WSOL, TOK), true], [PL.poolBaseTokenAccount, true], [PL.poolQuoteTokenAccount, true],
    [T22, false], [TOK, false], [SystemProgram.programId, false], [pda(['user_volume_accumulator', vault], AMM), true],
    [pda(['fee_config', AMM], FEE), false], [ata(gc.buybackFeeRecipients[0], WSOL, TOK), true], [ammEvent, false], [AMM, false],
  ].map(([pubkey, isWritable]) => ({ pubkey, isWritable, isSigner: false }));
}
function swapIx(side, amountIn, minOut, o = {}) {
  const signer = o.signer ?? agent.publicKey;
  const m = o.mandate ?? M, v = o.vault ?? V;
  const rem = o.rem ?? ammMetas(v);
  return mp.methods.swap(side, bn(amountIn), bn(minOut)).accountsStrict({
    agent: signer, config: CONFIG, mandate: m, vault: v, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId,
  }).remainingAccounts(rem).instruction();
}
// The same reserves the program reads (pump.rs read_pool: vault - protocol fees - creator fees + virtual quote).
function reserves() {
  const d = info(POOL).data;
  const base = tokenAmount(PL.poolBaseTokenAccount);
  let vq = d.length >= 261 ? d.readBigInt64LE(245) + (d.readBigInt64LE(253) << 64n) : 0n;
  if (vq < 0n) vq = 0n;
  const quote = tokenAmount(PL.poolQuoteTokenAccount) - d.readBigUInt64LE(271) - d.readBigUInt64LE(279) + vq;
  return { base, quote };
}
// The program's own fee-free quote, then a min_out inside its slippage band.
const cpOut = (rin, rout, x) => (rout * x) / (rin + x);
function buyMin(solIn, bps = 400) { const r = reserves(); return (cpOut(r.quote, r.base, solIn) * BigInt(10_000 - bps)) / 10_000n; }
function sellMin(tok, bps = 400) { const r = reserves(); return (cpOut(r.base, r.quote, tok) * BigInt(10_000 - bps)) / 10_000n; }
const userOp = (method, args, who = user.publicKey, m = M, v = V) =>
  mp.methods[method](...args).accountsStrict({ user: who, mandate: m, vault: v, systemProgram: SystemProgram.programId }).instruction();
const userOnly = (method, args, who = user.publicKey, m = M) => mp.methods[method](...args).accountsStrict({ user: who, mandate: m }).instruction();
const withdrawTokenIx = (amount, who = user.publicKey, m = M, v = V) =>
  mp.methods.withdrawToken(bn(amount)).accountsStrict({
    user: who, mandate: m, vault: v, mint: MINT, vaultAta: ata(v, MINT, T22), userAta: ata(who, MINT, T22), tokenProgram: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId,
  }).instruction();
const settleIx = (m = M, v = V, fee = feeTo.publicKey, bb = buybackVault.publicKey) =>
  mp.methods.settle().accountsStrict({ config: CONFIG, mandate: m, vault: v, buybackVault: bb, feeTo: fee, systemProgram: SystemProgram.programId }).instruction();
const setConfigIx = (p, who = admin.publicKey) => mp.methods.setConfig({ protocolBps: null, buybackVault: null, swapsPaused: null, admin: null, ...p }).accountsStrict({ admin: who, config: CONFIG }).instruction();
function uvaIx(payer, owner) {
  return new TransactionInstruction({
    programId: AMM,
    keys: [
      { pubkey: payer, isSigner: true, isWritable: true }, { pubkey: owner, isSigner: false, isWritable: false },
      { pubkey: pda(['user_volume_accumulator', owner], AMM), isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false }, { pubkey: ammEvent, isSigner: false, isWritable: false }, { pubkey: AMM, isSigner: false, isWritable: false },
    ],
    data: Buffer.from([94, 6, 202, 115, 255, 96, 232, 183]),
  });
}
async function openMandate(id, ag, lim = limits(), t = terms(), deposit = 3n * SOL, who = user) {
  const m = mandatePda(who.publicKey, id), v = vaultPda(m);
  const open = await mp.methods.open(bn(id), ag, feeTo.publicKey, lim, t, bn(deposit)).accountsStrict({ user: who.publicKey, mandate: m, vault: v, systemProgram: SystemProgram.programId }).instruction();
  const add = await mp.methods.addMint().accountsStrict({ payer: who.publicKey, mandate: m, vault: v, mint: MINT, vaultAta: ata(v, MINT, T22), tokenProgram: T22, associatedTokenProgram: ATAP, systemProgram: SystemProgram.programId }).instruction();
  const r = ok([open, uvaIx(who.publicKey, v), add], [who], 'open + volume accumulator + add_mint');
  return { m, v, r };
}
const CU = {};
// An outside trader buys on the same pool with its own wallet (raw PumpSwap buy_exact_quote_in_v2).
function whaleBuy(lams) {
  const w = whale.publicKey;
  const metas = ammMetas(w);
  metas[1] = { pubkey: w, isSigner: true, isWritable: true };
  const wsol = ata(w, WSOL, TOK);
  return [
    spl.createAssociatedTokenAccountIdempotentInstruction(w, wsol, w, WSOL, TOK),
    spl.createAssociatedTokenAccountIdempotentInstruction(w, ata(w, MINT, T22), w, MINT, T22),
    SystemProgram.transfer({ fromPubkey: w, toPubkey: wsol, lamports: lams }),
    spl.createSyncNativeInstruction(wsol, TOK),
    new TransactionInstruction({ programId: AMM, keys: metas, data: Buffer.concat([Buffer.from([194, 171, 28, 70, 104, 77, 91, 47]), le64(lams), le64(1)]) }),
  ];
}

test('setup: fixtures, a pump.fun coin graduated to PumpSwap, mandate config', async () => {
  loadFixtures();
  svm.addProgramFromFile(MP, path.join(TARGET, 'deploy/mandate.so'));
  setTime(now);
  for (const k of [admin, user, agent, agent2, stranger, creator, whale]) svm.airdrop(k.publicKey, 500n * SOL);
  svm.airdrop(buybackVault.publicKey, SOL);
  svm.airdrop(feeTo.publicKey, SOL);
  global = sdk.PUMP_SDK.decodeGlobal(info(pda(['global'], PUMP)));
  ok([await sdk.PUMP_SDK.createV2Instruction({ mint: MINT, name: 'Cadre Test', symbol: 'CDRT', uri: 'https://example.com/c.json', creator: creator.publicKey, user: creator.publicKey, mayhemMode: false })], [creator, mintKp], 'create_v2');
  const BC = sdk.bondingCurvePda(MINT);
  const bc = sdk.PUMP_SDK.decodeBondingCurve(info(BC));
  const big = bc.realTokenReserves.add(new BN('20000000000000'));
  ok(await sdk.PUMP_SDK.buyV3Instructions({ bondingCurve: bc, associatedUserAccountInfo: null, mint: MINT, user: whale.publicKey, amount: big, quoteAmount: new BN(150 * LAMPORTS_PER_SOL), slippage: 0, tokenProgram: T22 }), [whale], 'buy past curve end');
  ok([await sdk.PUMP_SDK.migrateV2Instruction({ withdrawAuthority: global.withdrawAuthority, mint: MINT, user: whale.publicKey, quoteMint: WSOL, baseTokenProgram: T22, quoteTokenProgram: TOK })], [whale], 'migrate_v2');
  POOL = sdk.canonicalPumpPoolPda(MINT);
  PL = off.decodePool(info(POOL));
  gc = off.decodeGlobalConfig(info(pda(['global_config'], AMM)));
  ok([uvaIx(whale.publicKey, whale.publicKey)], [whale], 'whale volume accumulator');
  ok([await mp.methods.initConfig(1_000, buybackVault.publicKey).accountsStrict({ admin: admin.publicKey, config: CONFIG, systemProgram: SystemProgram.programId }).instruction()], [admin], 'init_config');
  fails([await mp.methods.initConfig(1_000, buybackVault.publicKey).accountsStrict({ admin: stranger.publicKey, config: CONFIG, systemProgram: SystemProgram.programId }).instruction()], [stranger], /already in use/, 'second init_config');
  fails([await setConfigIx({ protocolBps: 6_000 })], [admin], /BadConfig/, 'protocol share above 50%');
  fails([await setConfigIx({ swapsPaused: true }, stranger.publicKey)], [stranger], /Unauthorized/, 'stranger kill switch');
  const r = reserves();
  assert.ok(r.quote > 10n * SOL, `pool has SOL depth (${r.quote})`);
});

test('open: the user funds the vault; the mandate records agent, limits, terms', async () => {
  const o = await openMandate(ID, agent.publicKey);
  M = o.m; V = o.v;
  const d = decodeMandate(M);
  assert.equal(d.user.toBase58(), user.publicKey.toBase58());
  assert.equal(d.agent.toBase58(), agent.publicKey.toBase58());
  assert.equal(BigInt(d.deposited.toString()), 3n * SOL);
  assert.equal(lamports(V), VAULT_RENT + 3n * SOL);
  const ev = evOf(o.r, 'opened');
  assert.equal(ev.length, 1);
  // bad limits refused at open
  const m2 = mandatePda(user.publicKey, 99n);
  fails([await mp.methods.open(bn(99), agent.publicKey, feeTo.publicKey, limits({ mints: [] }), terms(), bn(SOL)).accountsStrict({ user: user.publicKey, mandate: m2, vault: vaultPda(m2), systemProgram: SystemProgram.programId }).instruction()], [user], /BadLimits/, 'no mints and not any_mint');
  fails([await mp.methods.open(bn(99), agent.publicKey, feeTo.publicKey, limits(), terms({ perfFeeBps: 9_000 }), bn(SOL)).accountsStrict({ user: user.publicKey, mandate: m2, vault: vaultPda(m2), systemProgram: SystemProgram.programId }).instruction()], [user], /BadTerms/, 'fee above 50%');
});

let bought = 0n;
test('agent buy on PumpSwap: SOL out of the vault at most amount_in, tokens into the vault at least min_out', async () => {
  const solIn = SOL / 4n;
  const min = buyMin(solIn);
  const l0 = lamports(V), t0 = tokenAmount(ata(V, MINT, T22));
  const r = ok([await swapIx(0, solIn, min)], [agent], 'agent buy');
  CU.buy = r.cu; CU.buySize = r.size;
  const l1 = lamports(V), t1 = tokenAmount(ata(V, MINT, T22));
  assert.ok(l0 - l1 <= solIn, `spent ${l0 - l1} <= ${solIn}`);
  assert.ok(l0 - l1 > 0n);
  bought = t1 - t0;
  assert.ok(bought >= min, `got ${bought} >= ${min}`);
  assert.equal(svm.getAccount(ata(V, WSOL, TOK)), null, 'temporary wSOL account closed');
  const ev = evOf(r, 'mandateSwap');
  assert.equal(ev.length, 1);
  assert.equal(ev[0].side, 0);
  assert.equal(BigInt(ev[0].amountIn.toString()), l0 - l1);
  assert.equal(BigInt(ev[0].amountOut.toString()), bought);
  const d = decodeMandate(M);
  assert.equal(Number(d.trades), 1);
  assert.equal(BigInt(d.windowUsed.toString()), solIn);
});

test('agent sell: tokens out at most amount_in, SOL back at least min_out', async () => {
  const tok = bought / 2n;
  const min = sellMin(tok);
  const l0 = lamports(V), t0 = tokenAmount(ata(V, MINT, T22));
  const r = ok([await swapIx(1, tok, min)], [agent], 'agent sell');
  CU.sell = r.cu; CU.sellSize = r.size;
  const l1 = lamports(V), t1 = tokenAmount(ata(V, MINT, T22));
  assert.ok(t0 - t1 <= tok);
  assert.ok(l1 - l0 >= min, `sol back ${l1 - l0} >= ${min}`);
  assert.equal(evOf(r, 'mandateSwap')[0].side, 1);
});

test('exploit: the agent cannot withdraw SOL or tokens, change limits, rotate or revoke', async () => {
  fails([await userOp('withdrawSol', [bn(SOL / 10n)], agent.publicKey)], [agent], /ConstraintSeeds|Unauthorized|has_one|2006|2001/, 'agent withdraw_sol');
  fails([await withdrawTokenIx(1n, agent.publicKey)], [agent], /ConstraintSeeds|Unauthorized|2006|2001/, 'agent withdraw_token');
  fails([await userOnly('setLimits', [limits({ l: { maxInPerTrade: bn(100n * SOL) } })], agent.publicKey)], [agent], /ConstraintSeeds|Unauthorized|2006|2001/, 'agent set_limits');
  fails([await userOnly('setAgent', [agent2.publicKey], agent.publicKey)], [agent], /ConstraintSeeds|Unauthorized|2006|2001/, 'agent set_agent');
  // the agent cannot pass a mandate it does not own as "user" with its own key either: the vault seeds bind to the mandate
  const fakeVault = vaultPda(mandatePda(agent.publicKey, ID));
  fails([await userOp('withdrawSol', [bn(1)], user.publicKey, M, fakeVault)], [user], /ConstraintSeeds|2006/, 'wrong vault for the mandate');
  // a plain System transfer out of the vault cannot be signed by anyone (PDA, no key)
});

test('exploit: the agent cannot route trade output to itself or use another trader account', async () => {
  const solIn = SOL / 10n;
  const min = buyMin(solIn);
  const rem = ammMetas(V);
  // output token account = the agent's own ATA
  const r1 = rem.map((x) => ({ ...x })); r1[5].pubkey = ata(agent.publicKey, MINT, T22);
  fails([await swapIx(0, solIn, min, { rem: r1 })], [agent], /BadAccount/, 'base output to the agent ATA');
  const r2 = rem.map((x) => ({ ...x })); r2[6].pubkey = ata(agent.publicKey, WSOL, TOK);
  fails([await swapIx(0, solIn, min, { rem: r2 })], [agent], /BadAccount/, 'quote account of the agent');
  const r3 = rem.map((x) => ({ ...x })); r3[1].pubkey = agent.publicKey;
  fails([await swapIx(0, solIn, min, { rem: r3 })], [agent], /BadAccount/, 'agent as the PumpSwap user');
  const r4 = rem.map((x) => ({ ...x })); r4[16].pubkey = SystemProgram.programId;
  fails([await swapIx(0, solIn, min, { rem: r4 })], [agent], /BadVenue/, 'fake venue program');
  const r5 = rem.slice(0, 16);
  fails([await swapIx(0, solIn, min, { rem: r5 })], [agent], /BadAccount/, 'short account list');
  const r6 = rem.map((x) => ({ ...x })); r6[3].pubkey = Keypair.generate().publicKey;
  fails([await swapIx(0, solIn, min, { rem: r6 })], [agent], /MintNotAllowed/, 'mint not on the allowlist');
  const r7 = rem.map((x) => ({ ...x })); r7[0].pubkey = Keypair.generate().publicKey;
  fails([await swapIx(0, solIn, min, { rem: r7 })], [agent], /BadPool/, 'non-canonical pool');
  fails([await swapIx(2, solIn, min)], [agent], /BadSide/, 'unknown side');
});

test('exploit: every limit refuses (size, slippage, pool share, window cap, trades per window, thin pool)', async () => {
  fails([await swapIx(0, SOL / 2n + 1n, 1n)], [agent], /TradeTooLarge|SlippageTooWide/, 'above max_in_per_trade');
  fails([await swapIx(0, SOL / 10n, 1n)], [agent], /SlippageTooWide/, 'min_out 1 (no slippage bound)');
  fails([await swapIx(0, SOL / 10n, buyMin(SOL / 10n, 600))], [agent], /SlippageTooWide/, 'min_out 6% below quote with a 5% limit');
  // a min_out above what the pool gives is refused by PumpSwap itself (and our MinOut check)
  const r = reserves();
  const tooMuch = cpOut(r.quote, r.base, SOL / 10n) * 2n;
  fails([await swapIx(0, SOL / 10n, tooMuch)], [agent], /./, 'min_out above the pool');
  // tighten: pool share, window cap, trades per window, thin pool, each alone
  ok([await userOnly('setLimits', [limits({ l: { maxTradeShareBps: 1 } })])], [user], 'share limit 0.01%');
  fails([await swapIx(0, SOL / 10n, buyMin(SOL / 10n))], [agent], /TradeTooLarge/, 'above max_trade_share_bps');
  ok([await userOnly('setLimits', [limits({ l: { minPoolQuoteReserve: bn(1_000_000n * SOL) } })])], [user], 'min pool depth huge');
  fails([await swapIx(0, SOL / 10n, buyMin(SOL / 10n))], [agent], /ThinPool/, 'pool too thin');
  ok([await userOnly('setLimits', [limits({ l: { windowCap: bn(SOL / 2n) } })])], [user], 'window cap 0.5 SOL');
  // used so far this window: 0.25 buy + about 0.125 sell value; one more 0.2 passes the 0.5 cap
  const d = decodeMandate(M);
  const left = SOL / 2n - BigInt(d.windowUsed.toString());
  assert.ok(left > 0n && left < SOL / 2n);
  fails([await swapIx(0, left + 1_000_000n, buyMin(left + 1_000_000n))], [agent], /WindowCap/, 'over the window cap');
  ok([await userOnly('setLimits', [limits({ l: { maxTradesPerWindow: 2 } })])], [user], 'two trades per window');
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n))], [agent], /TooManyTrades/, 'third trade in the window');
  // the window rolls after window_secs
  setTime(now + 86_401n);
  ok([await swapIx(0, SOL / 100n, buyMin(SOL / 100n))], [agent], 'first trade of a new window');
  ok([await userOnly('setLimits', [limits()])], [user], 'limits back');
});

test('exploit: a stranger cannot swap; pause, expiry and the global kill switch stop the agent', async () => {
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n), { signer: stranger.publicKey })], [stranger], /NotAgent/, 'stranger swap');
  ok([await userOnly('setLimits', [limits({ l: { paused: true } })])], [user], 'user pauses');
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n))], [agent], /Paused/, 'swap while paused');
  ok([await userOp('withdrawSol', [bn(SOL / 100n)])], [user], 'withdraw while paused');
  ok([await userOnly('setLimits', [limits({ l: { expiryTs: bn(now - 1n) } })])], [user], 'expired');
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n))], [agent], /Expired/, 'swap after expiry');
  ok([await withdrawTokenIx(1_000n)], [user], 'withdraw tokens after expiry');
  ok([await userOnly('setLimits', [limits()])], [user], 'limits back');
  ok([await setConfigIx({ swapsPaused: true })], [admin], 'kill switch on');
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n))], [agent], /SwapsPaused/, 'swap under the kill switch');
  ok([await userOp('withdrawSol', [bn(SOL / 100n)])], [user], 'withdraw under the kill switch');
  ok([await userOp('deposit', [bn(SOL / 100n)])], [user], 'deposit under the kill switch');
  ok([await setConfigIx({ swapsPaused: false })], [admin], 'kill switch off');
});

test('exploit: revoke kills the agent at once; set_agent rotates (old key dead, new key works)', async () => {
  ok([await userOnly('setAgent', [agent2.publicKey])], [user], 'rotate to agent2');
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n))], [agent], /NotAgent/, 'old agent after rotation');
  ok([await swapIx(0, SOL / 100n, buyMin(SOL / 100n), { signer: agent2.publicKey })], [agent2], 'new agent trades');
  ok([await userOnly('revoke', [])], [user], 'revoke');
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n), { signer: agent2.publicKey })], [agent2], /Revoked/, 'agent2 after revoke');
  fails([await swapIx(0, SOL / 100n, buyMin(SOL / 100n))], [agent], /Revoked/, 'agent after revoke');
  ok([await userOnly('setAgent', [agent.publicKey])], [user], 'user re-grants agent');
});

test('fees: performance fee only on realized SOL profit above the high-water mark; 10% of it to the buyback vault', async () => {
  // fresh mandate so the arithmetic is exact
  const id = 8n;
  const { m, v } = await openMandate(id, agent.publicKey, limits({ l: { maxInPerTrade: bn(5n * SOL), windowCap: bn(10n * SOL) } }), terms({ perfFeeBps: 2_000, settleIntervalSecs: 3_600 }), 2n * SOL);
  // no profit yet: settle charges nothing
  let r = ok([await settleIx(m, v)], [stranger], 'settle at zero pnl (permissionless)');
  let s = evOf(r, 'settled')[0];
  assert.equal(BigInt(s.feeAgent.toString()) + BigInt(s.feeProtocol.toString()), 0n);
  fails([await settleIx(m, v)], [stranger], /TooSoon/, 'settle again inside the interval');
  // buy, then a whale pumps the pool, then the agent sells everything: realized profit in SOL
  const solIn = SOL;
  ok([await swapIx(0, solIn, buyMin(solIn), { mandate: m, vault: v })], [agent], 'buy 1 SOL');
  // unrealized gains are not charged: settle after the pump but before the sell charges 0
  ok(whaleBuy(20n * SOL), [whale], 'whale buys 20 SOL');
  setTime(now + 3_601n);
  r = ok([await settleIx(m, v)], [stranger], 'settle with only an unrealized gain');
  s = evOf(r, 'settled')[0];
  assert.equal(BigInt(s.feeAgent.toString()) + BigInt(s.feeProtocol.toString()), 0n, 'token marks are never charged');
  const tok = tokenAmount(ata(v, MINT, T22));
  ok([await swapIx(1, tok, sellMin(tok), { mandate: m, vault: v })], [agent], 'sell everything');
  const d = decodeMandate(m);
  const equity = lamports(v) - VAULT_RENT;
  const pnl = equity + BigInt(d.withdrawn.toString()) + BigInt(d.feesAgent.toString()) + BigInt(d.feesProtocol.toString()) - BigInt(d.deposited.toString());
  assert.ok(pnl > 0n, `realized profit ${pnl}`);
  const fee = (pnl * 2_000n) / 10_000n, proto = (fee * 1_000n) / 10_000n;
  const bb0 = lamports(buybackVault.publicKey), ft0 = lamports(feeTo.publicKey);
  setTime(now + 3_601n);
  r = ok([await settleIx(m, v)], [stranger], 'settle realized profit');
  s = evOf(r, 'settled')[0];
  assert.equal(BigInt(s.feeProtocol.toString()), proto);
  assert.equal(BigInt(s.feeAgent.toString()), fee - proto);
  assert.equal(lamports(buybackVault.publicKey) - bb0, proto, 'protocol share (1,000 bps of the fee) reached the buyback vault');
  assert.equal(lamports(feeTo.publicKey) - ft0, fee - proto, 'agent fee reached fee_to');
  assert.equal(BigInt(decodeMandate(m).hwm.toString()), pnl);
  // nothing new above the mark: the next settle charges 0
  setTime(now + 3_601n);
  r = ok([await settleIx(m, v)], [stranger], 'settle at the mark');
  s = evOf(r, 'settled')[0];
  assert.equal(BigInt(s.feeAgent.toString()) + BigInt(s.feeProtocol.toString()), 0n);
  // deposits and withdrawals are not profit
  ok([await userOp('deposit', [bn(SOL)], user.publicKey, m, v)], [user], 'deposit 1 SOL');
  ok([await userOp('withdrawSol', [bn(SOL / 2n)], user.publicKey, m, v)], [user], 'withdraw 0.5 SOL');
  setTime(now + 3_601n);
  r = ok([await settleIx(m, v)], [stranger], 'settle after deposit and withdrawal');
  s = evOf(r, 'settled')[0];
  assert.equal(BigInt(s.feeAgent.toString()) + BigInt(s.feeProtocol.toString()), 0n, 'flows are not profit');
  // wrong recipients refused
  setTime(now + 3_601n);
  fails([await settleIx(m, v, stranger.publicKey)], [stranger], /BadAccount/, 'fee to someone else');
  fails([await settleIx(m, v, feeTo.publicKey, stranger.publicKey)], [stranger], /BadAccount/, 'protocol share to someone else');
  // the user withdraws everything at the end, always possible
  const eq = lamports(v) - VAULT_RENT;
  ok([await userOp('withdrawSol', [bn(eq)], user.publicKey, m, v)], [user], 'withdraw all SOL');
  assert.equal(lamports(v), VAULT_RENT);
  fails([await userOp('withdrawSol', [bn(1)], user.publicKey, m, v)], [user], /Insufficient/, 'nothing left above rent');
});

test('withdraw everything from the first mandate, then report CU and tx size', async () => {
  ok([await userOnly('revoke', [])], [user], 'revoke');
  ok([await setConfigIx({ swapsPaused: true })], [admin], 'kill switch on');
  const t = tokenAmount(ata(V, MINT, T22));
  ok([await withdrawTokenIx(t)], [user], 'withdraw all tokens (revoked + kill switch)');
  assert.equal(tokenAmount(ata(V, MINT, T22)), 0n);
  ok([await userOp('withdrawSol', [bn(lamports(V) - VAULT_RENT)])], [user], 'withdraw all SOL (revoked + kill switch)');
  ok([await setConfigIx({ swapsPaused: false })], [admin], 'kill switch off');
  console.log(`  CU: buy ${CU.buy} (tx ${CU.buySize} B), sell ${CU.sell} (tx ${CU.sellSize} B)`);
  assert.ok(CU.buy < 400_000 && CU.sell < 400_000);
});
