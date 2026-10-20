// In-process fakes for the swarm tests: an agent-chain node (JSON-RPC: checks every tx's
// signature, chain id, nonce and fee, and applies Transfer / SetProfile / OperatorLink / Post /
// React / Follow), an inference gateway (checks the cadre-auth-v1 signature, serves scripted
// completions with receipts, prepaid balance and an x402 top-up that checks the payment tx's
// signature), cadre-api (/v1/town from the fake node's posts) and a Solana RPC
// (getLatestBlockhash). Ports 10696-10699 on 127.0.0.1.
import { createHash, randomBytes } from "node:crypto";
import { createServer, type Server } from "node:http";
import { b58decode, b58encode } from "../src/b58.ts";
import { decodeTx, frame, NS_TX, feeFor } from "../src/chaintx.ts";
import { authMessage } from "../src/clients.ts";
import { verifySig } from "../src/wallet.ts";

export const PORTS = { node: 10696, gateway: 10697, api: 10698, sol: 10699 };
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");

function serve(port: number, h: (method: string, url: string, headers: Record<string, any>, body: string) => Promise<{ status: number; body: any; headers?: Record<string, string> }>): Promise<Server> {
  const s = createServer((req, res) => {
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", async () => {
      try {
        const r = await h(req.method!, req.url!, req.headers, b);
        res.writeHead(r.status, { "content-type": "application/json", ...(r.headers ?? {}) });
        res.end(typeof r.body === "string" ? r.body : JSON.stringify(r.body));
      } catch (e) {
        res.writeHead(500);
        res.end(JSON.stringify({ error: String(e) }));
      }
    });
  });
  return new Promise((ok) => s.listen(port, "127.0.0.1", () => ok(s)));
}

export class FakeNode {
  chainId = 1128350802;
  height = 5000;
  params: any = { min_fee: "1000", social_min_fee: "1000", social_byte_fee: "2", post_inline_max: 1024, house_operators: [] as string[] };
  balances = new Map<string, bigint>();
  nonces = new Map<string, bigint>();
  profiles = new Map<string, any>();
  links = new Set<string>();
  follows = new Set<string>();
  reacts = new Set<string>();
  posts: any[] = [];
  txs = new Map<string, any>();
  blobs = new Map<string, Buffer>();
  sent: any[] = [];
  refused: string[] = [];
  mempool = 0;

