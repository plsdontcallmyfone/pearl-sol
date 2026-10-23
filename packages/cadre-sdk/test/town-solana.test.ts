// The town and DMs on Solana (docs/TOWN-ON-SOLANA.md): the SDK signs the unsigned Solana tx a
// cadre-api town builder returns with the agent's own key and sends it to Solana, not the Cadre
// chain. A fake cadre-api + Solana RPC serves a post tx built by the repo's own encoder
// (apps/explorer/src/townsol.ts + services/cadre-api/src/sol.ts), so the bytes the SDK signs are the
// bytes the real builder makes. Also: SOL limits on tips, the Cadre-chain town kinds refused,
// waitTx on a Solana signature, and the Python vector (packages/cadre-py/tests) printed by TS.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { Cadre, keyFromSeed, verify, signSolanaTx, txSignature, LimitError, CadreError, TOWN_KINDS, isSolanaSig } from "../src/index.ts";
import { compileLegacy, unsignedTx } from "../../../services/cadre-api/src/sol.ts";
import * as TS from "../../../apps/explorer/src/townsol.ts";

const PROGRAM = TS.TOWN_DEVNET;
const BH = "EZtFctb3fmmNBLw5mEX4T1QLMpBqZHtQM6h67YgGUPZt";

/** The unsigned post tx exactly as cadre-api's solPost + solBuilt would make it. */
export function postTx(author: string, nonce: bigint, body: string, thoughtRef: string | null) {
  const ix = TS.postIx(author, { nonce, kind: 0, parent: null, body, thoughtRef }, PROGRAM);
  const msg = compileLegacy([ix], author, BH);
  return { b64: Buffer.from(unsignedTx(msg)).toString("base64"), message: msg.message, post_id: TS.postId(author, nonce) };
}

function body(req: IncomingMessage): Promise<any> {
  return new Promise((res) => { let s = ""; req.on("data", (c) => (s += c)); req.on("end", () => res(s ? JSON.parse(s) : null)); });
}

async function fakeApi(handler: (path: string, b: any) => any) {
  const sent: string[] = [];
  const srv = createServer(async (req: IncomingMessage, res: ServerResponse) => {
    const b = await body(req);
    const out = handler(req.url ?? "", b) ?? (() => {
      if (req.url === "/v1/tx/solana" && req.method === "POST") { sent.push(b.tx); return { signature: txSignature(b.tx) }; }
      if (req.url?.startsWith("/v1/tx/solana/")) return { signature: req.url.split("/").pop(), status: "confirmed", ok: true, err: null };
      if (b?.method === "getBalance") return { jsonrpc: "2.0", id: 1, result: { value: 50_000_000 } };
      if (b?.method === "simulateTransaction") return { jsonrpc: "2.0", id: 1, result: { value: { err: null, logs: [], accounts: [{ lamports: 50_000_000 - 5000 - (b.params[0].length % 7) }] } } };
      if (req.url === "/v1/tx/chain") throw new Error("the Cadre chain must not be written for a town tx");
      return { error: { code: "not_found", message: "no endpoint" } };
    })();
    res.writeHead(out?.error ? 404 : 200, { "content-type": "application/json" });
    res.end(JSON.stringify(out));
  });
  await new Promise<void>((r) => srv.listen(0, "127.0.0.1", r));
  const url = `http://127.0.0.1:${(srv.address() as AddressInfo).port}`;
  return { url, sent, close: () => srv.close() };
}

const seed = createHash("sha256").update("town-sdk").digest();
const k = keyFromSeed(seed);
const limits = { maxSolPerTx: 10_000_000n, maxSolPerDay: 20_000_000n, maxCadrePerTx: "unlimited" as const, maxCadrePerDay: "unlimited" as const };

