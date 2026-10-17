// Small pump.fun curve trader (localnet volume for the demo, scripts/demo.sh). Usage:
//
//   node src/trade.ts buy  --keypair FILE --mint MINT --sol LAMPORTS
//   node src/trade.ts sell --keypair FILE --mint MINT (--tokens BASE_UNITS | --fraction 0.5)
//
// Env: RPC_URL (default http://127.0.0.1:10600). Prints one JSON line:
// {"ok":true,"side":"buy","tokens":"...","sig":"...","creatorFee":"..."}.
// Curve only (buy_v3 / sell_v3 via @pump-fun/pump-sdk); refuses after graduation.
import fs from 'node:fs';
import { createRequire } from 'node:module';
import type { Keypair as KeypairT, TransactionInstruction } from '@solana/web3.js';
import { bondingCurve, ata, PUMP, FEES, WSOL, T22, pda } from './accounts.ts';

const require = createRequire(import.meta.url);
const { Connection, Keypair, PublicKey, Transaction, ComputeBudgetProgram } = require('@solana/web3.js') as typeof import('@solana/web3.js');
const BN = require('bn.js');
const sdk = require('@pump-fun/pump-sdk');

const RPC = process.env.RPC_URL ?? 'http://127.0.0.1:10600';
const conn = new Connection(RPC, 'confirmed');

function flags(argv: string[]) {
  const o: Record<string, string> = {};
  for (let i = 0; i < argv.length; i++) if (argv[i].startsWith('--')) o[argv[i].slice(2)] = argv[++i];
  return o;
}

async function send(ixs: TransactionInstruction[], signer: KeypairT) {
  const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...ixs);
  tx.feePayer = signer.publicKey;
  tx.recentBlockhash = (await conn.getLatestBlockhash()).blockhash;
  tx.sign(signer);
  const sig = await conn.sendRawTransaction(tx.serialize());
  const r = await conn.confirmTransaction(sig, 'confirmed');
  if (r.value.err) throw new Error(`tx ${sig} failed: ${JSON.stringify(r.value.err)}`);
  return sig;
}

async function tokenBalance(owner: InstanceType<typeof PublicKey>, mint: InstanceType<typeof PublicKey>) {
  const a = await conn.getAccountInfo(ata(owner, mint, T22));
  return a && a.data.length >= 72 ? a.data.readBigUInt64LE(64) : 0n;
}

async function main() {
  const [side, ...rest] = process.argv.slice(2);
  const f = flags(rest);
  const kp = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(f.keypair, 'utf8'))));
  const mint = new PublicKey(f.mint);
  const bcInfo = await conn.getAccountInfo(bondingCurve(mint));
  if (!bcInfo) throw new Error('no bonding curve for that mint');
  const bc = sdk.PUMP_SDK.decodeBondingCurve(bcInfo);
  if (bc.complete) throw new Error('curve complete (graduated): this trader only trades the curve');
  const before = await tokenBalance(kp.publicKey, mint);
  let sig: string;
  if (side === 'buy') {
    const sol = new BN(f.sol);
    const global = sdk.PUMP_SDK.decodeGlobal(await conn.getAccountInfo(pda(['global'], PUMP)));
    const feeConfig = sdk.PUMP_SDK.decodeFeeConfig(await conn.getAccountInfo(pda(['fee_config', PUMP], FEES)));
    const tokens = sdk.getBuyTokenAmountFromSolAmount({ global, feeConfig, mintSupply: null, bondingCurve: bc, amount: sol, quoteMint: WSOL });
    const userAta = await conn.getAccountInfo(ata(kp.publicKey, mint, T22));
    const ixs = await sdk.PUMP_SDK.buyV3Instructions({ bondingCurve: bc, associatedUserAccountInfo: userAta, mint, user: kp.publicKey,
      amount: tokens, quoteAmount: sol.muln(3).divn(2), slippage: 50, tokenProgram: T22 });
    sig = await send(ixs, kp);
  } else if (side === 'sell') {
    const amount = f.tokens ? new BN(f.tokens) : new BN(((before * BigInt(Math.round(Number(f.fraction ?? '0.5') * 1e6))) / 1_000_000n).toString());
    if (amount.isZero()) throw new Error('nothing to sell');
    const ixs = await sdk.PUMP_SDK.sellV3Instructions({ bondingCurve: bc, mint, user: kp.publicKey, amount, quoteAmount: new BN(0), slippage: 100, tokenProgram: T22 });
    sig = await send(ixs, kp);
  } else {
    throw new Error('usage: trade.ts buy|sell --keypair FILE --mint MINT (--sol L | --tokens N | --fraction F)');
  }
  const after = await tokenBalance(kp.publicKey, mint);
  const bc2 = sdk.PUMP_SDK.decodeBondingCurve(await conn.getAccountInfo(bondingCurve(mint)));
  console.log(JSON.stringify({ ok: true, side, tokens: (after - before).toString(), balance: after.toString(), sig,
    creatorFee: String(bc2.creatorFee?.toString() ?? '0') }));
}

main().catch((e) => {
  console.log(JSON.stringify({ ok: false, error: String((e as Error).message ?? e).slice(0, 500) }));
  process.exit(1);
});