  apply(tx: Uint8Array): string {
    const d = decodeTx(tx);
    const from = b58encode(d.from);
    if (d.chainId !== this.chainId) throw new Error("wrong chain_id");
    if (!verifySig(d.from, frame(NS_TX, d.signing), d.sig)) throw new Error("bad tx signature");
    const want = feeFor(d.body, this.params);
    if (d.fee < want) throw new Error(`fee ${d.fee} below ${want}`);
    const n = this.nonces.get(from) ?? 0n;
    if (d.nonce !== n) throw new Error(`bad nonce: expected ${n}, got ${d.nonce}`);
    const bal = this.balances.get(from) ?? 0n;
    if (bal < d.fee) throw new Error("cannot pay fee");
    const b: any = d.body;
    const debit = (a: string, x: bigint) => { const v = this.balances.get(a) ?? 0n; if (v < x) throw new Error("insufficient balance"); this.balances.set(a, v - x); };
    const credit = (a: string, x: bigint) => this.balances.set(a, (this.balances.get(a) ?? 0n) + x);
    const h = createHash("sha256").update(tx).digest("hex");
    debit(from, d.fee);
    switch (b.kind) {
      case "Transfer": debit(from, b.amount); credit(b58encode(b.to), b.amount); break;
      case "OperatorLink": {
        const k = `${from}:${b58encode(b.agent)}`;
        if (b.add === this.links.has(k)) throw new Error(b.add ? "link exists" : "no such link");
        b.add ? this.links.add(k) : this.links.delete(k);
        break;
      }
      case "SetProfile": {
        const op = b58encode(b.operator);
        if (b.flags & 2 && (!this.params.house_operators.includes(op) || !this.links.has(`${op}:${from}`))) throw new Error("house flag refused");
        if (!/^[a-z0-9_]{3,24}$/.test(b.handle)) throw new Error("handle");
        for (const [a, p] of this.profiles) if (a !== from && p.handle === b.handle) throw new Error("handle is taken");
        if (b.profileUri.startsWith("blob:") && b.profileUri.slice(5) !== hex(b.profileHash)) throw new Error("profile_hash");
        this.profiles.set(from, { ...b, account: from, operator: op });
        break;
      }
      case "Post": {
        if (Buffer.byteLength(b.body) > this.params.post_inline_max) throw new Error("post too long");
        if (b.postKind !== 0 && !this.posts.some((p) => p.post_id === hex(b.parent))) throw new Error("parent post does not exist");
        // stand-in id (the real one is blake3("pearl-sol:post:v1" || from || nonce))
        const id = createHash("sha256").update(Buffer.concat([Buffer.from("post"), d.from, Buffer.from(d.nonce.toString())])).digest("hex");
        this.posts.push({ post_id: id, author: from, kind: ["post", "reply", "repost", "quote"][b.postKind], parent: b.postKind ? hex(b.parent) : null, body: b.body, thought_ref: b.thoughtRef, height: this.height, tx_hash: h });
        break;
      }
      case "React":
        for (const x of b.items) {
          const k = `${hex(x.postId)}:${from}:${x.reaction}`;
          if (this.reacts.has(k)) throw new Error("already given");
          if (!this.posts.some((p) => p.post_id === hex(x.postId))) throw new Error("unknown post");
          this.reacts.add(k);
        }
        break;
      case "Follow":
        for (const x of b.items) {
          const k = `${from}:${b58encode(x.target)}`;
          if (x.on === this.follows.has(k)) throw new Error(x.on ? "already following" : "not following");
          x.on ? this.follows.add(k) : this.follows.delete(k);
        }
        break;
    }
    this.nonces.set(from, n + 1n);
    this.txs.set(h, { status: "included", height: this.height });
    this.sent.push({ from, kind: b.kind, body: b, hash: h });
    this.height++;
    return h;
  }

  async rpc(method: string, p: any): Promise<any> {
    switch (method) {
      case "getParams": return this.params;
      case "getChainInfo": return { chain_id: this.chainId, height: this.height };
      case "getAccount": return { account: p.account, balance: (this.balances.get(p.account) ?? 0n).toString(), nonce: Number(this.nonces.get(p.account) ?? 0n) };
      case "getMempool": return { size: this.mempool, txs: [] };
      case "getTx": return this.txs.get(p.tx_hash) ?? null;
      case "getProfile": {
        const x = this.profiles.get(p.account);
        return x ? { account: p.account, operator: x.operator, operator_linked: this.links.has(`${x.operator}:${p.account}`) } : null;
      }
      case "isFollowing": return { following: this.follows.has(`${p.follower}:${p.target}`) };
      case "putBlob": {
        const data = Buffer.from(p.data, "base64");
        const id = createHash("sha256").update(data).digest("hex"); // stand-in for blake3
        this.blobs.set(id, data);
        return { blob_id: id, uri: `blob:${id}`, size: data.length, new: true };
      }
      case "sendTx":
        try {
          return { tx_hash: this.apply(Uint8Array.from(Buffer.from(p.tx, "base64"))) };
        } catch (e) {
          this.refused.push(String((e as Error).message));
          throw e;
        }
      default: throw new Error(`unknown method ${method}`);
    }
  }
}

export type Model = (messages: { role: string; content: string }[]) => string;

export class FakeGateway {
  model: Model;
  wallet = "";
  rowsFree = 0;
  receipts = new Map<string, any>();
  prompts: string[] = [];
  topups = 0;
  priceLamports = 3_000_000n;
  payTo = b58encode(randomBytes(32));
  authFailures = 0;
  constructor(model: Model) { this.model = model; }

  checkAuth(method: string, path: string, h: Record<string, any>, body: string): boolean {
    const w = h["x-cadre-wallet"], ts = Number(h["x-cadre-timestamp"]), sig = h["x-cadre-signature"];
    if (!w || !sig || Math.abs(Date.now() - ts) > 120_000) return false;
    return verifySig(b58decode(w), authMessage(method, path, ts, body), Uint8Array.from(Buffer.from(sig, "hex")));
  }

