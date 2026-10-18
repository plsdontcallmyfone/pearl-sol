// pearl-sol crank (SPEC 2.2, 4). Usage:
//
//   node src/main.ts status
//   node src/main.ts init   [--maxBuyPerCall L] [--minIntervalSecs S] [--maxSlippageBps B] ...   (admin = KEYPAIR)
//   node src/main.ts update [--cranker PK] [--funder PK] [--bridgeProgram PK] [--miningPool PK] ...
//   node src/main.ts launch --name N --symbol S --uri U --devBuy LAMPORTS [--mintKeypair FILE]
//   node src/main.ts once   one buyback attempt (+ fund_mining if FUND_AMOUNT is set)
//   node src/main.ts run    loop every CRANK_INTERVAL_SECS
//   node src/main.ts fund --amount BASE_UNITS
//
// Storefront buyback (token/programs/storefront, SPEC 5.1 "Buyback"):
//   node src/main.ts sf-status                   buyback config, vault, totals, pending orders (JSON)
//   node src/main.ts sf-setup                    vault PDA's pump + PumpSwap volume accumulators (once, anyone pays)
//   node src/main.ts sf-set [--bps N] [--dest reserve|burn] [--cranker PK|none] [--maxPerCall L] [--minBuy L]
//                           [--minIntervalSecs S] [--reserveProgram PK]       (admin = KEYPAIR)
//   node src/main.ts sf-buyback                  one crank_buyback(_amm) attempt, prints the BuybackExecuted fields (JSON)
//   `run` and `once` also attempt a storefront buyback each tick (SF_BUYBACK=0 turns it off).
//
// Env (all optional):
//   RPC_URL             default http://127.0.0.1:10600 (token localnet)
//   KEYPAIR             signer, default keys/localnet/payer-keypair.json
//   RESERVE_IDL         default token/target/idl/reserve.json
//   CRANK_INTERVAL_SECS loop period, default 60 (the on-chain min interval still applies)
//   MAX_SOL_IN          lamports per crank sent as max_sol_in, default 1e9
//   MIN_OUT_BPS         min_tokens_out = fee-free quote x (1 - bps), default 300
//   FUND_AMOUNT         base units per fund_mining call, default 0 (off)
//   FUND_INTERVAL_SECS  default 3600
//   STOREFRONT_IDL      default token/target/idl/storefront.json
//   SF_BUYBACK          1 (default) = storefront buyback in run/once, 0 = off
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import type { TransactionInstruction, PublicKey as PublicKeyT, Keypair as KeypairT } from '@solana/web3.js';
import { ReserveKeys, bondingCurve, canonicalPool, pda, ata, PUMP, AMM, WSOL, TOK } from './accounts.ts';
import { StorefrontBuyback } from './storefront.ts';

// CommonJS builds throughout: the pump SDKs' ESM builds do not load next to
// anchor's CJS build, and one web3.js copy keeps PublicKey checks working.
const require = createRequire(import.meta.url);
const { Connection, Keypair, PublicKey, Transaction, ComputeBudgetProgram, SYSVAR_CLOCK_PUBKEY } = require('@solana/web3.js') as typeof import('@solana/web3.js');
type PublicKey = PublicKeyT;
type Keypair = KeypairT;
const anchor = require('@coral-xyz/anchor');
const BN = require('bn.js');
const pumpSdk = require('@pump-fun/pump-sdk');
const { PUMP_SDK } = pumpSdk;
const amm = new (require('@pump-fun/pump-swap-sdk').PumpAmmSdk)();

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const env = (k: string, d: string) => process.env[k] ?? d;
const RPC = env('RPC_URL', 'http://127.0.0.1:10600');
const KEYPAIR = env('KEYPAIR', path.join(ROOT, 'keys/localnet/payer-keypair.json'));
const IDL = env('RESERVE_IDL', path.join(ROOT, 'token/target/idl/reserve.json'));
const INTERVAL = Number(env('CRANK_INTERVAL_SECS', '60'));
const MAX_SOL_IN = BigInt(env('MAX_SOL_IN', '1000000000'));
const MIN_OUT_BPS = BigInt(env('MIN_OUT_BPS', '300'));
const FUND_AMOUNT = BigInt(env('FUND_AMOUNT', '0'));
const FUND_INTERVAL = Number(env('FUND_INTERVAL_SECS', '3600'));
const SF_IDL = env('STOREFRONT_IDL', path.join(ROOT, 'token/target/idl/storefront.json'));
const SF_BUYBACK = env('SF_BUYBACK', '1') !== '0';

