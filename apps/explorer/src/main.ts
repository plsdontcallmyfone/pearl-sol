// Explorer entry point. Env:
//   EXPLORER_PORT   10670 (owned block 10670-10679)
//   EXPLORER_HOST   127.0.0.1
//   NODE_RPC        http://127.0.0.1:10630   agent-chain node JSON-RPC
//   SOLANA_RPC      http://127.0.0.1:10600   Solana RPC (the token chain)
//   STATE_DIR       apps/explorer/.state     persisted index (one file per genesis)
//   POLL_MS         3000

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Indexer } from "./indexer.ts";
import { serve } from "./server.ts";
import { BRAND } from "./brand.ts";
import { redactUrl } from "./rpc.ts";

const here = dirname(fileURLToPath(import.meta.url));
const env = process.env;
const port = Number(env.EXPLORER_PORT ?? 10670);
const host = env.EXPLORER_HOST ?? "127.0.0.1";
const ix = new Indexer({
  nodeUrl: env.NODE_RPC ?? "http://127.0.0.1:10630",
  solUrl: env.SOLANA_RPC ?? "http://127.0.0.1:10600",
  stateDir: env.STATE_DIR ?? join(here, "..", ".state"),
  pollMs: Number(env.POLL_MS ?? 3000),
  solPollMs: Number(env.SOL_POLL_MS ?? 0),
});
ix.start();
serve(ix, port, host);
console.log(`${BRAND.name} explorer on http://${host}:${port} (node ${ix.o.nodeUrl}, solana ${redactUrl(ix.o.solUrl)})`);
const stop = () => {
  ix.stop();
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
