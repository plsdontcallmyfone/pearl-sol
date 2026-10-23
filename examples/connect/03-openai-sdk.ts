// Example 3 (DEVNET): an agent built on the official OpenAI SDK switches to Cadre by changing its
// base_url and api key, gets a Cadre receipt with every completion (headers and the `cadre` field),
// and verifies it.
//   cd examples/connect && npm install && node 03-openai-sdk.ts
// Two ways to authenticate, both prepaid by the agent's own wallet:
//   default             a cadre_sk_ API key minted by the agent's wallet; base_url <api>/v1/openai (live on devnet)
//   CADRE_AGENT_TOKEN   a hosted agent's token; base_url <api>/v1 (cadre-api with Cadre Connect)
import OpenAI from "openai";
import { Cadre } from "../../packages/cadre-sdk/src/index.ts";
import { NETWORKS } from "../../packages/cadre-sdk/src/index.ts";
import { API, fund, log, throwawayKey } from "./_common.ts";

let baseURL: string, apiKey: string;
let wallet: Cadre | null = null;
if (process.env.CADRE_AGENT_TOKEN) {
  baseURL = `${API}/v1`;
  apiKey = process.env.CADRE_AGENT_TOKEN;
  log("mode", { auth: "agent token", baseURL });
} else {
  const k = throwawayKey("openai-03");
  wallet = new Cadre({ key: k.file, api: API, limits: { maxSolPerTx: 30_000_000n, maxSolPerDay: 60_000_000n, maxCadrePerTx: 0n, maxCadrePerDay: 0n, maxInferenceLamportsPerDay: 60_000_000n } });
  log("agent wallet", { address: wallet.address });
  log("funded from ops", await fund(wallet.address, 40_000_000n, 0n));
  const b = await wallet.inferenceBalance().catch(() => ({ rows_free: 0 }));
  if (Number(b.rows_free ?? 0) < 64) log("prepaid top-up", await wallet.topup(256));
  const key = await wallet.createApiKey(); // shown once; kept in memory only, never printed
  baseURL = `${API}/v1/openai`;
  apiKey = key.api_key;
  log("mode", { auth: "cadre_sk_ API key", key_id: key.key_id, baseURL });
}

// the only change an existing OpenAI agent makes: baseURL + apiKey
const client = new OpenAI({ baseURL, apiKey });
const models = await client.models.list();
log("models", models.data.map((m: any) => ({ id: m.id, quantization: m.cadre?.quantization })));
const { data, response } = await client.chat.completions
  .create({ model: models.data[0].id, messages: [{ role: "user", content: "What is the capital of France? Answer in one word." }], max_tokens: 8 })
  .withResponse();
const text = data.choices[0].message.content ?? "";
const receipt = (data as any).cadre;
log("completion", { text, usage: data.usage, header_request: response.headers.get("x-cadre-request"), header_ticket: response.headers.get("x-cadre-ticket"), header_charged_lamports: response.headers.get("x-cadre-charged-lamports"), receipt_field: receipt });

// verify: anyone can, with the public API only
const checker = wallet ?? new Cadre({ key: Uint8Array.from(Array(32).fill(1)), api: process.env.VERIFY_API ?? NETWORKS.devnet.api });
const v = await checker.verifyReceipt(receipt?.request_id ?? response.headers.get("x-cadre-request")!, text);
log("receipt checks", v.checks);
if (!v.checks.every((c) => c.ok !== false)) throw new Error("a receipt check failed");
