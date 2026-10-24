// Practice-mode house agents (docs/STRATEGIES.md 6):
//   node bin/practice.ts deploy --key <cadre key.json> [--out deploy/strategies-devnet.json] [--start <height>]
//        upload + instantiate contracts/strategies (admin zero: immutable) and write the deploy file
//   node bin/practice.ts keys --keys <dir>             create the house agents' keys (0600) if missing; prints addresses only
//   node bin/practice.ts run --keys <dir> [--config deploy/strategies-devnet.json] [--state <dir>] [--agents a,b]
//   node bin/practice.ts sync-blobs [--state <dir>]    re-send every kept blob to cadre-api's durable store
//   node bin/practice.ts status [--state <dir>]
// Env: CADRE_RPC (default devnet node 0), GATEWAY_URL (default cadre-api /v1/openai on devnet),
// CADRE_API (blob store), SOLANA_RPC (Solana DEVNET, only for x402 top-ups of the agents' own
// prepaid balance), STRAT_MAINNET_RPC or STRAT_MAINNET_RPC_FILE (mainnet READS only; never
// logged), STRAT_TICK_MS (default 20000), STRAT_KILL_FILE (default <state>/STOP), STRAT_REVISE_AFTER.
// STRAT_DESK_TRADERS (comma list, e.g. foxglove47,meadowsweet27,primrosedusk54,boragemist71) with
// STRAT_DESK_KEYS (default /etc/lode/keys/swarm): the house desktop agents whose job is practice
// trading (services/swarm/desktop/src/roster.ts, docs/HOUSE-AGENTS.md) trade here from their own
// keys (agent-<handle>.json); their desktops show the trades from <state>/<handle>.json.
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createKeyFile, loadWallet } from "../../src/wallet.ts";
import { Brain } from "../../trading/src/brain.ts";
import { Cadre, codeHash, contractAddress } from "../../trading/src/cadre.ts";
import { type AgentCfg, analyzerHash, PracticeAgent } from "../src/agent.ts";
import { mainnetUrl, ReadOnlySolana } from "../src/mainnet.ts";
import { discover, Market } from "../src/market.ts";
import { decodeCounts, q } from "../src/records.ts";
import { BlobStore } from "../src/store.ts";
import type { Think } from "../src/thoughts.ts";
import { deskTraders } from "../src/desktraders.ts";

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, "../../../..");
const env = process.env;
function arg(name: string, def?: string): string {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1]) return process.argv[i + 1];
  if (def !== undefined) return def;
  throw new Error(`--${name} is required`);
}
const log = (event: string, x: Record<string, unknown> = {}) => console.log(JSON.stringify({ t: new Date().toISOString(), event, ...x }, (_k, v) => (typeof v === "bigint" ? v.toString() : v)));

const SIZE = { equity_fraction_bps: 1_000, max_lamports: 100_000_000, max_pool_share_bps: 20 };
const BASE = { size: SIZE, max_risk_bps: 100, max_slippage_bps: 150, practice_balance_lamports: 1_000_000_000 };

/** The practice house agents. Personas are ours (no real people); every one is labelled house. */
export const HOUSE: (AgentCfg & { key: string })[] = [
  { key: "practice-1.json", handle: "tape_lark", name: "Tape Lark", persona: "Tape Lark, a momentum trader who buys strength in deep pools", base: { ...BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 60 }, { signal: "ret_5m", op: ">=", value: 0.01 }, { signal: "ret_15m", op: ">=", value: 0.03 }, { signal: "flow_15m", op: ">=", value: 0 }] } },
  { key: "practice-2.json", handle: "floor_wren", name: "Floor Wren", persona: "Floor Wren, who buys the first bounce after a fall", base: { ...BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 80 }, { signal: "ret_15m", op: "<=", value: -0.05 }, { signal: "ret_5m", op: ">=", value: 0.005 }] } },
  { key: "practice-3.json", handle: "still_heron", name: "Still Heron", persona: "Still Heron, who only trades calm, liquid pools", base: { ...BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 150 }, { signal: "ret_15m", op: ">=", value: 0.01 }, { signal: "vol_15m", op: "<=", value: 0.01 }, { signal: "top10_share", op: "<=", value: 0.3 }] } },
];

const cadreRpc = () => env.CADRE_RPC ?? "https://rpc.64-95-12-116.sslip.io";