const loadKp = (f: string) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(f, 'utf8'))));
const signer = loadKp(KEYPAIR);
const conn = new Connection(RPC, 'confirmed');
const idl = JSON.parse(fs.readFileSync(IDL, 'utf8'));
const program = new anchor.Program(idl, new anchor.AnchorProvider(conn, new anchor.Wallet(signer), { commitment: 'confirmed' }));
const keys = new ReserveKeys(new PublicKey(idl.address));
const log = (...a: unknown[]) => console.log(new Date().toISOString(), ...a);
let sfb: StorefrontBuyback | null = null;
const storefront = () => (sfb ??= new StorefrontBuyback(conn, signer, SF_IDL, (ixs, extra) => send(ixs, extra), MIN_OUT_BPS, MAX_SOL_IN));

const ZERO = PublicKey.default;
const cpQuote = (base: bigint, quote: bigint, inAmt: bigint) => (quote + inAmt === 0n ? 0n : (base * inAmt) / (quote + inAmt));
const min = (...v: bigint[]) => v.reduce((a, b) => (b < a ? b : a));

async function send(ixs: TransactionInstruction[], extra: Keypair[] = []) {
  const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 600_000 }), ...ixs);
  tx.feePayer = signer.publicKey;
  tx.recentBlockhash = (await conn.getLatestBlockhash()).blockhash;
  tx.sign(signer, ...extra);
  const sig = await conn.sendRawTransaction(tx.serialize());
  const r = await conn.confirmTransaction(sig, 'confirmed');
  if (r.value.err) throw new Error(`tx ${sig} failed: ${JSON.stringify(r.value.err)}`);
  return sig;
}

async function fetchConfig() {
  const a = await conn.getAccountInfo(keys.config);
  return a ? program.coder.accounts.decode('config', a.data) : null;
}
async function chainTime() {
  const a = await conn.getAccountInfo(SYSVAR_CLOCK_PUBKEY);
  return Number(a!.data.readBigInt64LE(32));
}
async function tokenAmount(k: PublicKey) {
  const a = await conn.getAccountInfo(k);
  return a && a.data.length >= 72 ? a.data.readBigUInt64LE(64) : 0n;
}
async function pumpGlobal() {
  return PUMP_SDK.decodeGlobal(await conn.getAccountInfo(pda(['global'], PUMP)));
}

function parseFlags(argv: string[]) {
  const o: Record<string, string> = {};
  for (let i = 0; i < argv.length; i++) if (argv[i].startsWith('--')) o[argv[i].slice(2)] = argv[++i];
  return o;
}
const PK_FIELDS = ['mint', 'cranker', 'funder', 'bridgeProgram', 'miningPool'];
const NUM_FIELDS = ['maxBuyPerCall', 'minBuy', 'minIntervalSecs', 'solFloor', 'maxFundPerCall'];
function configParams(f: Record<string, string>) {
  const p: Record<string, unknown> = {};
  for (const k of PK_FIELDS) p[k] = f[k] ? new PublicKey(f[k]) : null;
  for (const k of NUM_FIELDS) p[k] = f[k] ? new BN(f[k]) : null;
  p.maxSlippageBps = f.maxSlippageBps ? Number(f.maxSlippageBps) : null;
  return p;
}

async function status() {
  const c = await fetchConfig();
  if (!c) return log('reserve config not initialized', keys.config.toBase58());
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(c)) out[k] = (v as any)?.toBase58?.() ?? String(v);
  out.creatorPda = keys.creator.toBase58();
  out.creatorLamports = String(await conn.getBalance(keys.creator));
  if (!c.mint.equals(ZERO)) out.reserveTokens = String(await tokenAmount(keys.reserveAta(c.mint)));
  console.log(JSON.stringify(out, null, 2));
}

