// In-process fake stack for the integration tests: agent-chain node JSON-RPC, Solana RPC
// (real account layouts built byte for byte), inference gateway, storefront-x402, pool.
// Ports are ephemeral (listen(0)) on 127.0.0.1.

import { createServer, type Server } from "node:http";
import { b58encode, findPda, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import * as D from "../../../apps/explorer/src/decode.ts";
import { accountDisc, ata, bridgeConfigPda, bondingCurvePda, depositPda, RESERVE_ID, reserveAta, reserveConfigPda, reserveCreatorPda, TOKEN_2022 } from "../src/sol.ts";

export const Z = "0".repeat(64);
export const h64 = (s: string) => s.repeat(64).slice(0, 64);
export const key = (seed: string) => b58encode(sha256(utf8("key:" + seed)));

export const POSTER = key("poster");
export const SERVER = key("server");
export const MINER = key("miner");
export const OWNER = key("owner");
export const MINT = key("mint");
export const VAULT = key("vault");
export const SF_WALLET = key("storefront-wallet");
export const J1 = h64("a1");
export const T1 = h64("7e");
export const S1 = h64("51");
export const MODEL = h64("0d");

const u64 = (n: bigint | number) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return b;
};
const i64 = (n: bigint | number) => {
  const b = Buffer.alloc(8);
  b.writeBigInt64LE(BigInt(n));
  return b;
};
const pk = (s: string) => Buffer.from(D.b58decode(s));

export function bridgeConfigData(nonce = 7n) {
  return Buffer.concat([Buffer.from(accountDisc("Config")), pk(key("admin")), pk(MINT), pk(VAULT), pk(TOKEN_2022), u64(nonce), Buffer.from([1]), Buffer.alloc(8)]);
}
export function mintData(supply: bigint, decimals = 6) {
  const b = Buffer.alloc(82);
  b.writeBigUInt64LE(supply, 36);
  b[44] = decimals;
  b[45] = 1;
  return b;
}
export function tokenAccountData(mint: string, owner: string, amount: bigint) {
  const b = Buffer.alloc(165);
  pk(mint).copy(b, 0);
  pk(owner).copy(b, 32);
  b.writeBigUInt64LE(amount, 64);
  b[108] = 1;
  return b;
}
export function reserveConfigData() {
  return Buffer.concat([
    Buffer.from(accountDisc("Config")),
    pk(key("admin")), pk(MINT), Buffer.alloc(32), Buffer.alloc(32), pk(D.BRIDGE_ID), pk(key("pool")),
    u64(5_000_000_000n), u64(10_000_000n), i64(60), Buffer.from([0x2c, 0x01]), u64(2_000_000n), u64(0n), i64(1_760_000_000n), u64(123_000_000n), u64(1_500_000_000n), u64(100_000_000n),
    Buffer.from([254, 253]),
  ]);
}
export function curveData() {
  const b = Buffer.alloc(150);
  Buffer.from(sha256(utf8("account:BondingCurve")).subarray(0, 8)).copy(b, 0);
  b.writeBigUInt64LE(1_000_000_000_000_000n, 8); // virtual token
  b.writeBigUInt64LE(30_000_000_000n, 16); // virtual sol
  b.writeBigUInt64LE(800_000_000_000_000n, 24);
  b.writeBigUInt64LE(0n, 32);
  b.writeBigUInt64LE(1_000_000_000_000_000n, 40); // token_total_supply
  b[48] = 0;
  return b;
}
export function depositData(nonce: bigint, sender: string, recipient: string, amount: bigint) {
  return Buffer.concat([Buffer.from(accountDisc("Deposit")), u64(nonce), pk(sender), pk(recipient), u64(amount), u64(1234n), Buffer.from([255])]);
}

export interface FakeState {
  genesis: string;
  height: number;
  sent: { chain: string[]; solana: string[] };
  accounts: Map<string, { data: Buffer; owner: string; lamports: number }>;
  gatewayCalls: { method: string; path: string; headers: Record<string, string>; body: string }[];
}

function job(o: Record<string, unknown>) {
  return { poster: POSTER, status: "completed", worker_pay: "400000", protocol_fee: "100000", subsidy_paid: "80000", cap_remaining: "0", blocks_mined: 1, max_blocks: 5, posted_height: 3, closed_height: 6, deadline_height: 999, index: 0, hash_a: Z, data_uri: "", evaluators: [], threshold: 1, deliverer: SERVER, kind: "job", standing: null, m: 4, n: 8, k: 16, ...o };
}