async function deploy() {
  const w = loadWallet(arg("key"));
  const out = resolve(arg("out", join(REPO, "deploy/strategies-devnet.json")));
  const code = readFileSync(join(REPO, "contracts/target/wasm32-unknown-unknown/release/cadre_strategies.wasm"));
  const cadre = new Cadre(cadreRpc());
  const info = await cadre.init();
  const ch = codeHash(code);
  const have = await cadre.rpc.call("getCode", { code_hash: ch }).catch(() => null);
  if (!have) {
    const gas = 40n * BigInt(code.length) + 200_000n; // DeployCode: 40 gas per byte (CONTRACTS 4), plus headroom
    const s = await cadre.sendContract(w, { kind: "DeployCode", gasLimit: gas, code });
    const inc = await cadre.waitIncluded(s.txHash);
    log("code_deployed", { code_hash: ch, tx: s.txHash, height: inc.height, ok: inc.receipt?.ok, error: inc.receipt?.error });
  } else log("code_present", { code_hash: ch });
  const start = BigInt(arg("start", "0"));
  const args = Buffer.alloc(8);
  args.writeBigUInt64LE(start);
  const nonce = await cadre.nonceOf(w);
  const addr = contractAddress(w.publicKey, nonce);
  const s = await cadre.sendContract(w, { kind: "Instantiate", codeHash: ch, admin: new Uint8Array(32), value: 0n, gasLimit: 5_000_000n, label: "strategies", args });
  const inc = await cadre.waitIncluded(s.txHash);
  if (!inc.receipt?.ok) throw new Error(`instantiate failed: ${inc.receipt?.error}`);
  const doc = { network: "devnet", chain_id: info.chain_id, contract: addr, code_hash: ch, admin: "zero (immutable)", start_height: Number(start), instantiate_tx: s.txHash, instantiated_height: inc.height, analyzer: "cadre-postmortem-v1", analyzer_hash: analyzerHash(), house: [] as any[] };
  if (existsSync(out)) Object.assign(doc, { house: JSON.parse(readFileSync(out, "utf8")).house ?? [] });
  writeFileSync(out, JSON.stringify(doc, null, 2) + "\n");
  log("instantiated", { contract: addr, height: inc.height, out });
}

function keys() {
  const dir = resolve(arg("keys"));
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const out = resolve(arg("config", join(REPO, "deploy/strategies-devnet.json")));
  const house = HOUSE.map((h) => {
    const f = join(dir, h.key);
    const address = createKeyFile(f).address;
    return { handle: h.handle, name: h.name, account: address, key: h.key, house: true, persona: h.persona };
  });
  if (existsSync(out)) {
    const doc = JSON.parse(readFileSync(out, "utf8"));
    doc.house = house;
    writeFileSync(out, JSON.stringify(doc, null, 2) + "\n");
  }
  for (const h of house) log("key", { handle: h.handle, account: h.account });
}

function thinker(w: ReturnType<typeof loadWallet>): Think {
  const brain = new Brain({ gatewayUrl: env.GATEWAY_URL ?? "https://api.64-95-12-116.sslip.io/v1/openai", wallet: w, solRpc: env.SOLANA_RPC ?? "https://api.devnet.solana.com", topupRows: 4_096, maxTopupLamportsPerDay: 100_000_000n, log: (e, x) => log(e, { agent: w.address, ...x }) });
  return async (messages, maxTokens) => {
    await brain.ensureRows(512);
    const t = await brain.think(messages, maxTokens);
    if (!t.requestId) throw new Error("the gateway returned no request id: a thought without a receipt is not committed");
    log("thought", { agent: w.address, request: t.requestId, rows: t.rows, ms: Math.round(t.totalMs), text: t.text.slice(0, 160) });
    return { text: t.text, requestId: t.requestId };
  };
}

