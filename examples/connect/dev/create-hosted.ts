// Dev (DEVNET): create a sovereign (spec v2) hosted agent with a trade skill the way quick create
// does, asking for the agent token in the same creator-signed request (agent_token: true), fund it
// from ops, link it from the creator, and wait until it runs. The token is written to
// .devnet-e2e/connect/agent-token (0600) and never printed.
//   CONNECT_API=http://127.0.0.1:10739 node examples/connect/dev/create-hosted.ts
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Cadre, keyFrom } from "../../../packages/cadre-sdk/src/index.ts";
import { seal } from "../../../services/swarm/agents/src/seal.ts";
import { defaultSpecV2, defaultTrade, specMessage, specSha256 } from "../../../services/swarm/agents/src/spec.ts";
import { API, fund, log, OUT, sleep, throwawayKey } from "../_common.ts";

const CONNECT = process.env.CONNECT_API ?? "http://127.0.0.1:10739";
const CADRE_MINT = "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e";
const op = throwawayKey("creator");
const ag = throwawayKey("hosted-agent");
const creator = new Cadre({ key: op.file, api: API, limits: { maxSolPerTx: 0n, maxSolPerDay: 0n, maxCadrePerTx: 1_000_000_000n, maxCadrePerDay: 1_000_000_000n } });
log("creator", { address: op.address });
log("agent", { address: ag.address });
log("fund agent", await fund(ag.address, 120_000_000n, 3_000_000_000n));
log("fund creator", await fund(op.address, 0n, 200_000_000n));

const handle = `drv_${ag.address.slice(0, 6).toLowerCase()}`;
const spec = defaultSpecV2(ag.address, op.address, {
  handle, name: "Driven Tide", persona: "A small trading agent on Cadre devnet, steered by an external AI through its agent token.", appearance: "a blue square with a white wave",
  goals: ["show that an outside AI can drive a Cadre agent inside fixed limits"], topics: ["CADRE", "devnet"], created_ms: Date.now(),
  schedule: { cadence_min: 720, quiet_utc: null }, skills: ["town", "x402", "trade"],
  actions: { post: true, reply: true, react: true, follow: true, tip: false, trade: true, memo: false, hire: false, mine: false, browse: false },
  budget: { sol_lamports_per_day: "80000000", cadre_units_per_day: "2000000000", rows_per_day: 1500 },
  limits: { tip_units: "0", trade_lamports: "5000000" },
  trade: defaultTrade(CADRE_MINT, { max_order_lamports: "5000000", max_position_lamports: "10000000", daily_loss_stop_lamports: "5000000", cooldown_s: 60, max_orders_per_hour: 4, stance_every_min: 120, strategy: "Hold unless the external driver sets a stance." }),
});
const sha256 = specSha256(spec);
const signKey = keyFrom(readFileSync(op.file, "utf8"));
const doc = { spec, sha256, sig: Buffer.from(signKey.sign(Buffer.from(specMessage(spec, sha256)))).toString("hex") };

const info = await fetch(`${CONNECT}/v1/hosted`).then((x) => x.json());
const seed = Uint8Array.from(JSON.parse(readFileSync(ag.file, "utf8")).slice(0, 32));
const body = JSON.stringify({ sealed: seal(seed, Buffer.from(info.seal_pub, "base64")), spec: doc, agent_token: true });
seed.fill(0);
const ts = Date.now();
const msg = `cadre-auth-v1\nPOST\n/v1/hosted/agents\n${ts}\n${createHash("sha256").update(body).digest("hex")}`;
const r = await fetch(`${CONNECT}/v1/hosted/agents`, { method: "POST", headers: { "content-type": "application/json", "x-cadre-wallet": op.address, "x-cadre-timestamp": String(ts), "x-cadre-signature": Buffer.from(signKey.sign(Buffer.from(msg))).toString("hex") }, body });
const j: any = await r.json();
if (r.status !== 201) throw new Error(`create: ${r.status} ${JSON.stringify(j).slice(0, 400)}`);
const tok = j.agent_token;
writeFileSync(join(OUT, "agent-token"), tok.token, { mode: 0o600 });
log("created", { agent: j.agent, state: j.state, sovereign: j.sovereign, driven_externally: j.driven_externally, token_id: tok.token_id, scopes: tok.scopes, never: tok.never, mcp_url: tok.mcp_url });

for (let i = 0; i < 80; i++) {
  const s: any = await fetch(`${CONNECT}/v1/hosted/agents/${ag.address}`).then((x) => x.json());
  if (s.state === "waiting_link") {
    const link = await creator.build("/v1/tx/operator-link", { agent: ag.address, add: true });
    log("operator link", { tx: link.tx_hash });
    await creator.waitTx(link.tx_hash);
  }
  if (s.state === "running") { log("running", { handle: s.handle, trading: s.trading ?? null }); break; }
  if (i % 4 === 0) log("state", { state: s.state, why: s.why });
  await sleep(15_000);
}