export function makeState(): FakeState {
  const st: FakeState = { genesis: h64("g1"), height: 12, sent: { chain: [], solana: [] }, accounts: new Map(), gatewayCalls: [] };
  const put = (a: string, data: Buffer, owner: string, lamports = 1_000_000) => st.accounts.set(a, { data, owner, lamports });
  put(bridgeConfigPda(), bridgeConfigData(), D.BRIDGE_ID);
  put(MINT, mintData(999_000_000_000_000n), TOKEN_2022);
  put(VAULT, tokenAccountData(MINT, bridgeConfigPda(), 5_000_000n), TOKEN_2022);
  put(reserveConfigPda(), reserveConfigData(), RESERVE_ID);
  put(reserveCreatorPda(), Buffer.alloc(0), "11111111111111111111111111111111", 2_500_000);
  put(reserveAta(MINT), tokenAccountData(MINT, reserveCreatorPda(), 77_000_000n), TOKEN_2022);
  put(bondingCurvePda(MINT), curveData(), "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
  put(ata(OWNER, MINT, TOKEN_2022), tokenAccountData(MINT, OWNER, 9_000_000n), TOKEN_2022);
  put(D.buybackVaultPda(), Buffer.alloc(0), D.STOREFRONT_ID, 3_000_000);
  put(depositPda(3n), depositData(3n, OWNER, OWNER, 2_000n), D.BRIDGE_ID);
  return st;
}

const listen = (s: Server) => new Promise<number>((r) => s.listen(0, "127.0.0.1", () => r((s.address() as any).port)));

function rpcServer(handle: (m: string, p: any) => any) {
  return createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      if (req.method === "GET") return res.end("ok");
      const q = JSON.parse(body);
      const one = (x: any) => {
        try {
          return { jsonrpc: "2.0", id: x.id, result: handle(x.method, x.params) };
        } catch (e) {
          return { jsonrpc: "2.0", id: x.id, error: { code: -32000, message: (e as Error).message } };
        }
      };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
}

export async function startFakeStack(st: FakeState) {
  const blocks = () =>
    Array.from({ length: st.height }, (_, i) => {
      const h = i + 1;
      const jobId = h === 5 ? J1 : h === 8 ? S1 : Z;
      return {
        hash: h64(`b${h}`), fees: "1000", subsidy: h === 5 || h === 8 ? "40000" : "0", reward: "1000", proof_len: 100, canonical: true, final: h <= st.height - 2, confirmations: st.height - h,
        header_fields: { height: h, prev_hash: h > 1 ? h64(`b${h - 1}`) : Z, timestamp_ms: 1_760_000_000_000 + h * 1500, miner: MINER, job_id: jobId, proof_hash: h64("p"), target_bits: 1 },
        txs: h === 2 ? [{ tx_hash: h64("tx2"), kind: "RegisterAgent", from: SERVER, fee: "1000", tx: Buffer.alloc(60).toString("base64") }] : [],
      };
    });
  const jobs = () => [
    job({ job_id: J1 }),
    job({ job_id: T1, kind: "ticket", deliverer: SERVER, standing: { model_id: MODEL, server: SERVER, rows: 16, prompt_tokens: 6, max_new_tokens: 10, segments: 1, blocks_mined: 1, subsidy_paid: "40000" }, protocol_fee: "200000", worker_pay: "800000", posted_height: 7, closed_height: 9 }),
    job({ job_id: S1, kind: "segment", poster: SERVER, worker_pay: "0", protocol_fee: "0", standing: { ticket_id: T1, model_id: MODEL, matrix: 0 }, posted_height: 7 }),
    job({ job_id: h64("e1"), status: "expired", deliverer: SERVER, protocol_fee: "50000", posted_height: 4, closed_height: 11 }),
    job({ job_id: h64("f1"), status: "completed", poster: SERVER, deliverer: SERVER, protocol_fee: "999000", posted_height: 4, closed_height: 10 }),
  ];
  const node = rpcServer((m, p) => {
    switch (m) {
      case "getChainInfo": return { chain_id: 1347616769, genesis_hash: st.genesis, height: st.height, finalized_height: st.height - 2, finality_k: 2, target_bits: 1, next_target_bits: 1 };
      case "getParams": return { alpha_ppm: 800000, emission_ppm: 10000, min_fee: "1000", target_block_time_ms: 1500, finality_k: 2, fallback_m: 4, fallback_n: 8, max_job_blocks: 100000, bridge_paused: false };
      case "getBlock": {
        const b = blocks()[(p.height ?? 0) - 1] ?? blocks().find((x) => x.hash === p.hash);
        return b ?? null;
      }
      case "getHeader": {
        const b = blocks()[(p.height ?? 0) - 1];
        return b ? { hash: b.hash, header_fields: b.header_fields, proof: Buffer.from("proof").toString("base64") } : null;
      }
      case "listJobs": return { total: jobs().length, jobs: jobs() };
      case "getJob": return jobs().find((j) => j.job_id === p.job_id) ?? null;
      case "getTicket": return p.ticket_id === T1 ? { ticket_id: T1, segments: [] } : null;
      case "getBridgeEvents": return { withdrawal_count: 1, deposit_count: 4, withdrawals: [{ nonce: 0, from: OWNER, solana_recipient: OWNER, amount_spl: "2000", amount: "2000000", height: 6, block_hash: h64("b6"), confirmations: 6, final: true }] };
      case "getDeposit": return p.solana_nonce === 3 ? { processed: true, height: 4, recipient: OWNER, amount_spl: "2000", confirmations: 8, final: true } : { processed: false };
      case "getAccount": return { account: p.account, balance: p.account === OWNER ? "5000000000" : "0", nonce: p.account === OWNER ? 3 : 0, route: null };
      case "getAgent": return p.account === SERVER ? { account: SERVER, name: "server-one", uri: "https://agent.example/card.json", registered_height: 2, updated_height: 2 } : null;
      case "getRoute": return null;
      case "getAccountTxs": return { account: p.account, txs: [], next: null, tip_height: st.height, finalized_height: st.height - 2 };
      case "getEscrow": return { escrow: key("escrow"), escrow_balance: "999999995000000000", pool: key("pool"), pool_balance: "1000000000", total_supply: "1000000000000000000", outside_escrow: "5000000000", outside_escrow_spl: "5000000", height: st.height };
      case "listModels": return { total: 1, models: [{ model_id: MODEL, name: "llama-test", macs_per_row: "1000", row_period: 4 }] };
      case "getTx": return p.tx_hash === h64("ab") ? { tx_hash: h64("ab"), status: "included", height: 2 } : null;
      case "sendTx": {
        st.sent.chain.push(p.tx);
        return { tx_hash: h64("se") };
      }
      default: throw new Error("unknown method " + m);
    }
  });
  const sol = rpcServer((m, p) => {
    switch (m) {
      case "getMultipleAccounts": return { value: p[0].map((a: string) => { const x = st.accounts.get(a); return x ? { data: [x.data.toString("base64"), "base64"], owner: x.owner, lamports: x.lamports } : null; }) };
      case "getProgramAccounts": {
        const out = [];
        for (const [a, x] of st.accounts) {
          if (x.owner !== p[0]) continue;
          const f = p[1]?.filters ?? [];
          const ok = f.every((ff: any) => { const want = Buffer.from(D.b58decode(ff.memcmp.bytes)); return x.data.subarray(ff.memcmp.offset, ff.memcmp.offset + want.length).equals(want); });
          if (ok) out.push({ pubkey: a, account: { data: [x.data.toString("base64"), "base64"], owner: x.owner, lamports: x.lamports } });
        }
        return out;
      }
      case "getSignaturesForAddress": return [];
      case "getTransaction": return null;
      case "getBalance": return { value: p[0] === OWNER ? 7_000_000_000 : 0 };
      case "getTokenAccountsByOwner": {
        const out = [];
        for (const [a, x] of st.accounts) if (x.owner === TOKEN_2022 && x.data.length === 165 && b58encode(x.data.subarray(32, 64)) === p[0] && b58encode(x.data.subarray(0, 32)) === p[1].mint) out.push({ pubkey: a, account: { data: [x.data.toString("base64"), "base64"], owner: x.owner, lamports: x.lamports } });
        return { value: out };
      }
      case "getLatestBlockhash": return { value: { blockhash: b58encode(sha256(utf8("bh"))), lastValidBlockHeight: 500 } };
      case "getSignatureStatuses": return { value: [p[0][0].startsWith("5") ? { slot: 9, confirmations: null, confirmationStatus: "finalized", err: null } : null] };
      case "sendTransaction": {
        st.sent.solana.push(p[0]);
        return "5" + "1".repeat(86);
      }
      default: throw new Error("unknown method " + m);
    }
  });
  const http = (f: (method: string, path: string, headers: Record<string, string>, body: string) => { status: number; body: unknown; headers?: Record<string, string>; sse?: string[] }) =>
    createServer((req, res) => {
      let body = "";
      req.on("data", (d) => (body += d));
      req.on("end", () => {
        const r = f(req.method ?? "GET", req.url ?? "/", req.headers as Record<string, string>, body);
        if (r.sse) {
          res.writeHead(r.status, { "content-type": "text/event-stream", ...(r.headers ?? {}) });
          for (const e of r.sse) res.write(`data: ${e}\n\n`);
          res.end();
          return;
        }
        res.writeHead(r.status, { "content-type": "application/json", ...(r.headers ?? {}) });
        res.end(JSON.stringify(r.body));
      });
    });
  const gateway = http((method, path, headers, body) => {
    st.gatewayCalls.push({ method, path, headers, body });
    if (path === "/health") return { status: 200, body: { ok: true } };
    if (path === "/v1/models") return { status: 200, body: { data: [{ id: MODEL, name: "llama-test" }] } };
    if (path === "/v1/completions" && method === "POST") {
      if (!headers["x-payment"]) return { status: 402, body: { x402Version: 1, error: "X-PAYMENT header is required", accepts: [{ scheme: "exact", network: "solana-localnet", maxAmountRequired: "24000000", payTo: SF_WALLET, asset: "So11111111111111111111111111111111111111112", extra: { feePayer: SF_WALLET, assetKind: "native", decimals: 9 } }] } };
      return { status: 201, body: { ticket_id: T1, text: "hi" }, headers: { "x-payment-response": "eyJzdWNjZXNzIjp0cnVlfQ==" } };
    }
    if (path === "/v1/chat/completions" && method === "POST") return { status: 200, body: null, sse: ['{"choices":[{"delta":{"content":"he"}}]}', '{"choices":[{"delta":{"content":"llo"}}]}', "[DONE]"] };
    if (path.startsWith("/v1/requests/")) return { status: 200, body: { gateway: { stage: "completed" }, ticket: null } };
    return { status: 404, body: { error: `no route ${method} ${path}` } };
  });
  const storefront = http((method, path) => {
    const u = new URL(path, "http://x");
    if (u.pathname === "/health") return { status: 200, body: { ok: true, wallet: SF_WALLET, network: "solana-localnet" } };
    if (u.pathname === "/quote") {
      if (u.searchParams.get("model_id") !== MODEL) return { status: 400, body: { error: "not registered" } };
      return { status: 200, body: { m: Number(u.searchParams.get("prompt_tokens")), n: Number(u.searchParams.get("max_tokens")), k: 1, ticket: { model_id: MODEL }, work_flops: "32000", sol_in_lamports: "11111112", price_lamports: "25111112", buyback_lamports: "1111111", venue: "curve", payment: { pay_to: SF_WALLET } } };
    }
    return { status: 404, body: { error: "not found" } };
  });
  const pool = http((_m, path) => (path.startsWith("/stats") ? { status: 200, body: { miners: 2, shares: 10 } } : path.startsWith("/miner") ? { status: 200, body: { payout: MINER, shares: 5 } } : { status: 200, body: "ok" }));
  const servers = [node, sol, gateway, storefront, pool];
  const [np, sp, gp, fp, pp] = await Promise.all(servers.map(listen));
  return {
    urls: { node: `http://127.0.0.1:${np}`, sol: `http://127.0.0.1:${sp}`, gateway: `http://127.0.0.1:${gp}`, storefront: `http://127.0.0.1:${fp}`, pool: `http://127.0.0.1:${pp}` },
    close: () => servers.forEach((s) => s.close()),
  };
}

void findPda;