async function run() {
  const cfg = JSON.parse(readFileSync(resolve(arg("config", join(REPO, "deploy/strategies-devnet.json"))), "utf8"));
  const kdir = resolve(arg("keys"));
  const stateDir = resolve(arg("state", join(env.HOME ?? ".", "practice")));
  mkdirSync(stateDir, { recursive: true });
  const kill = env.STRAT_KILL_FILE ?? join(stateDir, "STOP");
  const tickMs = Number(env.STRAT_TICK_MS ?? 20_000);
  const only = process.argv.includes("--agents") ? arg("agents").split(",") : null;
  const sol = new ReadOnlySolana(mainnetUrl(env, REPO));
  await sol.ensureMainnet();
  const cadre = new Cadre(cadreRpc());
  await cadre.init();
  const counts = decodeCounts((await cadre.query(cfg.contract, q.counts())).b);
  log("practice_start", { contract: cfg.contract, versions: counts.versions, entries: counts.entries, tick_ms: tickMs, analyzer_hash: analyzerHash(), mainnet: "read-only" });
  const market = new Market(sol);
  const store = new BlobStore({ dir: join(stateDir, "blobs"), api: env.CADRE_API ?? "https://api.64-95-12-116.sslip.io", nodeRpc: (m, p) => cadre.rpc.call(m, p), log });
  const ah = analyzerHash();
  if (cfg.analyzer_hash && cfg.analyzer_hash !== ah) log("analyzer_changed", { deployed: cfg.analyzer_hash, running: ah, note: "post-mortems commit the hash of the code that ran" });
  const desk = deskTraders(env.STRAT_DESK_TRADERS, env.STRAT_DESK_KEYS ?? "/etc/lode/keys/swarm").filter((d) => existsSync(d.keyPath));
  const all = [...HOUSE.map((h) => ({ ...h, keyPath: join(kdir, h.key) })), ...desk];
  if (desk.length) log("desk_traders", { handles: desk.map((d) => d.handle) });
  const agents = all.filter((h) => !only || only.includes(h.handle)).map((h) => {
    const w = loadWallet(h.keyPath);
    return new PracticeAgent(h, { cadre, contract: cfg.contract, market, store, think: thinker(w), wallet: w, stateFile: join(stateDir, `${h.handle}.json`), tickMs, analyzerHash: ah, log, reviseAfter: env.STRAT_REVISE_AFTER ? Number(env.STRAT_REVISE_AFTER) : undefined });
  });
  let lastDiscover = 0;
  for (;;) {
    if (existsSync(kill)) { log("killed", { file: kill }); await new Promise((r) => setTimeout(r, tickMs)); continue; }
    try {
      if (Date.now() - lastDiscover > 15 * 60_000 || market.pools.size === 0) {
        const found = await discover(sol, { signatures: 100, txs: 40 });
        const keep = new Set(agents.map((a) => a.s.open?.pool).filter((x): x is string => !!x));
        const added = await market.refresh(found, { minPoolSol: 30, max: 12, keep });
        lastDiscover = Date.now();
        log("discover", { found: found.size, added: added.length, evicted: market.evicted.length, watched: market.pools.size, mainnet_calls: sol.calls });
      }
      const slot = await market.tick();
      log("tick", { slot, pools: market.pools.size, mainnet_calls: sol.calls });
    } catch (e) {
      log("tick_error", { message: (e as Error).message.slice(0, 300) });
    }
    for (const a of agents) a.tick(); // each agent runs its own flow; a busy agent skips the tick
    await new Promise((r) => setTimeout(r, tickMs));
  }
}

async function syncBlobs() {
  const stateDir = resolve(arg("state", join(env.HOME ?? ".", "practice")));
  const store = new BlobStore({ dir: join(stateDir, "blobs"), api: env.CADRE_API ?? "https://api.64-95-12-116.sslip.io", log });
  log("sync_blobs", await store.syncToApi());
}

function status() {
  const stateDir = resolve(arg("state", join(env.HOME ?? ".", "practice")));
  for (const f of readdirSync(stateDir).filter((x) => x.endsWith(".json"))) {
    const s = JSON.parse(readFileSync(join(stateDir, f), "utf8"));
    log("agent", { file: f, version: s.version, open: s.open ? { entry: s.open.entry, stage: s.open.stage, pool: s.open.pool } : null, explained: s.history.length, pnl: s.history.reduce((a: bigint, h: any) => a + BigInt(h.pnl), 0n) });
  }
}

const cmd = process.argv[2];
const main = { deploy, keys: async () => keys(), run, "sync-blobs": syncBlobs, status: async () => status() }[cmd as string];
if (!main) {
  console.error("usage: practice.ts deploy|keys|run|sync-blobs|status (see the header)");
  process.exit(2);
}
main().catch((e) => { log("fatal", { message: (e as Error).message }); process.exit(1); });
