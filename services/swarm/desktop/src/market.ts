// The house desktops' side of the services market (docs/MARKET.md): offers, hires, deliveries,
// verdicts and refunds, signed by this controller with each agent's own key (no browser wallet:
// the desktops only browse). One signer per agent here, one call at a time per agent (a mutex), so
// nonces never race. Request, delivery and verdict documents are node blobs (cadre-api keeps a copy
// of every market blob it reads) and a local copy under the controller's state dir.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { MarketIndex, marketArgs, type Hire, type Offer } from "../../../../apps/explorer/src/market.ts";
import type { Wallet } from "../../src/wallet.ts";
import { blake3 } from "../../src/blake3.ts";

export const CADRE = 1_000_000_000n;

/** What the market needs from a Cadre client (trading/src/cadre.ts implements it; tests fake it). */
export interface ChainClient {
  rpc: { call(method: string, params?: unknown): Promise<any>; putBlob(data: Uint8Array, push?: boolean): Promise<any> };
  sendContract(w: Wallet, b: { kind: "Call"; contract: string; value: bigint; gasLimit: bigint; args: Uint8Array }): Promise<{ txHash: string }>;
  waitIncluded(txHash: string, timeoutMs?: number, pollMs?: number): Promise<{ height: number; receipt: any }>;
}

export interface OfferDef { title: string; category: string; price: bigint; turnaround: number; description: string; deliverable: string }

export const RESEARCH_OFFER: OfferDef = {
  title: "Sourced web research brief",
  category: "research",
  price: CADRE / 5n, // 0.2 CADRE: a house price (design choice), editable on chain with update_offer
  turnaround: 240, // blocks, about 2 h at the 30 s target
  description: "Send a question; get a short brief written from at least two independent web pages opened in a real browser, with an exact quote and link for every claim. Reviewed by a house source checker.",
  deliverable: "A cadre-delivery-v1 blob: title, summary, three to six findings (claim, source URL, quote), and the check of every quote against the page the agent opened.",
};

const errText = (hex: string | null | undefined) => (hex ? Buffer.from(hex, "hex").toString("utf8") : "");

export class MarketOps {
  c: ChainClient;
  ix: MarketIndex;
  contract: string;
  blobDir: string;
  log: (e: Record<string, unknown>) => void;
  private locks = new Map<string, Promise<unknown>>();
  constructor(c: ChainClient, contract: string, blobDir: string, log: (e: Record<string, unknown>) => void) {
    this.c = c;
    this.contract = contract;
    this.blobDir = blobDir;
    this.log = log;
    mkdirSync(blobDir, { recursive: true });
    this.ix = new MarketIndex(contract, (m, p) => c.rpc.call(m, p));
  }

  async refresh(maxAgeMs = 20_000) { await this.ix.refresh(maxAgeMs); return this.ix; }
  get height() { return this.ix.height; }
  offers(): Offer[] { return this.ix.offers; }
  hires(): Hire[] { return [...this.ix.hires.values()]; }

  /** One call at a time per wallet. */
  private serial<T>(addr: string, f: () => Promise<T>): Promise<T> {
    const prev = this.locks.get(addr) ?? Promise.resolve();
    const next = prev.catch(() => {}).then(f);
    this.locks.set(addr, next);
    return next;
  }

  async call(w: Wallet, handle: string, what: string, args: Uint8Array, value = 0n): Promise<{ tx: string; height: number; ret: string | null }> {
    return this.serial(w.address, async () => {
      const s = await this.c.sendContract(w, { kind: "Call", contract: this.contract, value, gasLimit: 0n, args });
      const r = await this.c.waitIncluded(s.txHash, 900_000, 2_000);
      const ok = r.receipt ? r.receipt.ok !== false : true;
      this.log({ event: `market_${what}`, agent: handle, tx: s.txHash, height: r.height, ok, error: ok ? undefined : errText(r.receipt?.error) || r.receipt?.error });
      if (!ok) throw new Error(`${what} failed: ${errText(r.receipt?.error) || "receipt not ok"}`);
      return { tx: s.txHash, height: r.height, ret: r.receipt?.return ?? null };
    });
  }

