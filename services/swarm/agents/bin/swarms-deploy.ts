// Deploy the swarms contract (contracts/swarms, docs/SWARMS.md 7) on the Cadre devnet:
//   node bin/swarms-deploy.ts --key <cadre key.json> [--start <height>] [--out deploy/swarms-devnet.json]
// Uploads the wasm (contracts/target/wasm32-unknown-unknown/release/cadre_swarms.wasm) when the
// node does not hold its code yet, instantiates it with admin zero (immutable code) and a start
// height, and writes the deploy file the runtime and cadre-api read. Prints addresses only.
// Env: CADRE_RPC (default devnet node 0).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { loadWallet } from "../../src/wallet.ts";
import { Cadre, codeHash, contractAddress } from "../../trading/src/cadre.ts";
import { REPO } from "../src/swarmabi.ts";

const arg = (name: string, def?: string) => {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1]) return process.argv[i + 1];
  if (def !== undefined) return def;
  throw new Error(`--${name} is required`);
};
const log = (event: string, x: Record<string, unknown> = {}) => console.log(JSON.stringify({ t: new Date().toISOString(), event, ...x }));

async function main() {
  const w = loadWallet(arg("key"));
  const out = resolve(arg("out", join(REPO, "deploy/swarms-devnet.json")));
  const wasm = join(REPO, "contracts/target/wasm32-unknown-unknown/release/cadre_swarms.wasm");
  if (!existsSync(wasm)) throw new Error(`build it first: (cd contracts && cargo build --release --target wasm32-unknown-unknown -p cadre-swarms)`);
  const code = readFileSync(wasm);
  const cadre = new Cadre(process.env.CADRE_RPC ?? "https://rpc.64-95-12-116.sslip.io");
  const info = await cadre.init();
  log("deployer", { account: w.address, balance: (await cadre.balance(w.address)).toString(), height: info.height });
  const ch = codeHash(code);
  const have = await cadre.rpc.call("getCode", { code_hash: ch }).catch(() => null);
  if (!have) {
    // the code's storage deposit is storage_byte_deposit per byte, held from the deployer (CONTRACTS 5)
    const deposit = BigInt(cadre.params!.storage_byte_deposit ?? 100_000) * BigInt(code.length);
    const bal = await cadre.balance(w.address);
    if (bal < deposit + 1_000_000n) throw new Error(`the deployer holds ${bal} units; the code deposit alone is ${deposit} (fund it first)`);
    const s = await cadre.sendContract(w, { kind: "DeployCode", gasLimit: 40n * BigInt(code.length) + 200_000n, code });
    const inc = await cadre.waitIncluded(s.txHash);
    if (inc.receipt && inc.receipt.ok === false) throw new Error(`DeployCode failed: ${inc.receipt.error}`);
    log("code_deployed", { code_hash: ch, bytes: code.length, tx: s.txHash, height: inc.height });
  } else log("code_present", { code_hash: ch });
  const start = BigInt(arg("start", String(info.height + 5)));
  const args = Buffer.alloc(8);
  args.writeBigUInt64LE(start);
  const nonce = await cadre.nonceOf(w);
  const addr = contractAddress(w.publicKey, nonce);
  const s = await cadre.sendContract(w, { kind: "Instantiate", codeHash: ch, admin: new Uint8Array(32), value: 0n, gasLimit: 5_000_000n, label: "swarms", args });
  const inc = await cadre.waitIncluded(s.txHash);
  if (!inc.receipt?.ok) throw new Error(`instantiate failed: ${inc.receipt?.error}`);
  writeFileSync(out, JSON.stringify({ network: "devnet", chain_id: info.chain_id, contract: addr, code_hash: ch, admin: "zero (immutable)", start_height: Number(start), deployer: w.address, instantiate_tx: s.txHash, instantiated_height: inc.height }, null, 2) + "\n");
  log("instantiated", { contract: addr, start_height: Number(start), height: inc.height, out });
}

main().catch((e) => { log("fatal", { message: (e as Error).message }); process.exit(1); });
