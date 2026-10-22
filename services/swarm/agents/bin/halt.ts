#!/usr/bin/env node
// The protocol halt record (contracts/halt, docs/ECOSYSTEM.md 7.8). Keys are passed by file and
// never printed; only addresses are.
//   node bin/halt.ts deploy --key <authority.json> --wasm <cadre_halt.wasm> --out <deploy/halt-devnet.json>
//        DeployCode + Instantiate with admin ZERO (immutable code) and the key as halt authority
//   node bin/halt.ts status [--deploy <file>]                        the record (anyone)
//   node bin/halt.ts set --key <authority.json> --scope all|trade|launch|trade,launch|none --reason "<text>" [--deploy <file>]
//   node bin/halt.ts authority --key <authority.json> --to <address> [--deploy <file>]
// Env: CADRE_RPC (default the public devnet node).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadWallet } from "../../src/wallet.ts";
import { b58decode } from "../../src/b58.ts";
import { Cadre, codeHash, contractAddress, errText } from "../../trading/src/cadre.ts";
import { decodeHalt, HALT, STATUS_ARGS } from "../src/halt.ts";

const here = dirname(fileURLToPath(import.meta.url));
const DEFAULT_DEPLOY = join(here, "../../../../deploy/halt-devnet.json");
const rpcUrl = process.env.CADRE_RPC ?? "https://rpc.64-95-12-116.sslip.io";
const log = (event: string, x: Record<string, unknown>) => console.log(JSON.stringify({ t: new Date().toISOString(), event, ...x }));
function arg(name: string, def?: string): string {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1] !== undefined && !process.argv[i + 1].startsWith("--")) return process.argv[i + 1];
  if (def !== undefined) return def;
  throw new Error(`--${name} is required`);
}
const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const method = (m: string) => Buffer.concat([Buffer.of(m.length), Buffer.from(m)]);

async function landed(c: Cadre, what: string, s: { txHash: string }) {
  const r = await c.waitIncluded(s.txHash, 900_000, 2_000);
  const ok = r.receipt ? r.receipt.ok : true;
  log(what, { tx: s.txHash, height: r.height, ok, error: r.receipt && !r.receipt.ok ? errText(r.receipt.error) || r.receipt.error : undefined });
  if (!ok) throw new Error(`${what} failed at ${r.height}`);
  return r;
}

async function status(c: Cadre, contract: string) {
  const r = await c.rpc.call("queryContract", { address: contract, args: STATUS_ARGS.toString("hex") });
  if (!r.ok) throw new Error("status query failed");
  const h = decodeHalt(Buffer.from(r.return, "hex"));
  return { contract, scope: h.scope, halted: { all: !!(h.scope & HALT.all), trade: !!(h.scope & HALT.trade), launch: !!(h.scope & HALT.launch) }, seq: h.seq.toString(), height: h.height.toString(), authority: h.authority, reason: h.reason };
}

function scopeOf(s: string): number {
  if (s === "none" || s === "0") return 0;
  return s.split(",").reduce((m, x) => {
    const k = x.trim() as keyof typeof HALT;
    if (!(k in HALT)) throw new Error(`unknown scope ${x} (all, trade, launch, none)`);
    return m | HALT[k];
  }, 0);
}

async function main() {
  const cmd = process.argv[2];
  const c = new Cadre(rpcUrl);
  await c.init();
  if (cmd === "deploy") {
    const w = loadWallet(resolve(arg("key")));
    const code = readFileSync(arg("wasm"));
    const out = arg("out");
    const hash = codeHash(code);
    if (!(await c.rpc.call("getCode", { code_hash: hash }).catch(() => null))) await landed(c, "deploy_code", await c.sendContract(w, { kind: "DeployCode", gasLimit: BigInt(40 * code.length + 200_000), code }));
    if (existsSync(out)) { const prev = JSON.parse(readFileSync(out, "utf8")); if (prev.contract && prev.code_hash === hash) { log("exists", { contract: prev.contract }); return; } }
    const nonce = await c.nonceOf(w);
    const addr = contractAddress(w.publicKey, nonce);
    const r = await landed(c, "instantiate", await c.sendContract(w, { kind: "Instantiate", codeHash: hash, admin: new Uint8Array(32), value: 0n, gasLimit: 5_000_000n, label: "cadre-halt", args: w.publicKey }));
    const info = await c.rpc.call("getChainInfo");
    writeFileSync(out, JSON.stringify({
      note: "Cadre protocol halt record (contracts/halt, docs/ECOSYSTEM.md 7.8). Immutable code (admin zero). The authority signs halts; hosted agents (cadre-agents) enforce it, a self-hosted cadre-agent obeys it as shipped (advisory: whoever runs one can change it). Scope bits: 1 all, 2 trade, 4 launch.",
      chain_id: info.chain_id, rpc: rpcUrl, contract: addr, code_hash: hash, authority: w.address, deployed_height: r.height,
    }, null, 2) + "\n");
    log("deployed", { contract: addr, code_hash: hash, authority: w.address, out });
    return;
  }
  const dep = JSON.parse(readFileSync(arg("deploy", DEFAULT_DEPLOY), "utf8"));
  if (cmd === "status") { console.log(JSON.stringify(await status(c, dep.contract), null, 2)); return; }
  if (cmd === "set") {
    const w = loadWallet(resolve(arg("key")));
    const scope = scopeOf(arg("scope"));
    const reason = Buffer.from(arg("reason", ""), "utf8");
    const args = Buffer.concat([method("set"), Buffer.of(scope), u32(reason.length), reason]);
    await landed(c, "halt_set", await c.sendContract(w, { kind: "Call", contract: dep.contract, value: 0n, gasLimit: 0n, args }));
    console.log(JSON.stringify(await status(c, dep.contract), null, 2));
    return;
  }
  if (cmd === "authority") {
    const w = loadWallet(resolve(arg("key")));
    const to = b58decode(arg("to"));
    if (to.length !== 32) throw new Error("--to must be a 32-byte address");
    await landed(c, "halt_authority", await c.sendContract(w, { kind: "Call", contract: dep.contract, value: 0n, gasLimit: 0n, args: Buffer.concat([method("set_authority"), Buffer.from(to)]) }));
    console.log(JSON.stringify(await status(c, dep.contract), null, 2));
    return;
  }
  console.log("usage: halt.ts deploy|status|set|authority (see the header)");
  process.exit(cmd ? 1 : 0);
}

main().catch((e) => { console.error(JSON.stringify({ event: "fatal", error: (e as Error).message })); process.exit(1); });
