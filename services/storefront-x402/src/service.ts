// storefront-x402 (SPEC 5.2): prices a job, takes an x402 "exact" payment on
// Solana, calls storefront open_order + buy_job (or buy_job_amm after
// graduation) as the order's buyer, and serves the Order (and its result once
// finalized). Transport-free: `handle()` maps a request to a response, so the
// tests drive it directly and src/main.ts wraps it in node:http.
//
// The service wallet is the on-chain `buyer` of every order it places; the
// x402 payer's key is the order's agent-chain `recipient` (the job poster,
// same ed25519 key on both chains) unless the request names another one. So
// an expired job's W lands on the payer's own chain account.

import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import type { ChainIO } from "./chain.ts";
import * as sf from "./storefront.ts";
import { decodePaymentHeader, PaymentError, verifyExact, X402_VERSION } from "./x402.ts";
import type { PaymentRequirements } from "./x402.ts";

export interface Pricing {
  /** Flat part of sol_in per order. */
  baseLamports: bigint;
  /** sol_in per 1e9 multiply-adds x2 (flops) x max_blocks. */
  lamportsPerGflop: bigint;
  /** On top of sol_in: Order + Deposit rent and tx fees the service pays. */
  overheadLamports: bigint;
}

export interface ServiceOptions {
  chain: ChainIO;
  wallet: Keypair;
  network: string;
  publicUrl: string;
  pricing: Pricing;
  addresses?: ReturnType<typeof sf.addresses>;
  stateFile?: string;
  maxCuPriceMicroLamports?: bigint;
}

export interface HttpReq {
  method: string;
  url: string;
  headers: Record<string, string | undefined>;
  body?: string;
}
export interface HttpRes {
  status: number;
  headers: Record<string, string>;
  body: unknown;
}

interface PaymentRecord {
  payer: string;
  amount: string;
  signature: string;
  at: number;
  nonce?: string;
  refund?: string;
  error?: string;
}
interface State {
  payments: Record<string, PaymentRecord>;
}

const json = (status: number, body: unknown, headers: Record<string, string> = {}): HttpRes => ({ status, headers: { "content-type": "application/json", ...headers }, body });
export const toJson = (v: unknown) => JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x));

class BadRequest extends Error {}

function hexBytes(v: unknown, len: number, name: string): Buffer {
  if (typeof v !== "string" || !/^[0-9a-fA-F]*$/.test(v) || v.length !== len * 2) throw new BadRequest(`${name} must be ${len} bytes hex`);
  return Buffer.from(v, "hex");
}
function int(v: unknown, name: string, min = 1, max = 2 ** 32 - 1): number {
  const n = typeof v === "string" ? Number(v) : v;
  if (typeof n !== "number" || !Number.isInteger(n) || n < min || n > max) throw new BadRequest(`${name} must be an integer in [${min}, ${max}]`);
  return n;
}
function key(v: unknown, name: string): PublicKey {
  try {
    return new PublicKey(v as string);
  } catch {
    throw new BadRequest(`${name} is not a base58 public key`);
  }
}

export class Service {
  readonly o: ServiceOptions;
  readonly a: ReturnType<typeof sf.addresses>;
  private state: State;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(o: ServiceOptions) {
    this.o = o;
    this.a = o.addresses ?? sf.addresses();
    this.state = o.stateFile && existsSync(o.stateFile) ? JSON.parse(readFileSync(o.stateFile, "utf8")) : { payments: {} };
  }

  private save() {
    const f = this.o.stateFile;
    if (!f) return;
    mkdirSync(dirname(f), { recursive: true });
    writeFileSync(f + ".tmp", JSON.stringify(this.state, null, 1));
    renameSync(f + ".tmp", f);
  }

  /** Serialises chain work (the Order PDA is keyed by the bridge nonce). */
  private exclusive<T>(f: () => Promise<T>): Promise<T> {
    const run = this.queue.then(f, f);
    this.queue = run.catch(() => undefined);
    return run;
  }

