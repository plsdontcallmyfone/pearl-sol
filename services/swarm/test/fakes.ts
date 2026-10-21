// In-process fakes for the swarm tests: an agent-chain node (JSON-RPC: checks every tx's
// signature, chain id, nonce and fee, and applies Transfer / SetProfile / OperatorLink / Post /
// React / Follow, and the DM kinds SendMessage / ReclaimMessage with the gate, pay-to-DM escrow and
// release rules of SPEC 4.10; a held mempool for "sending" messages), an inference gateway (checks the cadre-auth-v2 signature naming its server key, serves scripted
// completions with receipts, prepaid balance and an x402 top-up that checks the payment tx's
// signature), cadre-api (/v1/town from the fake node's posts) and a Solana RPC
// (getLatestBlockhash). Ports 10696-10699 on 127.0.0.1.
import { createHash, randomBytes } from "node:crypto";
import { createServer, type Server } from "node:http";
import { b58decode, b58encode } from "../src/b58.ts";
import { decodeTx, frame, NS_TX, feeFor, messageId } from "../src/chaintx.ts";
import { parseSealedDm } from "../src/dmcrypto.ts";
import { authMessageV2 } from "../src/clients.ts";
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
  params: any = { min_fee: "1000", social_min_fee: "1000", social_byte_fee: "2", post_inline_max: 1024, house_operators: [] as string[], target_block_time_ms: 5000 };
  dmActive = true;
  dmParams = { msg_inline_max: 1024, dm_reclaim_min: 10, dm_reclaim_max: 10080, dm_max_open_escrows: 256, dm_protocol_bps: 0 };
  escrows = new Map<string, { from: string; to: string; pay: bigint; height: number; deadline: number }>();
  dmLast = new Map<string, number>();
  history: { tx: string; height: number; index: number; from: string; kind: string; body: any; hash: string; to?: string }[] = [];
  held: Uint8Array[] = [];
  released: string[] = [];
  refunded: { id: string; by: string }[] = [];
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
    if ((b.kind === "SendMessage" || b.kind === "ReclaimMessage") && !this.dmActive) throw new Error("DM txs activate at a later height");
    // DM checks before the fee moves (the node's admission runs the executor first)
    if (b.kind === "SendMessage") this.checkMessage(from, b);
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
    if (b.kind === "SendMessage") {
      const to = b58encode(b.to);
      const id = hex(messageId(d.from, d.nonce));
      if (b.replyTo.some((x: number) => x)) {
        const e = this.escrows.get(hex(b.replyTo));
        if (e && e.to === from && e.from === to && this.height <= e.deadline) {
          credit(e.to, e.pay);
          this.escrows.delete(hex(b.replyTo));
          this.released.push(hex(b.replyTo));
        }
      }
      if (b.pay > 0n) {
        debit(from, b.pay);
        if (b.reclaimAfter > 0) this.escrows.set(id, { from, to, pay: b.pay, height: this.height, deadline: this.height + b.reclaimAfter });
        else credit(to, b.pay);
      }
      this.dmLast.set(`${from}:${to}`, this.height);
    }
    if (b.kind === "ReclaimMessage") {
      const id = hex(b.messageId);
      const e = this.escrows.get(id);
      if (!e) throw new Error("no open escrow for that message");
      if (!(from === e.to || (from === e.from && this.height > e.deadline))) throw new Error("only the recipient, or the sender after the deadline, can reclaim");
      credit(e.from, e.pay);
      this.escrows.delete(id);
      this.refunded.push({ id, by: from });
    }
    this.history.push({ tx: Buffer.from(tx).toString("base64"), height: this.height, index: 0, from, kind: b.kind, body: b, hash: h, to: b.kind === "SendMessage" ? b58encode(b.to) : undefined });
    this.nonces.set(from, n + 1n);
    this.txs.set(h, { status: "included", height: this.height });
    this.sent.push({ from, kind: b.kind, body: b, hash: h });
    this.height++;
    return h;
  }

  /** The DM gate of SPEC 4.10 for `from` -> `to` in the next block (no consent records here). */
  gate(from: string, to: string) {
    const prof = this.profiles.get(to);
    const window = prof?.threadFreeBlocks ?? 0;
    const last = this.dmLast.get(`${to}:${from}`);
    const inThread = last !== undefined && (window === 0 || this.height <= last + window);
    const followed = this.follows.has(`${to}:${from}`);
    const policy = prof?.dmPolicy ?? 0;
    const price = BigInt(prof?.dmPrice ?? 0n);
    const free = inThread || followed;
    const [can, required, reason] = policy === 0 ? [true, 0n, "open"] : policy === 1 ? (free ? [true, 0n, "free"] : [true, price, "priced cold DM"]) : policy === 2 ? (free ? [true, 0n, "followed"] : [false, 0n, "followers only"]) : inThread ? [true, 0n, "thread"] : [false, 0n, "closed"];
    const enc = prof?.encKey && prof.encKey.some((x: number) => x) ? b58encode(prof.encKey) : null;
    const mine = this.profiles.get(from)?.encKey;
    return { from, to, next_height: this.height, can_send: can, required_pay: required.toString(), reason, dm_policy: policy, dm_price: price.toString(), in_thread: inThread, followed_by_recipient: followed, enc_key: enc, sender_enc_key: mine && mine.some((x: number) => x) ? b58encode(mine) : null, sealed_ok: !!enc, msg_inline_max: this.dmParams.msg_inline_max };
  }

  checkMessage(from: string, b: any) {
    const to = b58encode(b.to);
    if (b.channel.some((x: number) => x) || !b.to.some((x: number) => x)) throw new Error("fake node: DMs only");
    if (b.body.length > this.dmParams.msg_inline_max) throw new Error("body over msg_inline_max");
    const g = this.gate(from, to);
    if (!g.can_send) throw new Error(`refused: ${g.reason}`);
    if (b.pay < BigInt(g.required_pay)) throw new Error(`a cold DM to this inbox needs pay >= ${g.required_pay}`);
    if (b.mode === 1) {
      if (!g.enc_key) throw new Error("the recipient has no enc_key");
      parseSealedDm(b.body);
    }
    if (b.reclaimAfter > 0 && b.pay === 0n) throw new Error("reclaim_after needs pay");
    if (b.reclaimAfter > 0 && (b.reclaimAfter < this.dmParams.dm_reclaim_min || b.reclaimAfter > this.dmParams.dm_reclaim_max)) throw new Error("reclaim_after out of range");
  }

  escrowJson(id: string) {
    const e = this.escrows.get(id);
    if (!e) return null;
    const next = this.height;
    return { message_id: id, from: e.from, to: e.to, pay: e.pay.toString(), height: e.height, deadline: e.deadline, reply_open: next <= e.deadline, blocks_left: Math.max(0, e.deadline - (next - 1)), reclaimable: next > e.deadline };
  }

  /** Hold a signed tx in the mempool (status pending) until mine(). */
  hold(tx: Uint8Array): string {
    this.held.push(tx);
    return createHash("sha256").update(tx).digest("hex");
  }
  mine() {
    const h = this.held;
    this.held = [];
    for (const t of h) this.apply(t);
  }

  async rpc(method: string, p: any): Promise<any> {
    switch (method) {
      case "getParams": return this.params;
      case "getChainInfo": return { chain_id: this.chainId, height: this.height };
      case "getAccount": return { account: p.account, balance: (this.balances.get(p.account) ?? 0n).toString(), nonce: Number(this.nonces.get(p.account) ?? 0n) };
      case "getMempool": return { size: this.mempool + this.held.length, txs: this.held.map((t) => createHash("sha256").update(t).digest("hex")) };
      case "getTx": {
        const held = this.held.find((t) => createHash("sha256").update(t).digest("hex") === p.tx_hash);
        if (held) return { tx_hash: p.tx_hash, status: "pending", tx: Buffer.from(held).toString("base64") };
        return this.txs.get(p.tx_hash) ?? null;
      }
      case "getDmParams":
        if (!this.dmActive) return { ...this.dmParams, activation_height: null, active: false };
        return { ...this.dmParams, activation_height: 0, active: true };
      case "getDmGate": return this.gate(p.from, p.to);
      case "getMessageEscrow": return this.escrowJson(p.message_id);
      case "getAccountTxs": {
        const rows = this.history.filter((x) => x.from === p.account || x.to === p.account).reverse().slice(0, p.limit ?? 50);
        return { account: p.account, txs: rows.map((x) => ({ tx_hash: x.hash, kind: x.kind, from: x.from, tx: x.tx, height: x.height, index: x.index, role: x.from === p.account ? "sender" : x.kind === "SendMessage" ? "message" : "recipient" })), next: null };
      }
      case "getProfile": {
        const x = this.profiles.get(p.account);
        if (!x) return null;
        const z = (k: Uint8Array) => (k && k.some((v) => v) ? b58encode(k) : null);
        return {
          account: p.account, has_profile: true, handle: x.handle || null, name: x.name, profile_uri: x.profileUri, profile_hash: x.profileUri ? hex(x.profileHash) : null, flags: x.flags,
          operator: x.operator === b58encode(new Uint8Array(32)) ? null : x.operator, operator_linked: this.links.has(`${x.operator}:${p.account}`),
          enc_key: z(x.encKey), dm_price: String(x.dmPrice ?? 0n), dm_policy: x.dmPolicy ?? 0, thread_free_blocks: x.threadFreeBlocks ?? 0,
        };
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
  /** This gateway's server key: the cadre-auth-v2 audience (served in /v1/models). */
  server = b58encode(randomBytes(32));
  constructor(model: Model) { this.model = model; }

  /** Strict like a gateway with CADRE_AUTH_REQUIRE_AUDIENCE=1: v2 naming this server only. */
  checkAuth(method: string, path: string, h: Record<string, any>, body: string): boolean {
    const w = h["x-cadre-wallet"], ts = Number(h["x-cadre-timestamp"]), sig = h["x-cadre-signature"];
    if (!w || !sig || Math.abs(Date.now() - ts) > 120_000) return false;
    return verifySig(b58decode(w), authMessageV2(this.server, method, path, ts, body), Uint8Array.from(Buffer.from(sig, "hex")));
  }

  async http(method: string, url: string, h: Record<string, any>, body: string) {
    const path = url.split("?")[0];
    if (path === "/v1/models" || path === "/models") return { status: 200, body: { object: "list", data: [{ id: "test-model", object: "model", cadre: { server: this.server, macs_per_row: "1235746816", pricing: { lamports_per_row: this.lamportsPerRow ?? null } } }] } };
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
  lamportsPerRow: string | null = "37"; // a test fixture, not a measured price
}

export interface FakeApi { quote: boolean; splPerLamport: bigint; handler?: (url: string) => Promise<{ status: number; body: any } | null> }
export interface Fakes { node: FakeNode; gw: FakeGateway; api: FakeApi; close: () => Promise<void> }

export async function startFakes(model: Model): Promise<Fakes> {
  const node = new FakeNode();
  const gw = new FakeGateway(model);
  const api: FakeApi = { quote: true, splPerLamport: 30n }; // test fixtures, not market data
  const servers = [
    await serve(PORTS.node, async (_m, _u, _h, body) => {
      const j = JSON.parse(body);
      try { return { status: 200, body: { jsonrpc: "2.0", id: j.id, result: await node.rpc(j.method, j.params ?? {}) } }; } catch (e) { return { status: 200, body: { jsonrpc: "2.0", id: j.id, error: { code: -32000, message: String((e as Error).message) } } }; }
    }),
    await serve(PORTS.gateway, (m, u, h, b) => gw.http(m, u, h, b)),
    await serve(PORTS.api, async (_m, url) => {
      if (url.startsWith("/v1/pump/quote")) {
        if (!api.quote) return { status: 503, body: { error: { code: "curve_missing", message: "no pump bonding curve" } } };
        const sol = BigInt(new URL(url, "http://x").searchParams.get("sol_in") ?? "0");
        return { status: 200, body: { side: "buy", venue: "curve", sol_in_lamports: sol.toString(), tokens_out_spl: (sol * api.splPerLamport).toString() } };
      }
      if (api.handler) { const r = await api.handler(url); if (r) return r; }
      if (!url.startsWith("/v1/town")) return { status: 404, body: {} };
      const posts = [...node.posts].reverse().slice(0, 60).map((p) => ({ ...p, author: { account: p.author, handle: node.profiles.get(p.author)?.handle ?? null } }));
      return { status: 200, body: { posts, pending: [], next: null } };
    }),
    await serve(PORTS.sol, async () => ({ status: 200, body: { jsonrpc: "2.0", id: 1, result: { value: { blockhash: b58encode(randomBytes(32)) } } } })),
  ];
  return { node, gw, api, close: async () => { for (const s of servers) await new Promise((r) => s.close(() => r(null))); } };
}
