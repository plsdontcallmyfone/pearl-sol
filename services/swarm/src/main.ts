// cadre-swarm CLI (docs/ECOSYSTEM.md 2.6, services/swarm/README.md).
//   node src/main.ts keys <dir>       create operator.json + one key per persona (0600; prints public addresses only)
//   node src/main.ts pubkeys <dir>    print the public addresses (operator + agents) as JSON
//   node src/main.ts run              tick forever (systemd cadre-swarm.service)
//   node src/main.ts once             one tick, then print the status JSON
//   node src/main.ts dm-price         derive each persona's dm_price from the live inputs and print them (read-only, no keys)
// Env: SWARM_KEYS (key dir), CHAIN_RPC, GATEWAY_URL, CADRE_API, SOLANA_RPC, SWARM_STATE,
// SWARM_KILL_FILE, SWARM_STATUS_PORT and the budget knobs listed in README.md.
import { createServer } from "node:http";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { CadreApi, Gateway, latestBlockhash, NodeRpc } from "./clients.ts";
import { dmConfigFromEnv } from "./dms.ts";
import { PERSONAS } from "./personas.ts";
import { type Agent, Swarm, type SwarmConfig } from "./swarm.ts";
import { randomBytes } from "node:crypto";
import { createKeyFile, loadWallet, walletFromSeed } from "./wallet.ts";

const env = process.env;
const num = (k: string, d: number) => (env[k] === undefined || env[k] === "" ? d : Number(env[k]));
const big = (k: string, d: bigint) => (env[k] === undefined || env[k] === "" ? d : BigInt(env[k]!));

export function configFromEnv(): SwarmConfig {
  const rowsPerDay = num("SWARM_ROWS_PER_DAY", 16384);
  return {
    budget: {
      rowsPerDay,
      agentRowsCap: num("SWARM_AGENT_ROWS_PER_DAY", Math.max(1, Math.floor(rowsPerDay / 8))),
      topupLamportsPerDay: big("SWARM_TOPUP_LAMPORTS_PER_DAY", 500_000_000n),
      transferUnitsPerDay: big("SWARM_TRANSFER_PER_DAY", 2_000_000_000n),
      agentFloat: big("SWARM_AGENT_FLOAT", 50_000_000n),
    },
    maxTokens: num("SWARM_MAX_TOKENS", 48),
    judgeMaxTokens: num("SWARM_JUDGE_MAX_TOKENS", 2),
    model: env.SWARM_MODEL || undefined,
    topupRows: num("SWARM_TOPUP_ROWS", 4096),
    topupRoundSecs: env.SWARM_TOPUP_ROUND_SECS ? Number(env.SWARM_TOPUP_ROUND_SECS) : undefined,
    minFreeRows: num("SWARM_MIN_FREE_ROWS", 1024),
    mempoolMax: num("SWARM_MEMPOOL_MAX", 500),
    cadenceScale: num("SWARM_CADENCE_SCALE", 1),
    killFile: env.SWARM_KILL_FILE ?? "/var/lib/lode/swarm/STOP",
    stateFile: env.SWARM_STATE ?? "/var/lib/lode/swarm/state.json",
    allowHosts: (env.SWARM_ALLOW_HOSTS ?? "64-95-12-116.sslip.io").split(",").map((s) => s.trim()).filter(Boolean),
    house: (env.SWARM_HOUSE ?? "1") !== "0",
    reactShare: num("SWARM_REACT_SHARE", 0.2),
    followShare: num("SWARM_FOLLOW_SHARE", 0.1),
    replyShare: num("SWARM_REPLY_SHARE", 0.25),
    solRpc: env.SOLANA_RPC,
    dm: dmConfigFromEnv(env, num("SWARM_TICK_MS", 20_000)),
  };
}

export function loadAgents(dir: string, count = num("SWARM_AGENTS", PERSONAS.length)): { operator: ReturnType<typeof loadWallet>; agents: Agent[] } {
  const operator = loadWallet(resolve(dir, "operator.json"));
  const agents = PERSONAS.slice(0, count).map((persona) => ({ persona, wallet: loadWallet(resolve(dir, `agent-${persona.handle}.json`)) }));
  return { operator, agents };
}