  async handle(req: HttpReq): Promise<HttpRes> {
    try {
      const u = new URL(req.url, "http://x");
      if (req.method === "GET" && u.pathname === "/health") return json(200, { ok: true, wallet: this.o.wallet.publicKey.toBase58(), network: this.o.network });
      if (req.method === "GET" && u.pathname === "/quote") return json(200, await this.quote(Object.fromEntries(u.searchParams)));
      if (req.method === "POST" && u.pathname === "/jobs") return await this.postJob(req);
      const m = u.pathname.match(/^\/jobs\/([^/]+)$/);
      if (req.method === "GET" && m) return await this.getJob(m[1]);
      return json(404, { error: "not found" });
    } catch (e) {
      if (e instanceof BadRequest) return json(400, { error: e.message });
      if (/fetch failed|ECONNREFUSED/.test((e as Error).message)) return json(503, { error: "token chain RPC unreachable" });
      return json(500, { error: (e as Error).message });
    }
  }

  // ------------------------------------------------------------ chain reads

  private async storeConfig(): Promise<sf.StoreConfig> {
    const [c] = await this.o.chain.getAccounts([this.a.config]);
    if (!c) throw new Error("storefront config not initialized");
    return sf.decodeConfig(c.data);
  }

  /** Venue, pre-trade quote and the accounts buy_job needs. */
  private async market(cfg: sf.StoreConfig, solIn: bigint) {
    const mint = cfg.mint;
    const bc = sf.bondingCurvePda(mint);
    const pool = sf.canonicalPool(mint);
    const [curveAcc, globalAcc, ammCfgAcc, poolAcc] = await this.o.chain.getAccounts([
      bc, sf.pda([Buffer.from("global")], sf.PUMP), sf.pda([Buffer.from("global_config")], sf.AMM), pool,
    ]);
    if (!curveAcc || !globalAcc) throw new Error("pump coin or pump Global missing");
    const curve = sf.decodeCurve(curveAcc.data);
    const p: sf.PumpAccounts = { mint, curveBuyback: sf.pumpGlobalBuyback(globalAcc.data), ammBuyback: ammCfgAcc ? sf.ammGlobalBuyback(ammCfgAcc.data) : sf.pumpGlobalBuyback(globalAcc.data) };
    let quote: bigint;
    let venue: "curve" | "pumpswap" = "curve";
    if (!curve.complete) {
      quote = sf.cpQuote(curve.virtualToken, curve.virtualQuote, solIn);
    } else {
      if (!poolAcc) throw new Error("curve complete but PumpSwap pool missing");
      venue = "pumpswap";
      const pl = sf.decodePool(poolAcc.data);
      p.poolBaseVault = pl.baseVault;
      p.poolQuoteVault = pl.quoteVault;
      const [bv, qv] = await this.o.chain.getAccounts([pl.baseVault, pl.quoteVault]);
      if (!bv || !qv) throw new Error("pool vaults missing");
      let q = sf.tokenAmount(qv.data) - pl.protocolFees - pl.creatorFees;
      if (q < 0n) q = 0n;
      quote = sf.cpQuote(sf.tokenAmount(bv.data), q + pl.virtualQuote, solIn);
    }
    const minOut = (quote * BigInt(10_000 - cfg.maxSlippageBps)) / 10_000n;
    return { venue, quote, minOut: minOut > 0n ? minOut : 1n, accounts: p };
  }

  // ------------------------------------------------------------ pricing

  price(cfg: sf.StoreConfig, m: number, n: number, k: number, maxBlocks: number) {
    const flops = 2n * BigInt(m) * BigInt(n) * BigInt(k) * BigInt(maxBlocks);
    const work = (flops * this.o.pricing.lamportsPerGflop + 999_999_999n) / 1_000_000_000n;
    let solIn = this.o.pricing.baseLamports + work;
    if (solIn < cfg.minSolIn) solIn = cfg.minSolIn;
    if (solIn > cfg.maxSolIn) throw new BadRequest(`job needs ${solIn} lamports, above the per-order cap ${cfg.maxSolIn}`);
    return { flops, solIn, price: solIn + this.o.pricing.overheadLamports };
  }

  private checkDims(cfg: sf.StoreConfig, q: Record<string, unknown>) {
    const m = int(q.m, "m", 1, cfg.maxDim), n = int(q.n, "n", 1, cfg.maxDim), k = int(q.k, "k", 1, cfg.maxDim);
    const maxBlocks = int(q.max_blocks ?? 1, "max_blocks", 1, cfg.maxBlocksCap);
    return { m, n, k, maxBlocks };
  }

