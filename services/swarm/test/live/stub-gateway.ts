// Stub inference gateway for the DM live check (not a test file; `node test/live/stub-gateway.ts`):
// the cadre-auth-v1 signature is checked like the real gateway, completions are deterministic
// (a fixed list picked by sha256 of the prompt), and every reply has the real receipt shape
// (choices, usage, cadre {request_id, ticket_id, rows, charged_lamports, model_id}, x-cadre-request /
// x-cadre-rows / x-cadre-ticket headers, GET /v1/requests/{id} with text_sha256). /v1/models has no
// price (lamports_per_row null): the stub is not a market, so the live run uses SWARM_DM_PRICE.
// Receipts are appended to $STUB_RECEIPTS (jsonl) for the live script to check.
import { createHash, randomBytes } from "node:crypto";
import { appendFileSync } from "node:fs";
import { createServer } from "node:http";
import { b58decode } from "../../src/b58.ts";
import { authMessage } from "../../src/clients.ts";
import { verifySig } from "../../src/wallet.ts";

const port = Number(process.env.STUB_PORT ?? 11760);
const file = process.env.STUB_RECEIPTS ?? "receipts.jsonl";
const LINES = [
  "The harbour is quiet tonight and the lamps are lit.",
  "I counted three boats at dawn and one very loud gull.",
  "Ask me again after the next block; the fog is thick.",
  "The miners worked late and the bread is still warm.",
];
const receipts = new Map<string, any>();
const sha = (s: string | Buffer) => createHash("sha256").update(s).digest("hex");

createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const path = (req.url ?? "/").split("?")[0];
    const send = (status: number, b: any, h: Record<string, string> = {}) => { res.writeHead(status, { "content-type": "application/json", ...h }); res.end(JSON.stringify(b)); };
    const authed = () => {
      const w = req.headers["x-cadre-wallet"] as string, ts = Number(req.headers["x-cadre-timestamp"]), sig = req.headers["x-cadre-signature"] as string;
      return !!w && !!sig && Math.abs(Date.now() - ts) < 120_000 && verifySig(b58decode(w), authMessage(req.method!, path, ts, body), Buffer.from(sig, "hex"));
    };
    if (path === "/v1/models") return send(200, { object: "list", data: [{ id: "stub-1b", object: "model", cadre: { model_id: "00".repeat(32), pricing: { lamports_per_row: null, basis: "stub gateway: no price" } } }] });
    if (path === "/v1/balance") return authed() ? send(200, { wallet: req.headers["x-cadre-wallet"], rows_free: 1_000_000, tickets: [] }) : send(401, { error: { message: "bad signature" } });
    if (path.startsWith("/v1/requests/")) { const r = receipts.get(path.slice(13)); return r ? send(200, r) : send(404, { error: "unknown request" }); }
    if (path === "/v1/chat/completions" && req.method === "POST") {
      if (!authed()) return send(401, { error: { message: "bad signature" } });
      const b = JSON.parse(body);
      const last = b.messages.at(-1).content as string;
      const text = /Answer yes or no/.test(last) ? "No" : LINES[parseInt(sha(JSON.stringify(b.messages)).slice(0, 8), 16) % LINES.length];
      const promptTokens = Math.ceil(b.messages.reduce((n: number, m: any) => n + Buffer.byteLength(m.content), 0) / 4) + 30;
      const completion = Math.min(b.max_tokens ?? 48, Math.ceil(Buffer.byteLength(text) / 4));
      const id = `req_${randomBytes(12).toString("hex")}`;
      const ticket = "stub-ticket";
      const receipt = { request_id: id, ticket_id: ticket, wallet: req.headers["x-cadre-wallet"], rows: promptTokens + completion, charged_lamports: null, text_sha256: sha(text), status: "completed", created_ms: Date.now() };
      receipts.set(id, receipt);
      appendFileSync(file, JSON.stringify(receipt) + "\n");
      return send(200, {
        id: `chatcmpl-${id}`, object: "chat.completion", created: Math.floor(Date.now() / 1000), model: "stub-1b",
        choices: [{ index: 0, message: { role: "assistant", content: text }, finish_reason: "stop" }],
        usage: { prompt_tokens: promptTokens, completion_tokens: completion, total_tokens: promptTokens + completion },
        cadre: { request_id: id, ticket_id: ticket, rows: promptTokens + completion, charged_lamports: null, model_id: "00".repeat(32) },
      }, { "x-cadre-request": id, "x-cadre-rows": String(promptTokens + completion), "x-cadre-ticket": ticket });
    }
    send(404, { error: "not found" });
  });
}).listen(port, "127.0.0.1", () => console.log(JSON.stringify({ event: "stub_gateway", port, pid: process.pid })));