  async http(method: string, url: string, h: Record<string, any>, body: string) {
    const path = url.split("?")[0];
    if (path === "/v1/chat/completions" || path === "/v1/balance") {
      if (!this.checkAuth(method, path, h, body)) { this.authFailures++; return { status: 401, body: { error: { message: "bad signature" } } }; }
    }
    if (path === "/v1/balance") return { status: 200, body: { wallet: h["x-cadre-wallet"], rows_free: this.rowsFree, tickets: [] } };
    if (path === "/v1/balance/topup") {
      const b = JSON.parse(body || "{}");
      const req = { scheme: "exact", network: "solana-devnet", maxAmountRequired: this.priceLamports.toString(), payTo: this.payTo, extra: { assetKind: "native" } };
      if (!h["x-payment"]) return { status: 402, body: { x402Version: 1, accepts: [req], rows: b.rows } };
      const p = JSON.parse(Buffer.from(h["x-payment"], "base64").toString());
      const tx = Buffer.from(p.payload.transaction, "base64");
      // legacy tx: 1 sig (payer pays its own fee here), then the message; payer = first key
      const nSig = tx[0];
      const msg = tx.subarray(1 + 64 * nSig);
      const payer = msg.subarray(4, 36);
      if (!verifySig(payer, msg, tx.subarray(1, 65))) return { status: 402, body: { error: "bad payment signature" } };
      this.topups++;
      this.rowsFree += Number(b.rows);
      return { status: 201, body: { ticket_id: hex(randomBytes(32)), order: String(this.topups), rows: b.rows, round_secs: b.round_secs ?? null } };
    }
    if (path === "/v1/chat/completions") {
      const b = JSON.parse(body);
      this.prompts.push(JSON.stringify(b.messages));
      const promptBytes = b.messages.reduce((s: number, m: any) => s + Buffer.byteLength(m.content), 0);
      const text = this.model(b.messages);
      const completion = Math.min(b.max_tokens, Math.ceil(Buffer.byteLength(text) / 4) || 1);
      const rows = Math.ceil(promptBytes / 4) + 30 + completion; // a plausible tokenization, below the swarm's bound
      if (this.rowsFree < rows) return { status: 402, body: { error: { message: "no prepaid balance with enough free rows" } } };
      this.rowsFree -= rows;
      const id = `req_${hex(randomBytes(12))}`;
      const receipt = { request_id: id, rows, text_sha256: createHash("sha256").update(text).digest("hex") };
      this.receipts.set(id, receipt);
      return { status: 200, headers: { "x-cadre-request": id }, body: { choices: [{ message: { role: "assistant", content: text } }], usage: { completion_tokens: completion }, cadre: { request_id: id, rows, ticket_id: "t" } } };
    }
    return { status: 404, body: { error: "not found" } };
  }
}

export interface Fakes { node: FakeNode; gw: FakeGateway; close: () => Promise<void> }

export async function startFakes(model: Model): Promise<Fakes> {
  const node = new FakeNode();
  const gw = new FakeGateway(model);
  const servers = [
    await serve(PORTS.node, async (_m, _u, _h, body) => {
      const j = JSON.parse(body);
      try { return { status: 200, body: { jsonrpc: "2.0", id: j.id, result: await node.rpc(j.method, j.params ?? {}) } }; } catch (e) { return { status: 200, body: { jsonrpc: "2.0", id: j.id, error: { code: -32000, message: String((e as Error).message) } } }; }
    }),
    await serve(PORTS.gateway, (m, u, h, b) => gw.http(m, u, h, b)),
    await serve(PORTS.api, async (_m, url) => {
      if (!url.startsWith("/v1/town")) return { status: 404, body: {} };
      const posts = [...node.posts].reverse().slice(0, 60).map((p) => ({ ...p, author: { account: p.author, handle: node.profiles.get(p.author)?.handle ?? null } }));
      return { status: 200, body: { posts, pending: [], next: null } };
    }),
    await serve(PORTS.sol, async () => ({ status: 200, body: { jsonrpc: "2.0", id: 1, result: { value: { blockhash: b58encode(randomBytes(32)) } } } })),
  ];
  return { node, gw, close: async () => { for (const s of servers) await new Promise((r) => s.close(() => r(null))); } };
}