  async quote(q: Record<string, unknown>) {
    const cfg = await this.storeConfig();
    const { m, n, k, maxBlocks } = this.checkDims(cfg, q);
    const pr = this.price(cfg, m, n, k, maxBlocks);
    const mk = await this.market(cfg, pr.solIn);
    const p = (mk.quote * BigInt(cfg.protocolFeeBps)) / 10_000n;
    return {
      m, n, k, max_blocks: maxBlocks,
      work_flops: pr.flops,
      sol_in_lamports: pr.solIn,
      overhead_lamports: this.o.pricing.overheadLamports,
      price_lamports: pr.price,
      venue: mk.venue,
      estimate: {
        note: "fee-free constant-product quote before pump fees; the fill is lower by pump's fee (1.25% on the curve) and bounded by max_slippage_bps",
        tokens_spl: mk.quote,
        min_tokens_out_spl: mk.minOut,
        worker_pay_chain: (mk.quote - p) * 1000n,
        protocol_fee_chain: p * 1000n,
        protocol_fee_bps: cfg.protocolFeeBps,
      },
      payment: { scheme: "exact", network: this.o.network, asset: "SOL (native)", pay_to: this.o.wallet.publicKey.toBase58() },
    };
  }

  requirements(price: bigint): PaymentRequirements {
    return {
      scheme: "exact",
      network: this.o.network,
      maxAmountRequired: price.toString(),
      resource: `${this.o.publicUrl}/jobs`,
      description: "pearl-sol AI job: the payment buys the token, bridges W + P to the agent chain and posts the job",
      mimeType: "application/json",
      payTo: this.o.wallet.publicKey.toBase58(),
      maxTimeoutSeconds: 120,
      asset: sf.WSOL.toBase58(),
      extra: { feePayer: this.o.wallet.publicKey.toBase58(), assetKind: "native", decimals: 9 },
    };
  }

  // ------------------------------------------------------------ jobs

  private parseSpec(cfg: sf.StoreConfig, b: Record<string, unknown>): sf.JobSpec {
    const s = (b.spec ?? {}) as Record<string, unknown>;
    const { m, n, k, maxBlocks } = this.checkDims(cfg, s);
    const dataUri = s.data_uri ?? "";
    if (typeof dataUri !== "string" || Buffer.byteLength(dataUri) > sf.MAX_URI) throw new BadRequest("data_uri must be a string of at most 200 bytes");
    const deadlineBlocks = BigInt(int(s.deadline_blocks, "deadline_blocks", 1, Number(cfg.maxDeadlineBlocks)));
    const evaluators = s.evaluators === undefined ? cfg.attesters.slice(0, sf.MAX_EVALUATORS) : (s.evaluators as unknown[]).map((e, i) => key(e, `evaluators[${i}]`));
    if (evaluators.length < 1 || evaluators.length > sf.MAX_EVALUATORS) throw new BadRequest("1 to 8 evaluators");
    const threshold = s.threshold === undefined ? Math.min(cfg.threshold, evaluators.length) : int(s.threshold, "threshold", 1, evaluators.length);
    return {
      m, n, k, maxBlocks, dataUri, deadlineBlocks, evaluators, threshold,
      config: hexBytes(s.config, 52, "config"), hashA: hexBytes(s.hash_a, 32, "hash_a"), hashB: hexBytes(s.hash_b, 32, "hash_b"),
    };
  }

