// Dev only: cadre-api with every route but WITHOUT starting the explorer indexer, so a local copy
// can serve the Cadre Connect routes (MCP, OAuth, agent tokens, OpenAI aliases) against the public
// devnet without backfilling the whole chain from node 0. Reads of other data go to CADRE_SELF_URL.
//   CADRE_AGENTS_URL=http://127.0.0.1:10738 CADRE_SELF_URL=https://api.64-95-12-116.sslip.io \
//   CADRE_API_PORT=10739 node examples/connect/dev/api-no-index.ts
import { join } from "node:path";
import { configFromEnv, Deps } from "../../../services/cadre-api/src/deps.ts";
import { build } from "../../../services/cadre-api/src/app.ts";
import { OUT } from "../_common.ts";

const cfg = configFromEnv(process.env, { stateDir: join(OUT, "api-state") });
const d = new Deps(cfg);
const srv = build(d).listen(cfg.port, cfg.host);
srv.on("listening", () => console.log(JSON.stringify({ msg: "cadre-api (connect dev, no indexer)", url: `http://${cfg.host}:${cfg.port}`, agents: process.env.CADRE_AGENTS_URL, self: process.env.CADRE_SELF_URL })));
