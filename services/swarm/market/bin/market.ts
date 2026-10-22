// House market desk on Cadre devnet (docs/MARKET.md 6). Keys are passed by directory and never
// printed; only addresses are. Idempotent steps:
//   node bin/market.ts keygen   --keys <dir>                         ops.json, judge.json, agent-<handle>.json (0600)
//   node bin/market.ts fund     --keys <dir> --funder <key> [--cadre-ops 3000 --cadre-agent 200 --cadre-judge 50 --sol 0.3]
//   node bin/market.ts deploy   --keys <dir> --wasm <cadre_market.wasm> --out <deploy/market-devnet.json> [--fee-bps 2000]
//   node bin/market.ts link     --keys <dir> --operator <key> [--only h1,h2]       OperatorLink from the house operator (FIRST: the house flag needs it)
//   node bin/market.ts profiles --keys <dir> --operator-address <house operator>    SetProfile (house flag) per agent
//   node bin/market.ts offers   --keys <dir> --deploy <file>                        list the persona offers
//   node bin/market.ts run      --keys <dir> --deploy <file> [--once]               the desk loop
//   node bin/market.ts status   --deploy <file>
// Env: CADRE_RPC, GATEWAY_URL (default cadre-api's /v1/openai), SOLANA_RPC, MARKET_HIRE_EVERY_S (900),
// MARKET_DAILY_CAP_CADRE (60 per buyer), MARKET_JUDGE_FEE_CADRE (0.5), MARKET_KILL_FILE, MARKET_STATE,
// MARKET_BLOB_NODES (comma list of other nodes that also store every desk blob; default devnet nodes 1 and 2).
import { existsSync, mkdirSync, readFileSync, writeFileSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { createKeyFile, loadWallet, type Wallet } from "../../src/wallet.ts";
import { latestBlockhash, NodeRpc } from "../../src/clients.ts";
import { nativeTransferTx } from "../../src/pay.ts";
import { FLAG } from "../../src/chaintx.ts";
import { b58decode } from "../../src/b58.ts";
import { Cadre, codeHash, contractAddress, errText } from "../../trading/src/cadre.ts";
import { Brain } from "../../trading/src/brain.ts";
import { marketArgs, MarketIndex } from "../../../../apps/explorer/src/market.ts";
import { JUDGE, PERSONAS } from "../src/personas.ts";
import { CADRE, Desk, type DeskAgent } from "../src/desk.ts";

const env = process.env;
function arg(name: string, def?: string): string {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1] && !process.argv[i + 1].startsWith("--")) return process.argv[i + 1];
  if (def !== undefined) return def;
  throw new Error(`--${name} is required`);
}
const flag = (n: string) => process.argv.includes(`--${n}`);
const rpcUrl = env.CADRE_RPC ?? "https://rpc.64-95-12-116.sslip.io";
const solRpc = env.SOLANA_RPC ?? "https://api.devnet.solana.com";
const gatewayUrl = env.GATEWAY_URL ?? "https://api.64-95-12-116.sslip.io/v1/openai";
const log = (event: string, x: Record<string, unknown>) => console.log(JSON.stringify({ t: new Date().toISOString(), event, ...x }));
const cadreOf = (s: string) => BigInt(Math.round(Number(s) * 1e9));

function keyFiles(dir: string) {
  return { ops: join(dir, "ops.json"), judge: join(dir, "judge.json"), agents: PERSONAS.map((p) => ({ p, file: join(dir, `agent-${p.handle}.json`) })) };
}

async function landed(c: Cadre, what: string, s: { txHash: string }) {
  const r = await c.waitIncluded(s.txHash, 900_000, 2_000);
  const ok = r.receipt ? r.receipt.ok : true;
  log(what, { tx: s.txHash, height: r.height, ok, error: r.receipt && !r.receipt.ok ? errText(r.receipt.error) || r.receipt.error : undefined });
  if (!ok) throw new Error(`${what} failed at ${r.height}`);
  return r;
}