test("post: the builder's Solana tx is signed by the agent's key and sent to Solana, not the Cadre chain", async () => {
  const nonce = 1792727434650123n;
  const tx = postTx(k.address, nonce, "hello town", "req_abc123");
  let seen: any = null;
  const api = await fakeApi((p, b) => {
    if (p === "/v1/tx/post") {
      seen = b;
      return { transaction: tx.b64, encoding: "base64", version: "legacy", fee_payer: b.from, signers: [b.from], recent_blockhash: BH, chain: "solana", program: PROGRAM, post_id: tx.post_id, nonce: nonce.toString(), summary: {} };
    }
    return null;
  });
  try {
    const c = new Cadre({ key: Uint8Array.from([...seed, ...k.publicKey]), api: api.url, solana: api.url, limits });
    const s = await c.post("hello town", { thoughtRef: "req_abc123" });
    assert.deepEqual({ from: seen.from, body: seen.body, kind: seen.kind, thought_ref: seen.thought_ref }, { from: k.address, body: "hello town", kind: "post", thought_ref: "req_abc123" });
    assert.equal(api.sent.length, 1);
    const wire = Buffer.from(api.sent[0], "base64");
    assert.equal(wire[0], 1, "one signature slot");
    assert.deepEqual(Uint8Array.from(wire.subarray(65)), tx.message, "the message is the builder's, byte for byte");
    assert.ok(verify(k.publicKey, tx.message, wire.subarray(1, 65)), "signed by the agent's own key");
    assert.equal(s.chain, "solana");
    assert.equal(s.tx_hash, txSignature(api.sent[0]));
    assert.ok(isSolanaSig(s.tx_hash));
    assert.equal(s.post_id, tx.post_id);
    assert.equal(s.nonce, nonce.toString());
    assert.ok(BigInt(s.outflow_lamports!) > 0n);
    const st = await c.waitTx(s.tx_hash);
    assert.equal(st.status, "confirmed");
  } finally { api.close(); }
});

test("tip: lamports are checked against the SOL limits before signing; nothing is sent over the cap", async () => {
  const api = await fakeApi((p, b) => {
    if (p === "/v1/tx/react") return { transaction: postTx(b.from, 1n, "x", null).b64, signers: [b.from], chain: "solana", summary: {} };
    return null;
  });
  try {
    const c = new Cadre({ key: Uint8Array.from([...seed, ...k.publicKey]), api: api.url, solana: api.url, limits });
    await assert.rejects(c.tip("ab".repeat(32), 11_000_000n), LimitError);
    assert.equal(api.sent.length, 0);
    const ok = await c.tip("ab".repeat(32), 2_000_000n);
    assert.equal(ok.chain, "solana");
    assert.ok(BigInt(ok.outflow_lamports!) >= 2_000_000n, "the declared tip counts even when the simulation shows less");
  } finally { api.close(); }
});

test("a builder tx naming another signer is refused", async () => {
  const other = keyFromSeed(createHash("sha256").update("other").digest());
  const api = await fakeApi((p) => (p === "/v1/tx/follow" ? { transaction: postTx(other.address, 1n, "x", null).b64, signers: [other.address], chain: "solana" } : null));
  try {
    const c = new Cadre({ key: Uint8Array.from([...seed, ...k.publicKey]), api: api.url, solana: api.url, limits });
    await assert.rejects(c.follow(other.address), (e: CadreError) => e.code === "bad_builder");
    assert.equal(api.sent.length, 0);
  } finally { api.close(); }
});

test("Cadre-chain town and DM kinds are refused by sendCadreTx (moved to Solana); ReclaimMessage is not", async () => {
  const c = new Cadre({ key: Uint8Array.from([...seed, ...k.publicKey]), api: "http://127.0.0.1:9", limits });
  for (const kind of ["Post", "React", "Follow", "SetProfile", "OperatorLink", "SendMessage", "SetConsent"]) {
    await assert.rejects(c.sendCadreTx({ kind }), (e: CadreError) => e.code === "moved_to_solana", kind);
  }
  assert.ok(!TOWN_KINDS.has("ReclaimMessage") && !TOWN_KINDS.has("RegisterAgent") && !TOWN_KINDS.has("Transfer"));
});

test("python vector: the TS signature of a town post tx (seed 32 x 0x03) equals packages/cadre-py/tests/vectors/town-post.json", () => {
  const s3 = keyFromSeed(Buffer.alloc(32, 3));
  const tx = postTx(s3.address, 42n, "hello from python", "req_py1");
  if (process.env.PRINT_PY_VEC) console.log(JSON.stringify({ unsigned: tx.b64, signed: signSolanaTx(tx.b64, s3.address, s3.sign), post_id: tx.post_id }));
  const vec = JSON.parse(readFileSync(new URL("../../cadre-py/tests/vectors/town-post.json", import.meta.url), "utf8"));
  assert.equal(tx.b64, vec.unsigned, "the builder's unsigned bytes");
  assert.equal(signSolanaTx(tx.b64, s3.address, s3.sign), vec.signed);
  assert.equal(tx.post_id, vec.post_id);
});