  /** Stores a JSON document as a node blob (and locally); returns its uri and blake3 id. */
  async blob(json: unknown): Promise<{ uri: string; id: string }> {
    const data = Buffer.from(JSON.stringify(json));
    const id = Buffer.from(blake3(data)).toString("hex");
    writeFileSync(join(this.blobDir, `${id}.json`), data);
    const put = await this.c.rpc.putBlob(data, true);
    return { uri: String(put?.uri ?? `blob:${id}`), id: String(put?.blob_id ?? put?.uri ?? id).replace(/^blob:/, "") };
  }

  /** A blob by uri: the local copy first, then the node. */
  async getBlob(uri: string): Promise<any | null> {
    const id = uri.replace(/^blob:/, "");
    const f = join(this.blobDir, `${id}.json`);
    try {
      if (existsSync(f)) return JSON.parse(readFileSync(f, "utf8"));
      const b = await this.c.rpc.call("getBlob", { blob_id: uri });
      if (!b?.data) return null;
      const raw = Buffer.from(b.data, "base64");
      if (Buffer.from(blake3(raw)).toString("hex") !== id) return null;
      return JSON.parse(raw.toString("utf8"));
    } catch { return null; }
  }

  /** The provider's offer with this title, listing it when it is missing. */
  async ensureOffer(w: Wallet, handle: string, o: OfferDef): Promise<Offer | null> {
    await this.refresh(0);
    const have = this.ix.offers.find((x) => x.provider === w.address && x.title === o.title);
    if (have) return have;
    const spec = await this.blob({ format: "cadre-offer-v1", title: o.title, category: o.category, description: o.description, deliverable: o.deliverable, turnaround_blocks: o.turnaround, evaluator_policy: "the buyer names the evaluators; house buyers name a house source reviewer", provider: w.address, house: true, inputs: "a question in plain text, optionally with links", outputs: o.deliverable });
    await this.call(w, handle, "offer", marketArgs.offer({ title: o.title, category: o.category, spec_uri: spec.uri, spec_hash: spec.id, price: o.price, turnaround: o.turnaround, policy: 0, evaluators: [], active: true }));
    await this.refresh(0);
    return this.ix.offers.find((x) => x.provider === w.address && x.title === o.title) ?? null;
  }

  async hire(buyer: Wallet, handle: string, offer: Offer, request: Record<string, unknown>, evaluator: string, judgeFee: bigint, deadlineBlocks: number): Promise<{ id: number | null; tx: string; total: bigint }> {
    const q = await this.ix.quote(offer.id, judgeFee);
    const req = await this.blob({ format: "cadre-request-v1", offer: offer.id, title: offer.title, buyer: buyer.address, house: true, ...request });
    const total = BigInt(q.total);
    const r = await this.call(buyer, handle, "hire", marketArgs.hire({ offer: offer.id, price: q.price, request_uri: req.uri, request_hash: req.id, deadline_blocks: deadlineBlocks, judge_fee: judgeFee, evaluators: [evaluator], threshold: 1 }), total);
    const id = r.ret ? Number(Buffer.from(r.ret, "hex").readBigUInt64LE(0)) : null;
    return { id, tx: r.tx, total };
  }

  async deliver(w: Wallet, handle: string, hire: number, doc: Record<string, unknown>, thoughtRef = ""): Promise<{ tx: string; uri: string; id: string }> {
    const b = await this.blob({ format: "cadre-delivery-v1", hire, provider: w.address, house: true, ...doc });
    const r = await this.call(w, handle, "deliver", marketArgs.deliver(hire, b.id, b.uri, thoughtRef));
    return { tx: r.tx, ...b };
  }

  async verdict(w: Wallet, handle: string, hire: number, accept: boolean, doc: Record<string, unknown>): Promise<{ tx: string; uri: string }> {
    const b = await this.blob({ format: "cadre-verdict-v1", hire, evaluator: w.address, accept, house: true, method: "source check (no language model)", ...doc });
    const r = await this.call(w, handle, "verdict", marketArgs.verdict(hire, accept, b.uri, b.id, ""));
    return { tx: r.tx, uri: b.uri };
  }

  async decline(w: Wallet, handle: string, hire: number, reason: string) { return this.call(w, handle, "decline", marketArgs.decline(hire, reason.slice(0, 240))); }
  async expire(w: Wallet, handle: string, hire: number) { return this.call(w, handle, "expire", marketArgs.expire(hire)); }
}

export const sha256hex = (s: string) => createHash("sha256").update(s).digest("hex");
