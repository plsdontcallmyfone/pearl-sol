// Fakes for the user-agent runtime tests, on 127.0.0.1:10733-10737 (this row's test ports):
// the swarm's FakeNode (services/swarm/test/fakes.ts, extended here with RegisterAgent, getAgent
// and getBlob; the full profile fields incl. the DM settings come from FakeNode), the swarm's
// FakeGateway (cadre-auth-v2 checked, prepaid rows, x402 top-up), and a fake cadre-api: balances, the Solana tx builders (each returns an
// unsigned legacy tx whose fee payer is the named key), /v1/tx/solana (checks the signature),
// /v1/tx/solana/{sig} and /v1/town.
import { createHash, randomBytes } from "node:crypto";
import { createServer, type Server } from "node:http";
import { b58decode, b58encode } from "../../src/b58.ts";
import { frame, NS_TX } from "../../src/chaintx.ts";
import { verifySig } from "../../src/wallet.ts";
import { FakeGateway, FakeNode, type Model } from "../../test/fakes.ts";

export const PORTS = { node: 10733, gateway: 10734, api: 10735, sol: 10736, runtime: 10737 };

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

export class Node2 extends FakeNode {
  agents = new Map<string, any>();
  override async rpc(method: string, p: any): Promise<any> {
    if (method === "sendTx") {
      const tx = Uint8Array.from(Buffer.from(p.tx, "base64"));
      if (tx[53] === 1) return { tx_hash: this.applyRegister(tx) };
    }
    if (method === "getAgent") return this.agents.get(p.account) ?? null;
    if (method === "getBlob") {
      const id = String(p.uri).replace(/^blob:/, "");
      const d = this.blobs.get(id);
      return d ? { uri: p.uri, data: d.toString("base64") } : null;
    }
    return super.rpc(method, p);
  }
  applyRegister(tx: Uint8Array): string {
    const b = Buffer.from(tx);
    const from = tx.subarray(5, 37);
    const nonce = b.readBigUInt64LE(37);
    const fee = b.readBigUInt64LE(45);
    const sig = tx.subarray(tx.length - 64);
    const signing = tx.subarray(0, tx.length - 64);
    if (!verifySig(from, frame(NS_TX, signing), sig)) throw new Error("bad tx signature");
    const a = b58encode(from);
    if (nonce !== (this.nonces.get(a) ?? 0n)) throw new Error("bad nonce");
    const bal = this.balances.get(a) ?? 0n;
    if (bal < fee || fee < BigInt(this.params.min_fee)) throw new Error("fee");
    this.balances.set(a, bal - fee);
    this.nonces.set(a, nonce + 1n);
    const nl = tx[54];
    const name = b.subarray(55, 55 + nl).toString("utf8");
    const ul = b.readUInt16LE(55 + nl);
    const uri = b.subarray(57 + nl, 57 + nl + ul).toString("utf8");
    this.agents.set(a, { account: a, name, uri, registered_height: this.height });
    const h = createHash("sha256").update(tx).digest("hex");
    this.txs.set(h, { status: "included", height: this.height });
    this.sent.push({ from: a, kind: "RegisterAgent", body: { name, uri }, hash: h });
    this.height++;
    return h;
  }
}

/** Fake Solana side behind the fake cadre-api: balances per address and the txs it accepted. */
export class FakeSol {
  lamports = new Map<string, bigint>();
  spl = new Map<string, bigint>();
  sent: { signature: string; builder: string; from: string; body: any }[] = [];
  pending = new Map<string, { builder: string; from: string; body: any }>();
  unsigned(from: string): string {
    const keys = [b58decode(from), new Uint8Array(32)];
    const msg = Uint8Array.from([1, 0, 1, 2, ...keys.flatMap((k) => [...k]), ...randomBytes(32), 1, 1, 1, 0, 0]);
    return Buffer.from(Uint8Array.from([1, ...new Uint8Array(64), ...msg])).toString("base64");
  }
}

export interface Fakes2 { node: Node2; gw: FakeGateway; sol: FakeSol; close: () => Promise<void> }

