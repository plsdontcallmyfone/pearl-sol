// RH relayer daemon (RELAYER_SIDE=rh, env in ./config.ts): polls the Cadre chain node and
// Robinhood Chain, relays both ways, serves /health on HEALTH_PORT (default 10666).

import { loadKeypair } from "../config.ts";
import { startHealth } from "../health.ts";
import { loadEvmConfig, loadEvmKey } from "./config.ts";
import { ViemEvm } from "./io.ts";
import { NodeV2 } from "./node-v2.ts";
import { EvmRelayer } from "./relayer.ts";

const cfg = loadEvmConfig();
const signer = loadEvmKey(cfg.signerKeyFile);
const evm = new ViemEvm({ rpc: cfg.rpc, bridge: cfg.bridge, storefront: cfg.storefront ?? undefined, oracle: cfg.oracle ?? undefined,
  signer, finality: cfg.finality });
const chainId = await evm.chainId();
if (chainId !== cfg.chainId) {
  console.error(`RH_RPC answers chain id ${chainId}, RH_CHAIN_ID is ${cfg.chainId}; refusing to start`);
  process.exit(2);
}
if (!cfg.oracle) {
  // proof mode reads the bridge's header source
  const b = await evm.bridge((await evm.blocks()).latest);
  if (!/^0x0+$/.test(b.headers)) evm.o.oracle = b.headers;
}
const relayer = new EvmRelayer({
  node: NodeV2.http(cfg.nodeRpc),
  evm,
  domain: cfg.domain,
  bridge: cfg.bridge,
  storefront: cfg.storefront,
  chainSigners: cfg.chainSignerFiles.map(loadKeypair),
  agentKey: loadKeypair(cfg.agentKeyFile),
  attesters: cfg.attesterKeyFiles.map(loadEvmKey),
  stateFile: cfg.stateFile,
  expectedChainId: cfg.chainId,
  releaseMode: cfg.releaseMode,
  totalSupply: cfg.totalSupply,
  allowSurplus: cfg.allowSurplus,
  refundGraceMs: cfg.refundGraceMs,
});

const server = startHealth(cfg.healthPort, () => relayer.health(cfg.pollMs));
server.on("error", (e) => {
  console.error(`health server on :${cfg.healthPort} failed: ${e.message}`);
  process.exit(1);
});
const host = (u: string) => { try { const x = new URL(u); return `${x.protocol}//${x.host}`; } catch { return "(rpc url)"; } };
console.log(JSON.stringify({
  msg: "relayer started", side: "rh", domain: cfg.domain, rhRpc: host(cfg.rpc), chainId: chainId.toString(), nodeRpc: cfg.nodeRpc,
  bridge: cfg.bridge, storefront: cfg.storefront, oracle: evm.o.oracle ?? null, finality: cfg.finality, releaseMode: cfg.releaseMode,
  sender: signer.address, attesters: relayer.o.attesters.map((k) => k.address),
  chainSigners: relayer.o.chainSigners.map((k) => k.publicKey.toBase58()), agentKey: relayer.o.agentKey.publicKey.toBase58(),
  health: `http://127.0.0.1:${cfg.healthPort}/health`, stateFile: cfg.stateFile,
}));

let stopping = false;
let timer: NodeJS.Timeout | undefined;
async function loop(): Promise<void> {
  if (stopping) return;
  const r = await relayer.tick();
  if (r.unlocked.length || r.released.length || r.finalized.length || r.expired.length || r.refunded.length || r.refused || r.errors.length) {
    console.log(JSON.stringify({ t: r.at, side: "rh", height: r.chainHeight, rhFinal: r.rhFinalBlock, unlocked: r.unlocked, released: r.released,
      viaProof: r.releasedViaProof, finalized: r.finalized, expired: r.expired, refunded: r.refunded, waitingDepth: r.waitingDepth,
      waitingOracle: r.waitingOracle, needsRefund: r.needsRefund, refused: r.refused, errors: r.errors }));
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
