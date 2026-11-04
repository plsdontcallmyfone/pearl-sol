// RH header-oracle poster daemon (`npm run oracle`): posts attester-signed final Cadre
// headers to CadreHeaderOracle every ORACLE_EVERY final blocks, and early when a final
// domain withdrawal waits above the oracle's latest header. Env:
//
//   RH_RPC            EVM JSON-RPC                                  required
//   RH_CHAIN_ID       expected eth_chainId                          default 46630
//   RH_HEADER_ORACLE  CadreHeaderOracle address                     required
//   RH_ORACLE_SENDER_KEY  secp256k1 key file that sends the posts (pays gas). Use a key the
//                     relayer does not send from (two processes on one EOA race for nonces)
//                     default RH_SIGNER_KEY
//   RH_ATTESTER_KEYS  comma-separated secp256k1 key files; those in the oracle's attester set
//                     sign FinalHeader (EIP-712)                     default RH_SIGNER_KEY
//   ORACLE_EVERY      post every N final blocks (decision E9)       default 10
//   RH_DOMAIN         bridge domain whose waiting withdrawals trigger an early post; "off" = cadence only  default 2
//   RH_BRIDGE         CadreBridge, to skip withdrawals already released  optional
//   NODE_RPC          Cadre chain node JSON-RPC                     default http://127.0.0.1:10630
//   POLL_MS           tick interval                                 default 5000
//   HEALTH_PORT       GET /health                                    default 10669

import { getAddress } from "viem";
import { startHealth } from "../health.ts";
import { loadEvmKey } from "./config.ts";
import { ViemEvm } from "./io.ts";
import { NodeV2 } from "./node-v2.ts";
import { OraclePoster } from "./oracle.ts";

const env = process.env;
const need = (k: string): string => {
  const v = env[k]?.trim();
  if (!v) {
    console.error(`${k} is required`);
    process.exit(2);
  }
  return v;
};
const senderFile = env.RH_ORACLE_SENDER_KEY?.trim() || need("RH_SIGNER_KEY");
const signer = loadEvmKey(senderFile);
const oracle = getAddress(need("RH_HEADER_ORACLE"));
const bridge = env.RH_BRIDGE ? getAddress(env.RH_BRIDGE) : undefined;
const attesterFiles = (env.RH_ATTESTER_KEYS ?? "").split(",").map((s) => s.trim()).filter(Boolean);
const pollMs = Number(env.POLL_MS ?? 5000);
const healthPort = Number(env.HEALTH_PORT ?? 10669);
const domain = env.RH_DOMAIN === "off" ? null : Number(env.RH_DOMAIN ?? 2);
const evm = new ViemEvm({ rpc: need("RH_RPC"), bridge: bridge ?? oracle, oracle, signer, finality: "latest" });
const chainId = await evm.chainId();
if (chainId !== BigInt(env.RH_CHAIN_ID ?? 46630)) {
  console.error(`RH_RPC answers chain id ${chainId}, RH_CHAIN_ID is ${env.RH_CHAIN_ID ?? 46630}; refusing to start`);
  process.exit(2);
}
const poster = new OraclePoster({
  node: NodeV2.http(env.NODE_RPC ?? "http://127.0.0.1:10630"),
  evm,
  oracle,
  attesters: (attesterFiles.length ? attesterFiles : [senderFile]).map(loadEvmKey),
  every: Number(env.ORACLE_EVERY ?? 10),
  domain,
  isProcessed: bridge ? (n) => evm.isProcessed(n, "latest") : undefined,
});
const server = startHealth(healthPort, () => poster.health(pollMs));
server.on("error", (e) => {
  console.error(`health server on :${healthPort} failed: ${e.message}`);
  process.exit(1);
});
console.log(JSON.stringify({ msg: "rh header oracle poster started", oracle, every: poster.o.every, domain, sender: signer.address,
  attesters: poster.o.attesters.map((k) => k.address), chainId: chainId.toString(), health: `http://127.0.0.1:${healthPort}/health` }));

let stopping = false;
let timer: NodeJS.Timeout | undefined;
async function loop(): Promise<void> {
  if (stopping) return;
  const r = await poster.tick();
  if (r.posted || r.errors.length) console.log(JSON.stringify(r));
  timer = setTimeout(loop, pollMs);
}
for (const sig of ["SIGINT", "SIGTERM"] as const) {
  process.on(sig, () => {
    stopping = true;
    if (timer) clearTimeout(timer);
    server.close(() => process.exit(0));
  });
}
void loop();