  private async postJob(req: HttpReq): Promise<HttpRes> {
    let body: Record<string, unknown>;
    try {
      body = JSON.parse(req.body || "{}");
    } catch {
      throw new BadRequest("body is not JSON");
    }
    const cfg = await this.storeConfig();
    if (cfg.paused) return json(503, { error: "storefront paused" });
    const spec = this.parseSpec(cfg, body);
    const { solIn, price } = this.price(cfg, spec.m, spec.n, spec.k, spec.maxBlocks);
    const reqs = this.requirements(price);
    const header = req.headers["x-payment"];
    const need = (error: string) => json(402, { x402Version: X402_VERSION, error, accepts: [reqs] });
    if (!header) return need("X-PAYMENT header is required");

    let paid;
    try {
      paid = verifyExact(decodePaymentHeader(header), reqs, this.o.maxCuPriceMicroLamports);
    } catch (e) {
      if (e instanceof PaymentError) return need(e.message);
      throw e;
    }
    const prior = this.state.payments[paid.id];
    if (prior) return json(409, { error: "payment already used", payment: prior, job: prior.nonce ? `/jobs/${prior.nonce}` : null });
    const recipient = body.recipient === undefined ? paid.payer : key(body.recipient, "recipient");

    return this.exclusive(async () => {
      if (this.state.payments[paid.id]) return json(409, { error: "payment already used" });
      // Settle: co-sign as fee payer when asked, send, confirm.
      if (paid.needsFeePayerSig) paid.tx.sign([this.o.wallet]);
      let signature: string;
      try {
        signature = await this.o.chain.sendRaw(paid.tx.serialize());
      } catch (e) {
        return need(`settlement failed: ${(e as Error).message.slice(0, 300)}`);
      }
      const rec: PaymentRecord = { payer: paid.payer.toBase58(), amount: paid.amount.toString(), signature, at: Date.now() };
      this.state.payments[paid.id] = rec;
      this.save();
      const xpr = Buffer.from(JSON.stringify({ success: true, transaction: signature, network: this.o.network, payer: rec.payer })).toString("base64");

      try {
        const placed = await this.place(cfg, spec, recipient, solIn);
        rec.nonce = placed.nonce.toString();
        this.save();
        return json(201, { id: rec.nonce, order: placed.order.toBase58(), job_id: placed.jobId, status: "pending", venue: placed.venue, txs: placed.txs, payment: rec, get: `/jobs/${rec.nonce}` }, { "x-payment-response": xpr });
      } catch (e) {
        // Payment taken but no order: give it back.
        rec.error = (e as Error).message.slice(0, 500);
        try {
          rec.refund = await this.o.chain.send([SystemProgram.transfer({ fromPubkey: this.o.wallet.publicKey, toPubkey: paid.payer, lamports: paid.amount })], this.o.wallet);
        } catch (e2) {
          rec.refund = `FAILED: ${(e2 as Error).message.slice(0, 200)}`;
        }
        this.save();
        return json(502, { error: "order failed, payment refunded", payment: rec }, { "x-payment-response": xpr });
      }
    });
  }

  private async place(cfg: sf.StoreConfig, spec: sf.JobSpec, recipient: PublicKey, solIn: bigint) {
    const w = this.o.wallet;
    const draftId = randomBytes(8).readBigUInt64LE(0);
    const txs: string[] = [await this.o.chain.send([sf.openOrderIx(w.publicKey, draftId, recipient, spec, this.a)], w)];
    let lastErr: Error | null = null;
    for (let attempt = 0; attempt < 3; attempt++) {
      const [bc] = await this.o.chain.getAccounts([this.a.bridgeConfig]);
      if (!bc) throw new Error("bridge config missing");
      const nonce = sf.bridgeNonce(bc.data);
      const mk = await this.market(cfg, solIn);
      const ix = mk.venue === "curve"
        ? sf.buyJobIx(w.publicKey, draftId, nonce, solIn, mk.minOut, mk.accounts, this.a)
        : sf.buyJobAmmIx(w.publicKey, draftId, nonce, solIn, mk.minOut, mk.accounts, this.a);
      try {
        txs.push(await this.o.chain.send([ix], w));
        return { nonce, order: sf.orderPda(nonce, this.a.storefront), jobId: sf.orderJobId(nonce).toString("hex"), venue: mk.venue, txs };
      } catch (e) {
        lastErr = e as Error;
        // Another deposit took the nonce between read and send (WrongNonce 6009 = 0x1779): retry.
        if (!/WrongNonce|0x1779|already in use/.test(lastErr.message)) break;
      }
    }
    try {
      await this.o.chain.send([sf.cancelDraftIx(w.publicKey, draftId, this.a)], w);
    } catch {
      /* draft rent stays; harmless */
    }
    throw lastErr ?? new Error("buy_job failed");
  }

  private async getJob(id: string): Promise<HttpRes> {
    let addr: PublicKey;
    if (/^\d+$/.test(id)) addr = sf.orderPda(BigInt(id), this.a.storefront);
    else addr = key(id, "job id");
    const [acc] = await this.o.chain.getAccounts([addr]);
    if (!acc || !acc.owner.equals(this.a.storefront)) return json(404, { error: "no such order" });
    const o = sf.decodeOrder(acc.data);
    return json(200, { id: o.nonce.toString(), order: addr.toBase58(), ...o });
  }
}
