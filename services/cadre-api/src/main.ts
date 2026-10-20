// cadre-api entry point: node src/main.ts. Env (defaults = the local demo stack):
//   CADRE_API_PORT 10685 (owned block 10685-10689), CADRE_API_HOST 127.0.0.1
//   NODE_RPC http://127.0.0.1:10630, SOLANA_RPC http://127.0.0.1:10600
//   GATEWAY_URL http://127.0.0.1:10664, STOREFRONT_URL http://127.0.0.1:10662, POOL_URL http://127.0.0.1:10663 ("off" disables one)
//   STATE_DIR services/cadre-api/.state, POLL_MS 3000
//   CADRE_ALLOWED_ORIGINS * (or a comma list), CADRE_RATE_LIMIT_PER_MIN 300, CADRE_RATE_BURST 120, CADRE_TRUST_PROXY 1 behind caddy
//   CADRE_PREPAID off|gateway, CADRE_NETWORK localnet, SOLANA_CLUSTER localnet, EXPLORER_PUBLIC_URL, CADRE_PUBLIC_URL
//   STOREFRONT_PROGRAM / BRIDGE_PROGRAM / LIGHTCLIENT_PROGRAM / RESERVE_PROGRAM: program ids (devnet)

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { configFromEnv, Deps } from "./deps.ts";
import { build, VERSION } from "./app.ts";
import { redactUrl } from "../../../apps/explorer/src/rpc.ts";

const here = dirname(fileURLToPath(import.meta.url));
const cfg = configFromEnv(process.env, { stateDir: join(here, "..", ".state") });
const d = new Deps(cfg);
d.ix.start();
const api = build(d);
const srv = api.listen(cfg.port, cfg.host);
srv.on("error", (e) => {
  console.error(`cadre-api on ${cfg.host}:${cfg.port} failed: ${e.message} (lsof -ti :${cfg.port})`);
  process.exit(1);
});
srv.on("listening", () =>
  console.log(JSON.stringify({ msg: "cadre-api started", version: VERSION, url: `http://${cfg.host}:${cfg.port}/v1`, node: cfg.nodeRpc, solana: redactUrl(cfg.solanaRpc), gateway: cfg.gatewayUrl, storefront: cfg.storefrontUrl, pool: cfg.poolUrl, prepaid: cfg.prepaid, origins: cfg.allowedOrigins, pid: process.pid })),
);
const stop = () => {
  d.ix.stop();
  srv.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 3000).unref();
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
