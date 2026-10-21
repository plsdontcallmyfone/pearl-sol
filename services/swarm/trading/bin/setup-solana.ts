// Solana devnet setup for the mandate program (docs/TRADING.md "Devnet"), idempotent:
//   node bin/setup-solana.ts --keys <dir> --out <file> [--deposit-sol 0.4]
// 1. init_config: admin = trading ops (holds the swap kill switch), protocol 1,000 bps (D7),
//    buyback vault = the storefront's ["buyback"] PDA on devnet;
// 2. trading ops (as the user) opens one mandate per house trader (trader-1..3) for CADRE on its
//    canonical PumpSwap pool, with small limits; the vault's PumpSwap volume accumulator and its
//    CADRE token account are created in the same tx.
// Prints addresses and signatures only.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { Connection, Keypair, PublicKey, Transaction, ComputeBudgetProgram, sendAndConfirmTransaction } from "@solana/web3.js";
import { MandateProgram, poolInfo, readReserves, type Limits } from "../src/solmandate.ts";

function arg(name: string, def?: string): string {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1]) return process.argv[i + 1];
  if (def !== undefined) return def;
  throw new Error(`--${name} is required`);
}
const kp = (f: string) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(f, "utf8"))));

const RPC = process.env.SOLANA_RPC ?? "https://api.devnet.solana.com";
const PROGRAM = new PublicKey(process.env.MANDATE_PROGRAM ?? "BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc");
const MINT = new PublicKey(process.env.CADRE_MINT ?? "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e");
const BUYBACK = new PublicKey(process.env.BUYBACK_VAULT ?? "9D3hqn9D2TsT4pS7p7J5sAHNB9QBHVJnPSKX8d7kJbQ6");

async function main() {
  const dir = arg("keys");
  const outFile = arg("out");
  const deposit = BigInt(Math.round(Number(arg("deposit-sol", "0.4")) * 1e9));
  const out: any = existsSync(outFile) ? JSON.parse(readFileSync(outFile, "utf8")) : {};
  const save = () => writeFileSync(outFile, JSON.stringify(out, null, 2) + "\n");
  const conn = new Connection(RPC, "confirmed");
  const ops = kp(join(dir, "ops.json"));
  const mp = new MandateProgram(PROGRAM);
  const send = async (label: string, ixs: any[], signers: Keypair[]) => {
    const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...ixs);
    const sig = await sendAndConfirmTransaction(conn, tx, signers, { commitment: "confirmed" });
    console.log(JSON.stringify({ step: label, sig }));
    return sig;
  };
  out.solana = { ...(out.solana ?? {}), program: PROGRAM.toBase58(), config: mp.config.toBase58(), mint: MINT.toBase58(), buyback_vault: BUYBACK.toBase58(), user: ops.publicKey.toBase58() };
  if (!(await conn.getAccountInfo(mp.config))) {
    out.solana.init_config_sig = await send("init_config", [mp.initConfig(ops.publicKey, 1_000, BUYBACK)], [ops]);
  }
  const pool = await poolInfo(conn, MINT);
  const r = await readReserves(conn, pool);
  console.log(JSON.stringify({ pool: pool.pool.toBase58(), reserves: { cadre_base_units: r.base.toString(), sol_lamports: r.quote.toString() } }));
  out.solana.pool = pool.pool.toBase58();
  // proposed devnet limits: small, and below 1% of the pool's SOL side per trade
  const limits: Limits = {
    venues: 1 << 1, anyMint: false, mints: [MINT],
    maxInPerTrade: 20_000_000n, // 0.02 SOL
    windowCap: 300_000_000n, // 0.3 SOL of trades per day
    windowSecs: 86_400, maxTradesPerWindow: 48, maxSlippageBps: 300,
    minPoolQuoteReserve: 1_000_000_000n, maxTradeShareBps: 100, expiryTs: 0n, paused: false,
  };
  out.solana.mandates = out.solana.mandates ?? {};
  for (const [i, f] of ["trader-1.json", "trader-2.json", "trader-3.json"].entries()) {
    const agent = kp(join(dir, f)).publicKey;
    const id = BigInt(i + 1);
    const m = mp.mandate(ops.publicKey, id);
    out.solana.mandates[agent.toBase58()] = { id: Number(id), mandate: m.toBase58(), vault: mp.vault(m).toBase58() };
    if (await conn.getAccountInfo(m)) continue;
    out.solana.mandates[agent.toBase58()].open_sig = await send(`open mandate ${id} for ${agent.toBase58()}`, [
      mp.open(ops.publicKey, id, agent, agent, limits, { perfFeeBps: 2_000, settleIntervalSecs: 3_600 }, deposit),
      mp.initVolumeAccumulator(ops.publicKey, m),
      mp.addMint(ops.publicKey, m, MINT),
    ], [ops]);
    save();
  }
  save();
  console.log(JSON.stringify({ done: true, solana: out.solana }, null, 2));
}
main().catch((e) => { console.error(JSON.stringify({ error: (e as Error).message, logs: (e as any).logs?.slice?.(-8) })); process.exit(1); });