async function solTransfer(from: Wallet, to: string, lamports: bigint) {
  const bh = await latestBlockhash(solRpc);
  const { tx } = nativeTransferTx(from, { scheme: "exact", network: "solana-devnet", maxAmountRequired: lamports.toString(), payTo: to }, bh);
  const r = await fetch(solRpc, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "sendTransaction", params: [Buffer.from(tx).toString("base64"), { encoding: "base64" }] }) });
  const j: any = await r.json();
  if (j.error) throw new Error(`SOL transfer: ${JSON.stringify(j.error).slice(0, 200)}`);
  return j.result as string;
}
async function solBalance(addr: string) {
  const r = await fetch(solRpc, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getBalance", params: [addr] }) });
  return BigInt((await r.json() as any).result?.value ?? 0);
}

async function main() {
  const cmd = process.argv[2];
  const c = new Cadre(rpcUrl);
  if (cmd === "keygen") {
    const dir = arg("keys");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    chmodSync(dir, 0o700);
    const k = keyFiles(dir);
    for (const [name, f] of [["ops", k.ops], ["judge", k.judge], ...k.agents.map((a) => [a.p.handle, a.file])] as [string, string][]) {
      const r = createKeyFile(f);
      log("key", { name, address: r.address, created: r.created });
    }
    return;
  }
  if (cmd === "fund") {
    const dir = arg("keys");
    const funder = loadWallet(arg("funder"));
    const k = keyFiles(dir);
    await c.init();
    const plan: [string, bigint][] = [[k.ops, cadreOf(arg("cadre-ops", "3000"))], [k.judge, cadreOf(arg("cadre-judge", "50"))], ...k.agents.map((a) => [a.file, cadreOf(arg("cadre-agent", "200"))] as [string, bigint])];
    const sol = BigInt(Math.round(Number(arg("sol", "0.3")) * 1e9));
    for (const [f, amt] of plan) {
      const w = loadWallet(f);
      const have = await c.balance(w.address);
      if (have < amt / 2n) await landed(c, `fund_cadre ${w.address}`, await c.sendNative(funder, { kind: "Transfer", to: w.publicKey, amount: amt - have }));
      if (f !== k.ops && sol > 0n) {
        const sb = await solBalance(w.address);
        if (sb < sol / 2n) log("fund_sol", { to: w.address, lamports: (sol - sb).toString(), sig: await solTransfer(funder, w.address, sol - sb) });
      }
    }
    return;
  }
  if (cmd === "deploy") {
    const dir = arg("keys");
    const ops = loadWallet(keyFiles(dir).ops);
    const code = readFileSync(arg("wasm"));
    const out = arg("out");
    await c.init();
    const hash = codeHash(code);
    const have = await c.rpc.call("getCode", { code_hash: hash }).catch(() => null);
    if (!have) await landed(c, "deploy_code", await c.sendContract(ops, { kind: "DeployCode", gasLimit: BigInt(40 * code.length + 200_000), code }));
    const prev = existsSync(out) ? JSON.parse(readFileSync(out, "utf8")) : null;
    if (prev?.market && prev.code_hash === hash) { log("exists", { market: prev.market }); return; }
    const nonce = await c.nonceOf(ops);
    const addr = contractAddress(ops.publicKey, nonce);
    const bps = Number(arg("fee-bps", "2000"));
    const r = await landed(c, "instantiate", await c.sendContract(ops, { kind: "Instantiate", codeHash: hash, admin: ops.publicKey, value: 0n, gasLimit: 5_000_000n, label: "cadre-market", args: marketArgs.instantiate(ops.address, bps, ops.address) }));
    const judge = loadWallet(keyFiles(dir).judge);
    const info = await c.rpc.call("getChainInfo");
    const doc = {
      note: "Cadre services market (contracts/market, docs/MARKET.md). Admin and fee_to: the market ops key (devnet). House desk agents and the LLM judge below are labelled house.",
      chain_id: info.chain_id, rpc: rpcUrl, market: addr, code_hash: hash, admin: ops.address, fee_bps: bps, fee_to: ops.address, deployed_height: r.height,
      judge: judge.address,
      house: Object.fromEntries([[judge.address, JUDGE.handle], ...keyFiles(dir).agents.map((a) => [loadWallet(a.file).address, a.p.handle])]),
    };
    writeFileSync(out, JSON.stringify(doc, null, 2) + "\n");
    log("deployed", { market: addr, code_hash: hash, out });
    return;
  }
  if (cmd === "profiles") {
    const dir = arg("keys");
    const op = b58decode(arg("operator-address"));
    await c.init();
    const k = keyFiles(dir);
    const people = [{ file: k.judge, handle: JUDGE.handle, name: JUDGE.name, persona: JUDGE.persona, appearance: JUDGE.appearance, offers: [] as string[], role: "llm-judge" }, ...k.agents.map((a) => ({ file: a.file, handle: a.p.handle, name: a.p.name, persona: a.p.persona, appearance: a.p.appearance, offers: a.p.offers.map((o) => o.title), role: "market-desk" }))];
    for (const p of people) {
      const w = loadWallet(p.file);
      const prof = await c.rpc.call("getProfile", { account: w.address }).catch(() => null);
      if (prof?.handle === p.handle) { log("profile_exists", { handle: p.handle }); continue; }
      const blob = Buffer.from(JSON.stringify({ format: "cadre-profile-v1", bio: p.persona, persona: p.persona, appearance: p.appearance, archetype: p.role, tags: ["market", p.role], offers: p.offers, house: true, links: [] }));
      const put = await c.rpc.putBlob(blob, true);
      const id = String(put.uri).replace(/^blob:/, "");
      await landed(c, `profile ${p.handle}`, await c.sendNative(w, { kind: "SetProfile", handle: p.handle, name: p.name, profileUri: put.uri, profileHash: Buffer.from(id, "hex"), flags: FLAG.agent | FLAG.house | (p.role === "market-desk" ? FLAG.hires : 0), operator: op }));
    }
    return;
  }
  if (cmd === "link") {
    const dir = arg("keys");
    const op = loadWallet(arg("operator"));
    await c.init();
    const k = keyFiles(dir);
    const only = process.argv.includes("--only") ? arg("only").split(",") : null; // handles, to finish a partial run
    for (const [h, f] of [[JUDGE.handle, k.judge], ...k.agents.map((a) => [a.p.handle, a.file])] as [string, string][]) {
      if (only && !only.includes(h)) continue;
      const w = loadWallet(f);
      // the link is an OperatorLink tx naming this agent (role operator); a profile may not exist yet
      const hist = await c.rpc.call("getAccountTxs", { account: w.address }).catch(() => null);
      const txs: any[] = hist?.txs ?? (Array.isArray(hist) ? hist : []);
      if (txs.some((t) => t.kind === "OperatorLink" && t.role === "operator" && t.from === op.address)) { log("link_exists", { handle: h }); continue; }
      await landed(c, `operator_link ${w.address}`, await c.sendNative(op, { kind: "OperatorLink", agent: w.publicKey, add: true }));
    }
    return;
  }
  const dep = JSON.parse(readFileSync(arg("deploy"), "utf8"));
  if (cmd === "status") {
    const ix = await new MarketIndex(dep.market, (m, p) => c.rpc.call(m, p as any)).refresh();
    console.log(JSON.stringify({ config: ix.config, offers: ix.offers.length, hires: ix.hires.size, by_status: [...ix.hires.values()].reduce((a: any, h) => ((a[h.status] = (a[h.status] ?? 0) + 1), a), {}), events: ix.events.length }, null, 2));
    return;
  }
  const dir = arg("keys");
  const k = keyFiles(dir);
  await c.init();
  const agents: DeskAgent[] = k.agents.map((a) => {
    const wallet = loadWallet(a.file);
    return { persona: a.p, wallet, brain: new Brain({ gatewayUrl, wallet, solRpc, topupRows: 2_048, maxTopupLamportsPerDay: 150_000_000n, log }) };
  });
  const desk = new Desk(c, agents, {
    market: dep.market, judge: dep.judge, judgeFee: cadreOf(env.MARKET_JUDGE_FEE_CADRE ?? "0.5"), dailyCapCadre: cadreOf(env.MARKET_DAILY_CAP_CADRE ?? "60"),
    hireEverySec: Number(env.MARKET_HIRE_EVERY_S ?? 900), stateFile: env.MARKET_STATE ?? join(dir, "..", "market-desk-state.json"), killFile: env.MARKET_KILL_FILE, postReceipts: env.MARKET_POSTS !== "off",
    blobMirrors: (env.MARKET_BLOB_NODES ?? "https://rpc.64-95-12-22.sslip.io,https://rpc.64-95-13-213.sslip.io").split(",").filter(Boolean).map((u) => new NodeRpc(u.trim())),
  }, log);
  if (cmd === "offers") { log("offers_listed", { n: await desk.ensureOffers() }); return; }
  if (cmd === "run") {
    await desk.ensureOffers();
    for (;;) {
      try { await desk.tick(); } catch (e) { log("tick_error", { error: (e as Error).message.slice(0, 300) }); }
      if (flag("once")) return;
      await new Promise((r) => setTimeout(r, Number(env.MARKET_TICK_S ?? 30) * 1000));
    }
  }
  throw new Error(`unknown command ${cmd}`);
}

main().catch((e) => { console.error(JSON.stringify({ event: "fatal", error: (e as Error).message })); process.exit(1); });
