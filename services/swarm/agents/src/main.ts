// cadre-agents service (hosted user agents; systemd deploy/systemd/cadre-agents.service).
//   node src/main.ts run      serve /v1/hosted on AGENTS_PORT (default 10730) and tick the agents
//   node src/main.ts info     print the runtime's public info (seal key, limits) and exit
// Env: AGENTS_STATE (default /var/lib/lode/agents), AGENTS_MASTER_KEY (default
// /etc/lode/keys/agents/master.hex, created 0600 when missing), CHAIN_RPC, GATEWAY_URL,
// CADRE_API, CADRE_PUBLIC_API, SOLANA_RPC, CADRE_NETWORK, AGENTS_ALLOW_HOSTS,
// AGENTS_MAX_PER_OPERATOR (5), AGENTS_MAX (200), AGENTS_TOPUP_ROWS (1024),
// AGENTS_MIN_FREE_ROWS (256), AGENTS_TICK_MS (20000), AGENTS_KILL_FILE, AGENTS_HALT_CONTRACT
// (the protocol halt contract; default deploy/halt-devnet.json; "off" = none).
import { Runtime, type RuntimeConfig } from "./runtime.ts";
import { haltContractFrom } from "./halt.ts";

const env = process.env;
const num = (k: string, d: number) => (env[k] === undefined || env[k] === "" ? d : Number(env[k]));

export function configFromEnv(): RuntimeConfig {
  const state = env.AGENTS_STATE ?? "/var/lib/lode/agents";
  const api = env.CADRE_API ?? "http://127.0.0.1:10685";
  return {
    stateDir: state,
    masterKeyFile: env.AGENTS_MASTER_KEY ?? "/etc/lode/keys/agents/master.hex",
    nodeRpc: env.CHAIN_RPC ?? "http://127.0.0.1:10630",
    gatewayUrl: env.GATEWAY_URL ?? `${api}/v1/openai`,
    apiUrl: api,
    publicApiUrl: env.CADRE_PUBLIC_API ?? api,
    solRpc: env.SOLANA_RPC,
    network: env.CADRE_NETWORK ?? "localnet",
    allowHosts: (env.AGENTS_ALLOW_HOSTS ?? "64-95-12-116.sslip.io").split(",").map((s) => s.trim()).filter(Boolean),
    maxPerOperator: num("AGENTS_MAX_PER_OPERATOR", 5),
    maxAgents: num("AGENTS_MAX", 200),
    topupRows: num("AGENTS_TOPUP_ROWS", 1024),
    minFreeRows: num("AGENTS_MIN_FREE_ROWS", 256),
    tickMs: num("AGENTS_TICK_MS", 20_000),
    killFile: env.AGENTS_KILL_FILE ?? `${state}/STOP`,
    haltContract: haltContractFrom(env),
  };
}

async function main() {
  const cmd = process.argv[2] ?? "run";
  const rt = new Runtime(configFromEnv());
  if (cmd === "info") {
    console.log(JSON.stringify(rt.info(), null, 2));
    return;
  }
  if (cmd !== "run") throw new Error("usage: run | info");
  const port = num("AGENTS_PORT", 10730);
  rt.listen(port, env.AGENTS_HOST ?? "127.0.0.1");
  const stop = () => { rt.stopped = true; };
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
  console.log(JSON.stringify({ event: "start", port, agents: rt.entries.size, network: rt.cfg.network }));
  await rt.run();
  console.log(JSON.stringify({ event: "stop" }));
  process.exit(0);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error(JSON.stringify({ event: "fatal", error: String(e?.message ?? e) }));
    process.exit(1);
  });
}
