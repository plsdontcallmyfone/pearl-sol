// RpcChain never waits forever, and src/reconcile.ts always exits (devnet 2026-10-23: the tool
// hung after its refunds on web3.js's confirmation websocket while the storefront stayed stopped).
// A fake JSON-RPC server on an ephemeral port; no validator, no fixed ports.

import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { Server } from "node:http";
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Keypair, SystemProgram } from "@solana/web3.js";
import { RpcChain } from "../src/chain.ts";

const here = dirname(fileURLToPath(import.meta.url));

/** JSON-RPC stub: `status` decides what getSignatureStatuses says; `stall` methods never answer. */
async function fakeRpc(o: { status: () => unknown; stall?: string[]; height?: number }) {
  const calls: string[] = [];
  const srv: Server = createServer((req, res) => {
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => {
      const j = JSON.parse(b);
      const one = (r: any) => {
        calls.push(r.method);
        if (o.stall?.includes(r.method)) return undefined;
        const result =
          r.method === "getLatestBlockhash" ? { context: { slot: 1 }, value: { blockhash: "11111111111111111111111111111111", lastValidBlockHeight: 100 } }
          : r.method === "sendTransaction" ? "5".repeat(64)
          : r.method === "getSignatureStatuses" ? { context: { slot: 1 }, value: [o.status()] }
          : r.method === "getBlockHeight" ? o.height ?? 50
          : r.method === "getProgramAccounts" ? []
          : r.method === "getHealth" ? "ok"
          : null;
        return { jsonrpc: "2.0", id: r.id, result };
      };
      const out = Array.isArray(j) ? j.map(one) : one(j);
      if (out === undefined || (Array.isArray(out) && out.includes(undefined))) return; // stall: never reply
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(out));
    });
  });
  await new Promise<void>((r) => srv.listen(0, "127.0.0.1", () => r()));
  const url = `http://127.0.0.1:${(srv.address() as any).port}`;
  return { url, calls, close: () => { srv.closeAllConnections(); srv.close(); } };
}

const payer = Keypair.generate();
const ix = () => SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: Keypair.generate().publicKey, lamports: 1 });

test("send returns once the status reaches the commitment", async () => {
  let n = 0;
  const f = await fakeRpc({ status: () => (++n < 3 ? null : { slot: 1, confirmations: 1, err: null, confirmationStatus: "confirmed" }) });
  try {
    const c = new RpcChain(f.url, "confirmed", { pollMs: 10 });
    assert.equal(await c.send([ix()], payer), "5".repeat(64));
  } finally {
    f.close();
  }
});

test("a tx never seen: send throws within confirmMs instead of hanging", async () => {
  const f = await fakeRpc({ status: () => null });
  try {
    const c = new RpcChain(f.url, "confirmed", { pollMs: 10, confirmMs: 300 });
    const t0 = Date.now();
    await assert.rejects(c.send([ix()], payer), /not confirmed .* within 300 ms/);
    assert.ok(Date.now() - t0 < 5_000);
  } finally {
    f.close();
  }
});

test("a blockhash past its height: expired, not waited out", async () => {
  const f = await fakeRpc({ status: () => null, height: 101 });
  try {
    const c = new RpcChain(f.url, "confirmed", { pollMs: 5, confirmMs: 60_000 });
    await assert.rejects(c.send([ix()], payer), /expired/);
  } finally {
    f.close();
  }
});

test("a failed tx throws its error", async () => {
  const f = await fakeRpc({ status: () => ({ slot: 1, confirmations: 1, err: { InstructionError: [1, { Custom: 6009 }] }, confirmationStatus: "confirmed" }) });
  try {
    await assert.rejects(new RpcChain(f.url, "confirmed", { pollMs: 10 }).send([ix()], payer), /failed: .*6009/);
  } finally {
    f.close();
  }
});

test("an RPC that never answers: each request is bounded by requestMs", async () => {
  const f = await fakeRpc({ status: () => null, stall: ["getSignatureStatuses"] });
  try {
    const c = new RpcChain(f.url, "confirmed", { pollMs: 10, confirmMs: 500, requestMs: 200 });
    const t0 = Date.now();
    await assert.rejects(c.send([ix()], payer), /not confirmed/);
    assert.ok(Date.now() - t0 < 5_000, `${Date.now() - t0} ms`);
  } finally {
    f.close();
  }
});

function run(args: string[], env: Record<string, string>): Promise<{ code: number | null; out: string; ms: number }> {
  const t0 = Date.now();
  return new Promise((res) => {
    const p = spawn(process.execPath, [resolve(here, "../src/reconcile.ts"), ...args], { env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    p.stdout.on("data", (c) => (out += c));
    p.stderr.on("data", (c) => (out += c));
    const kill = setTimeout(() => p.kill("SIGKILL"), 60_000);
    p.on("exit", (code) => {
      clearTimeout(kill);
      res({ code, out, ms: Date.now() - t0 });
    });
  });
}

test("reconcile.ts --cancel-drafts exits when done, also while the storefront is up", async () => {
  const f = await fakeRpc({ status: () => null });
  // a stand-in storefront /health on another ephemeral port
  const health = createServer((_q, r) => r.end('{"ok":true}'));
  await new Promise<void>((r) => health.listen(0, "127.0.0.1", () => r()));
  const dir = mkdtempSync(join(tmpdir(), "sf-reconcile-"));
  writeFileSync(join(dir, "wallet.json"), JSON.stringify(Array.from(Keypair.generate().secretKey)));
  writeFileSync(join(dir, "state.json"), JSON.stringify({ payments: {} }));
  const env = { TOKEN_RPC: f.url, STOREFRONT_WALLET: join(dir, "wallet.json"), STATE_FILE: join(dir, "state.json"), STOREFRONT_PORT: String((health.address() as any).port), TOOL_TIMEOUT_MS: "45000", RPC_REQUEST_TIMEOUT_MS: "5000" };
  try {
    const r = await run(["--cancel-drafts"], env);
    assert.equal(r.code, 0, r.out);
    assert.match(r.out, /"orphans": 0/);
    // --refund still refuses while the service answers (it rewrites the state file)
    const r2 = await run(["--refund", "x"], env);
    assert.equal(r2.code, 1);
    assert.match(r2.out, /stop it first/);
    // the plain listing exits too
    assert.equal((await run([], env)).code, 0);
  } finally {
    f.close();
    health.close();
  }
});

test("reconcile.ts: the watchdog ends a run whose RPC never answers", async () => {
  const f = await fakeRpc({ status: () => null, stall: ["getProgramAccounts"] });
  const dir = mkdtempSync(join(tmpdir(), "sf-reconcile-"));
  writeFileSync(join(dir, "wallet.json"), JSON.stringify(Array.from(Keypair.generate().secretKey)));
  try {
    const r = await run([], { TOKEN_RPC: f.url, STOREFRONT_WALLET: join(dir, "wallet.json"), STATE_FILE: join(dir, "none.json"), TOOL_TIMEOUT_MS: "1500" });
    assert.notEqual(r.code, 0, r.out);
    assert.ok(r.ms < 40_000, `${r.ms} ms`);
  } finally {
    f.close();
  }
});
