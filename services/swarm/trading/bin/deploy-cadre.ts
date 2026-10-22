// Cadre devnet setup for agent trading (docs/TRADING.md "Devnet"), idempotent, keys passed by path:
//   node bin/deploy-cadre.ts fund   --funder <key> --keys <dir>          CADRE to the trading ops key and the traders
//   node bin/deploy-cadre.ts deploy --keys <dir> --wasm <dir> --out <file>
//     1. trading ops deploys the AMM and mandate code (skipped if the code hash exists)
//     2. trader-1 (an agent) launches its own token (the SDK token code already on chain) and a CADRE pool,
//        seeding the liquidity from its own wallet
//     3. trading ops instantiates the Cadre mandate contract (protocol 1,000 bps)
//     4. trading ops (as a user) opens a Cadre mandate for trader-2
// Writes the addresses to --out (JSON). Prints addresses only, never keys.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { loadWallet } from "../../src/wallet.ts";
import { Abi, Cadre, codeHash, contractAddress, errText } from "../src/cadre.ts";

const CADRE = 1_000_000_000n;
const TOKEN_CODE = "a57941f4f844786a94380f714529e293712528fc40a27826365c5e90f8bd93ff"; // chain/contracts/testdata/token.wasm, on devnet since 4706

function arg(name: string, def?: string): string {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1]) return process.argv[i + 1];
  if (def !== undefined) return def;
  throw new Error(`--${name} is required`);
}

const rpcUrl = process.env.CADRE_RPC ?? "https://rpc.64-95-12-116.sslip.io";
const c = new Cadre(rpcUrl);

async function landed(what: string, s: { txHash: string }) {
  const r = await c.waitIncluded(s.txHash, 900_000, 2_000);
  const ok = r.receipt ? r.receipt.ok : true;
  console.log(JSON.stringify({ step: what, tx: s.txHash, height: r.height, ok, error: r.receipt && !r.receipt.ok ? errText(r.receipt.error) || r.receipt.error : undefined }));
  if (!ok) throw new Error(`${what} failed at ${r.height}`);
  return r;
}

async function fund() {
  const funder = loadWallet(arg("funder"));
  const dir = arg("keys");
  await c.init();
  const plan: [string, bigint][] = [["ops.json", 60_000n * CADRE], ["trader-1.json", 3_000n * CADRE], ["trader-2.json", 1_000n * CADRE], ["trader-3.json", 1_000n * CADRE]];
  for (const [f, amount] of plan) {
    const w = loadWallet(join(dir, f));
    const have = await c.balance(w.address);
    if (have >= amount / 2n) { console.log(JSON.stringify({ step: "fund", to: w.address, skipped: true, balance: have.toString() })); continue; }
    const s = await c.sendNative(funder, { kind: "Transfer", to: w.publicKey, amount });
    console.log(JSON.stringify({ step: "fund", to: w.address, amount: amount.toString(), tx: s.txHash }));
  }
}

async function codeExists(h: string) {
  return !!(await c.rpc.call("getCode", { code_hash: h }).catch(() => null));
}