export async function startFakes2(model: Model): Promise<Fakes2> {
  const node = new Node2();
  const gw = new FakeGateway(model);
  const sol = new FakeSol();
  const builders: Record<string, (b: any) => { from: string; apply: () => void; summary: any }> = {
    "/v1/tx/pump-buy": (b) => ({ from: b.buyer, summary: { venue: "pumpswap", quote_tokens_out_spl: "123456" }, apply: () => { sol.lamports.set(b.buyer, (sol.lamports.get(b.buyer) ?? 0n) - BigInt(b.sol_in) - 5000n); sol.spl.set(b.buyer, (sol.spl.get(b.buyer) ?? 0n) + 123456n); } }),
    "/v1/tx/sol-transfer": (b) => ({ from: b.from, summary: { to: b.to }, apply: () => { const v = BigInt(b.lamports); sol.lamports.set(b.from, (sol.lamports.get(b.from) ?? 0n) - v - 5000n); sol.lamports.set(b.to, (sol.lamports.get(b.to) ?? 0n) + v); } }),
    "/v1/tx/spl-transfer": (b) => ({ from: b.from, summary: { to: b.to }, apply: () => { const v = BigInt(b.amount_spl); sol.spl.set(b.from, (sol.spl.get(b.from) ?? 0n) - v); sol.spl.set(b.to, (sol.spl.get(b.to) ?? 0n) + v); sol.lamports.set(b.from, (sol.lamports.get(b.from) ?? 0n) - 5000n); } }),
    // the bridge deposit: SPL leaves the owner; the relayer credits the recipient's Cadre account (1 SPL base unit = 1000 chain units here)
    "/v1/tx/bridge-deposit": (b) => ({ from: b.owner, summary: { recipient: b.recipient }, apply: () => { const v = BigInt(b.amount_spl); sol.spl.set(b.owner, (sol.spl.get(b.owner) ?? 0n) - v); sol.lamports.set(b.owner, (sol.lamports.get(b.owner) ?? 0n) - 5000n); node.balances.set(b.recipient, (node.balances.get(b.recipient) ?? 0n) + v * 1000n); } }),
    "/v1/tx/sol-memo": (b) => ({ from: b.from, summary: { memo: b.memo }, apply: () => { sol.lamports.set(b.from, (sol.lamports.get(b.from) ?? 0n) - 5000n); } }),
  };
  const applyLater = new Map<string, { builder: string; from: string; body: any; apply: () => void }>();
  const servers = [
    await serve(PORTS.node, async (_m, _u, _h, body) => {
      const j = JSON.parse(body);
      try { return { status: 200, body: { jsonrpc: "2.0", id: j.id, result: await node.rpc(j.method, j.params ?? {}) } }; } catch (e) { return { status: 200, body: { jsonrpc: "2.0", id: j.id, error: { code: -32000, message: String((e as Error).message) } } }; }
    }),
    await serve(PORTS.gateway, (m, u, h, b) => gw.http(m, u, h, b)),
    await serve(PORTS.api, async (m, url, _h, body) => {
      const path = url.split("?")[0];
      if (path.startsWith("/v1/town")) {
        const posts = [...node.posts].reverse().slice(0, 60).map((p) => ({ ...p, author: { account: p.author, handle: node.profiles.get(p.author)?.handle ?? null } }));
        return { status: 200, body: { posts, pending: [], next: null } };
      }
      let mm = /^\/v1\/accounts\/([^/]+)\/balances$/.exec(path);
      if (mm) return { status: 200, body: { address: mm[1], chain: { balance: (node.balances.get(mm[1]) ?? 0n).toString() }, solana: { lamports: (sol.lamports.get(mm[1]) ?? 0n).toString(), spl: (sol.spl.get(mm[1]) ?? 0n).toString() } } };
      if (m === "POST" && builders[path]) {
        const b = JSON.parse(body);
        const x = builders[path](b);
        const tx = sol.unsigned(x.from);
        applyLater.set(tx.slice(-60), { builder: path, from: x.from, body: b, apply: x.apply });
        return { status: 200, body: { transaction: tx, summary: x.summary } };
      }
      if (m === "POST" && path === "/v1/tx/solana") {
        const tx = Buffer.from(JSON.parse(body).tx, "base64");
        const msg = tx.subarray(65);
        const signer = msg.subarray(4, 36);
        if (!verifySig(signer, msg, tx.subarray(1, 65))) return { status: 400, body: { error: { message: "bad signature" } } };
        const key = Buffer.from(Uint8Array.from([1, ...new Uint8Array(64), ...msg])).toString("base64").slice(-60);
        const p = applyLater.get(key);
        if (!p) return { status: 400, body: { error: { message: "unknown tx" } } };
        p.apply();
        const signature = b58encode(tx.subarray(1, 65));
        sol.sent.push({ signature, builder: p.builder, from: p.from, body: p.body });
        return { status: 200, body: { signature } };
      }
      if (m === "GET" && (path === "/v1/strategies" || path.startsWith("/v1/strategies/"))) return { status: 200, body: { fake: "strategies", url } };
      mm = /^\/v1\/tx\/solana\/([^/]+)$/.exec(path);
      if (mm) return { status: 200, body: { signature: mm[1], status: "confirmed", ok: true, err: null } };
      return { status: 404, body: { error: { message: `fake api: no ${m} ${path}` } } };
    }),
    await serve(PORTS.sol, async () => ({ status: 200, body: { jsonrpc: "2.0", id: 1, result: { value: { blockhash: b58encode(randomBytes(32)) } } } })),
  ];
  return { node, gw, sol, close: async () => { for (const s of servers) await new Promise((r) => s.close(() => r(null))); } };
}