async function main() {
  const [cmd, arg] = process.argv.slice(2);
  if (cmd === "keys" || cmd === "pubkeys") {
    if (!arg) throw new Error(`usage: ${cmd} <dir>`);
    const out: Record<string, string> = {};
    const mk = (f: string) => (cmd === "keys" ? createKeyFile(resolve(arg, f)).address : loadWallet(resolve(arg, f)).address);
    out.operator = mk("operator.json");
    for (const p of PERSONAS) out[p.handle] = mk(`agent-${p.handle}.json`);
    console.log(JSON.stringify(out, null, 2));
    return;
  }
  const cfg = configFromEnv();
  if (cmd === "dm-price") {
    // read-only: getParams from the node, /v1/models from the gateway, /v1/inference/models and
    // /v1/pump/quote from cadre-api; throwaway wallets (only the personas matter for the rows)
    const w = walletFromSeed(randomBytes(32));
    const agents = PERSONAS.slice(0, num("SWARM_AGENTS", PERSONAS.length)).map((persona) => ({ persona, wallet: w }));
    const out: any[] = [];
    const sw = new Swarm({ ...cfg, stateFile: `/tmp/cadre-dm-price-${process.pid}.json`, dm: { ...cfg.dm!, priceOverride: undefined } }, {
      node: new NodeRpc(env.CHAIN_RPC ?? "http://127.0.0.1:10630"), gateway: new Gateway(env.GATEWAY_URL ?? "http://127.0.0.1:10664", w), api: new CadreApi(env.CADRE_API ?? "http://127.0.0.1:10685"),
      latestBlockhash, now: () => Date.now(), rand: Math.random, log: (e) => out.push(e),
    }, w, agents);
    await sw.refreshParams().catch((e) => out.push({ event: "params_unavailable", error: String(e.message) }));
    await sw.dms!.derivePrices();
    for (const e of out) console.log(JSON.stringify(e));
    return;
  }
  const dir = env.SWARM_KEYS ?? "/etc/lode/keys/swarm";
  const { operator, agents } = loadAgents(dir);
  const sw = new Swarm(cfg, {
    node: new NodeRpc(env.CHAIN_RPC ?? "http://127.0.0.1:10630"),
    gateway: new Gateway(env.GATEWAY_URL ?? "http://127.0.0.1:10664", operator),
    api: new CadreApi(env.CADRE_API ?? "http://127.0.0.1:10685"),
    latestBlockhash,
    now: () => Date.now(),
    rand: Math.random,
    log: (e) => console.log(JSON.stringify(e)),
  }, operator, agents);
  if (cmd === "once") {
    console.log(JSON.stringify({ tick: await sw.tick(), status: sw.status() }, null, 2));
    return;
  }
  if (cmd !== "run") throw new Error("usage: keys <dir> | pubkeys <dir> | run | once");
  const port = num("SWARM_STATUS_PORT", 10668);
  createServer((req, res) => {
    const body = req.url === "/health" ? { ok: !sw.stopped, operator: operator.address } : req.url === "/status" ? sw.status() : null;
    res.writeHead(body ? 200 : 404, { "content-type": "application/json" });
    res.end(JSON.stringify(body ?? { error: "not found" }));
  }).listen(port, "127.0.0.1");
  let stop = false;
  process.on("SIGTERM", () => { stop = true; sw.stopped = true; });
  process.on("SIGINT", () => { stop = true; sw.stopped = true; });
  sw.log("start", { operator: operator.address, agents: agents.length, rows_per_day: cfg.budget.rowsPerDay, kill_file: cfg.killFile, status_port: port });
  const tickMs = num("SWARM_TICK_MS", 20_000);
  while (!stop && !existsSync(cfg.killFile)) {
    const r = await sw.tick();
    if (r === "killed") break;
    await new Promise((res) => setTimeout(res, r === "bootstrap" ? Math.min(tickMs, 5_000) : tickMs));
  }
  sw.log("stop", { reason: stop ? "signal" : "kill file" });
  process.exit(0);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error(JSON.stringify({ event: "fatal", error: String(e?.message ?? e) }));
    process.exit(1);
  });
}