async function launch(f: Record<string, string>) {
  const mintKp = f.mintKeypair ? loadKp(f.mintKeypair) : Keypair.generate();
  const g = await pumpGlobal();
  const devBuy = new BN(f.devBuy ?? '0');
  const feeConfig = PUMP_SDK.decodeFeeConfig(await conn.getAccountInfo(pda(['fee_config', PUMP], new PublicKey('pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ'))));
  const expected = devBuy.isZero() ? new BN(0) : (pumpSdk as any).getBuyTokenAmountFromSolAmount({ global: g, feeConfig, mintSupply: null, bondingCurve: null, amount: devBuy, quoteMint: WSOL });
  const minOut = expected.muln(98).divn(100);
  const ix = await program.methods
    .launch(f.name, f.symbol, f.uri, devBuy, minOut)
    .accountsStrict(keys.launch(signer.publicKey, mintKp.publicKey, g.buybackFeeRecipients[0]))
    .instruction();
  const sig = await send([ix], [mintKp]);
  log('launched', mintKp.publicKey.toBase58(), 'dev buy', devBuy.toString(), 'reserve tokens', String(await tokenAmount(keys.reserveAta(mintKp.publicKey))), sig);
}

async function crankOnce(): Promise<string> {
  const c = await fetchConfig();
  if (!c) return 'not initialized';
  if (c.mint.equals(ZERO)) return 'not launched';
  const now = await chainTime();
  const last = Number(c.lastCrankTs);
  if (last !== 0 && now < last + Number(c.minIntervalSecs)) return `wait ${last + Number(c.minIntervalSecs) - now}s`;
  const mint: PublicKey = c.mint;
  const floor = BigInt(c.solFloor.toString()), cap = BigInt(c.maxBuyPerCall.toString()), minBuy = BigInt(c.minBuy.toString());
  const curve = PUMP_SDK.decodeBondingCurve(await conn.getAccountInfo(bondingCurve(mint)));
  const native = BigInt(await conn.getBalance(keys.creator));
  const curveFee = BigInt(curve.creatorFee?.toString() ?? '0');
  const before = await tokenAmount(keys.reserveAta(mint));
  const g = await pumpGlobal();

  if (!curve.complete) {
    // lower bound of what the crank will spend: native above floor + fees waiting on the curve
    const spend = min(native + curveFee > floor ? native + curveFee - floor : 0n, cap, MAX_SOL_IN);
    if (spend < minBuy) return `curve: only ${spend} lamports to spend (min_buy ${minBuy})`;
    const q = cpQuote(BigInt(curve.virtualTokenReserves.toString()), BigInt(curve.virtualQuoteReserves?.toString() ?? curve.virtualSolReserves.toString()), spend);
    const minOut = (q * (10_000n - MIN_OUT_BPS)) / 10_000n;
    const ix = await program.methods
      .crankBuyback(new BN(MAX_SOL_IN.toString()), new BN(minOut.toString()))
      .accountsStrict(keys.curve(signer.publicKey, mint, g.buybackFeeRecipients[Math.floor(Math.random() * 8)]))
      .instruction();
    const sig = await send([ix]);
    return `curve buyback +${(await tokenAmount(keys.reserveAta(mint))) - before} tokens ${sig}`;
  }

  // graduated: first pull any old curve fees as lamports, then buy on PumpSwap
  const curveKeys = keys.curve(signer.publicKey, mint, g.buybackFeeRecipients[0]);
  const cvLamports = BigInt(await conn.getBalance(curveKeys.creatorVault));
  if (curveFee > 0n || cvLamports > 890_880n) {
    await send([await program.methods.collectCurveFees().accountsStrict(curveKeys).instruction()]);
  }
  const poolKey = canonicalPool(mint);
  const pool = amm.decodePool(await conn.getAccountInfo(poolKey));
  const gc = amm.decodeGlobalConfig(await conn.getAccountInfo(pda(['global_config'], AMM)));
  const wsolHave = await tokenAmount(ata(keys.creator, WSOL, TOK));
  const native2 = BigInt(await conn.getBalance(keys.creator));
  const poolCreatorFees = pool.coinCreator.equals(keys.creator) ? BigInt(pool.creatorFees.toString()) : 0n;
  const spend = min(wsolHave + (native2 > floor ? native2 - floor : 0n) + poolCreatorFees, cap, MAX_SOL_IN);
  if (spend < minBuy) return `pool: only ${spend} lamports to spend (min_buy ${minBuy})`;
  const baseRes = await tokenAmount(pool.poolBaseTokenAccount);
  const vq = BigInt(pool.virtualQuoteReserves?.toString() ?? '0');
  const quoteRes = (await tokenAmount(pool.poolQuoteTokenAccount)) - BigInt(pool.protocolFees?.toString() ?? '0') - BigInt(pool.creatorFees?.toString() ?? '0') + (vq > 0n ? vq : 0n);
  const q = cpQuote(baseRes, quoteRes, spend);
  const minOut = (q * (10_000n - MIN_OUT_BPS)) / 10_000n;
  const ix = await program.methods
    .crankBuybackAmm(new BN(MAX_SOL_IN.toString()), new BN(minOut.toString()))
    .accountsStrict(keys.amm(signer.publicKey, mint, pool.poolBaseTokenAccount, pool.poolQuoteTokenAccount, gc.buybackFeeRecipients[0]))
    .instruction();
  const sig = await send([ix]);
  return `pool buyback +${(await tokenAmount(keys.reserveAta(mint))) - before} tokens ${sig}`;
}

