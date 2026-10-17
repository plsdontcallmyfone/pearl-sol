// Relayer daemon: polls Solana and the agent-chain node, relays, serves /health on HEALTH_PORT.

import { RpcChain } from "./chain.ts";
import { loadConfig, loadKeypair } from "./config.ts";
import { startHealth } from "./health.ts";
import { NodeRpc } from "./node.ts";
import { Relayer } from "./relayer.ts";

const cfg = loadConfig();
const relayer = new Relayer({
  token: new RpcChain("token", cfg.tokenRpc, cfg.commitment),
  node: NodeRpc.http(cfg.nodeRpc),
  signers: cfg.signerKeyFiles.map(loadKeypair),
  tokenPayer: loadKeypair(cfg.tokenPayerFile),
  agentKey: loadKeypair(cfg.agentKeyFile),
  storefrontProgram: cfg.storefrontProgram,
  stateFile: cfg.stateFile,
  totalSupply: cfg.totalSupply,
  allowSurplus: cfg.allowSurplus,
});

const server = startHealth(cfg.healthPort, () => relayer.health(cfg.pollMs));
server.on("error", (e) => {
  console.error(`health server on :${cfg.healthPort} failed: ${e.message}`);
  process.exit(1);
});
console.log(JSON.stringify({
  msg: "relayer started",
  tokenRpc: cfg.tokenRpc,
  nodeRpc: cfg.nodeRpc,
  signers: relayer.o.signers.map((k) => k.publicKey.toBase58()),
  agentKey: relayer.o.agentKey.publicKey.toBase58(),
  health: `http://127.0.0.1:${cfg.healthPort}/health`,
  stateFile: cfg.stateFile,
}));

let stopping = false;
let timer: NodeJS.Timeout | undefined;
async function loop(): Promise<void> {
  if (stopping) return;
  const r = await relayer.tick();
  if (r.unlocked.length || r.released.length || r.finalized.length || r.expired.length || r.refused || r.errors.length) {
    console.log(JSON.stringify({ t: r.at, height: r.chainHeight, unlocked: r.unlocked, released: r.released,
      finalized: r.finalized, expired: r.expired, waitingDepth: r.waitingDepth, needsRefund: r.needsRefund, refused: r.refused, errors: r.errors }));
  }
  timer = setTimeout(loop, cfg.pollMs);
}
for (const sig of ["SIGINT", "SIGTERM"] as const) {
  process.on(sig, () => {
    stopping = true;
    if (timer) clearTimeout(timer);
    server.close(() => process.exit(0));
  });
}
void loop();