async function deploy() {
  const dir = arg("keys");
  const wasm = arg("wasm");
  const outFile = arg("out");
  const out: any = existsSync(outFile) ? JSON.parse(readFileSync(outFile, "utf8")) : {};
  const save = () => writeFileSync(outFile, JSON.stringify(out, null, 2) + "\n");
  const ops = loadWallet(join(dir, "ops.json"));
  const t1 = loadWallet(join(dir, "trader-1.json"));
  const t2 = loadWallet(join(dir, "trader-2.json"));
  const info = await c.init();
  out.chain_id = info.chain_id;
  out.rpc = rpcUrl;
  out.ops = ops.address;

  // 1. code
  for (const name of ["cadre_amm", "cadre_mandate"]) {
    const code = readFileSync(join(wasm, `${name}.wasm`));
    const h = codeHash(code);
    out[`${name}_code`] = h;
    if (await codeExists(h)) { console.log(JSON.stringify({ step: `deploy ${name}`, code_hash: h, skipped: true })); continue; }
    const gas = 40n * BigInt(code.length) + 200_000n; // DeployCode: 40 gas per byte (CONTRACTS 4), plus headroom
    await landed(`deploy ${name}`, await c.sendContract(ops, { kind: "DeployCode", gasLimit: gas, code }));
  }
  save();

  // 2. trader-1 launches its token and pool
  if (!out.agent_token) {
    const nonce = await c.nonceOf(t1);
    const addr = contractAddress(t1.publicKey, nonce);
    const args = new Abi().str("Desk Coin").str("DESK").u8(9).u64(1_000_000_000n * CADRE).bool(false).out();
    await landed("trader-1 launches its token", await c.sendContract(t1, { kind: "Instantiate", codeHash: TOKEN_CODE, admin: new Uint8Array(32), value: 0n, gasLimit: 5_000_000n, label: "DESK agent token", args }));
    out.agent_token = addr;
    out.agent_token_launcher = t1.address;
    save();
  }
  if (!out.amm_pool) {
    const nonce = await c.nonceOf(t1);
    const addr = contractAddress(t1.publicKey, nonce);
    const args = new Abi().addr(out.agent_token).u16(30).addr(new Uint8Array(32)).out();
    await landed("trader-1 opens the CADRE/DESK pool", await c.sendContract(t1, { kind: "Instantiate", codeHash: out.cadre_amm_code, admin: new Uint8Array(32), value: 0n, gasLimit: 5_000_000n, label: "CADRE/DESK pool", args }));
    out.amm_pool = addr;
    save();
  }
  const pool = await c.query(out.amm_pool, Abi.method("info").out());
  pool.addr();
  const rc = pool.u64();
  if (rc === 0n) {
    await landed("approve the pool", await c.sendContract(t1, { kind: "Call", contract: out.agent_token, value: 0n, gasLimit: 0n, args: Abi.method("approve").addr(out.amm_pool).u64((1n << 64n) - 1n).out() }));
    // 1,000 CADRE against 100,000,000 DESK: the agent sets its own launch price
    await landed("seed liquidity", await c.sendContract(t1, { kind: "Call", contract: out.amm_pool, value: 1_000n * CADRE, gasLimit: 0n, args: Abi.method("add_liquidity").u64(100_000_000n * CADRE).u64(1n).u64(0n).out() }));
  }

  // 3. mandate contract
  if (!out.mandate_contract) {
    const nonce = await c.nonceOf(ops);
    const addr = contractAddress(ops.publicKey, nonce);
    const args = new Abi().addr(ops.publicKey).u16(1_000).addr(ops.publicKey).hash(out.cadre_amm_code).out();
    await landed("mandate contract", await c.sendContract(ops, { kind: "Instantiate", codeHash: out.cadre_mandate_code, admin: new Uint8Array(32), value: 0n, gasLimit: 5_000_000n, label: "Cadre mandate v1", args }));
    out.mandate_contract = addr;
    save();
  }

  // 4. a Cadre mandate: ops (user) grants trader-2, 500 CADRE, small limits
  if (out.cadre_mandate_id === undefined) {
    const limits = new Abi()
      .u64(20n * CADRE) // max_in per trade
      .u64(200n * CADRE) // window cap
      .u64(1_440n) // window: 1,440 blocks (one day at 60 s)
      .u16(40) // trades per window
      .u16(300) // max slippage bps
      .u64(500n * CADRE) // min pool CADRE
      .u16(500) // max trade share of the pool, bps
      .u64(0n) // no expiry
      .bool(false)
      .bool(false)
      .u8(1)
      .addr(out.agent_token)
      .out();
    const args = Uint8Array.from([...Abi.method("open").addr(t2.publicKey).addr(t2.publicKey).u16(2_000).u64(60n).out(), ...limits]);
    const s = await c.sendContract(ops, { kind: "Call", contract: out.mandate_contract, value: 500n * CADRE, gasLimit: 0n, args });
    const r = await landed("open a Cadre mandate for trader-2", s);
    const ret = r.receipt?.return;
    out.cadre_mandate_id = ret ? Number(Buffer.from(ret, "hex").readBigUInt64LE(0)) : 0;
    out.cadre_mandate_user = ops.address;
    out.cadre_mandate_agent = t2.address;
    save();
  }
  console.log(JSON.stringify({ done: true, ...out }, null, 2));
}

const cmd = process.argv[2];
(cmd === "fund" ? fund() : cmd === "deploy" ? deploy() : Promise.reject(new Error("usage: deploy-cadre.ts fund|deploy ..."))).catch((e) => {
  console.error(JSON.stringify({ error: (e as Error).message }));
  process.exit(1);
});