async function fund(amount: bigint) {
  const c = await fetchConfig();
  if (!c || c.mint.equals(ZERO) || c.bridgeProgram.equals(ZERO)) return 'fund: reserve not launched or bridge not wired';
  const bcfg = await conn.getAccountInfo(pda(['config'], c.bridgeProgram));
  if (!bcfg) return 'fund: bridge config missing';
  const nonce = bcfg.data.readBigUInt64LE(8 + 32 * 4); // bridge Config: admin, mint, vault, token_program, deposit_nonce
  const ix = await program.methods.fundMining(new BN(amount.toString())).accountsStrict(keys.fund(signer.publicKey, c.mint, c.bridgeProgram, nonce)).instruction();
  const sig = await send([ix]);
  return `funded ${amount} (deposit nonce ${nonce}) ${sig}`;
}

let lastFund = 0;
async function tick() {
  try {
    log(await crankOnce());
  } catch (e) {
    log('crank error:', String((e as Error).message ?? e).slice(0, 400));
  }
  if (SF_BUYBACK) {
    try {
      log('storefront buyback', JSON.stringify(await storefront().once(await chainTime())));
    } catch (e) {
      log('storefront buyback error:', String((e as Error).message ?? e).slice(0, 400));
    }
  }
  if (FUND_AMOUNT > 0n && Date.now() / 1000 - lastFund >= FUND_INTERVAL) {
    try {
      log(await fund(FUND_AMOUNT));
      lastFund = Date.now() / 1000;
    } catch (e) {
      log('fund error:', String((e as Error).message ?? e).slice(0, 400));
    }
  }
}

const [cmd = 'status', ...rest] = process.argv.slice(2);
const flags = parseFlags(rest);
switch (cmd) {
  case 'status':
    await status();
    break;
  case 'init': {
    const ix = await program.methods.initialize(configParams(flags)).accountsStrict({ admin: signer.publicKey, config: keys.config, creator: keys.creator, systemProgram: new PublicKey('11111111111111111111111111111111') }).instruction();
    log('initialized', await send([ix]));
    break;
  }
  case 'update': {
    const ix = await program.methods.updateConfig(configParams(flags)).accountsStrict({ admin: signer.publicKey, config: keys.config }).instruction();
    log('updated', await send([ix]));
    break;
  }
  case 'launch':
    await launch(flags);
    break;
  case 'fund':
    log(await fund(BigInt(flags.amount)));
    break;
  case 'sf-status':
    console.log(JSON.stringify(await storefront().status(), null, 2));
    break;
  case 'sf-setup':
    log('storefront vault volume accumulators', JSON.stringify(await storefront().setup()));
    break;
  case 'sf-set':
    log('set_buyback', await storefront().set(flags));
    break;
  case 'sf-buyback':
    console.log(JSON.stringify(await storefront().once(await chainTime())));
    break;
  case 'once':
    await tick();
    break;
  case 'run':
    log(`crank on ${RPC.replace(/api-key=[^&]+/, 'api-key=REDACTED')} as ${signer.publicKey.toBase58()}, every ${INTERVAL}s`);
    for (;;) {
      await tick();
      await new Promise((r) => setTimeout(r, INTERVAL * 1000));
    }
  default:
    console.error('unknown command', cmd);
    process.exit(2);
}
