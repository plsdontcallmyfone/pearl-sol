// Read-only indexer: every canonical agent-chain block (summary + the txs the explorer
// links on), the chain's jobs, the storefront's Orders with the Solana txs seen for them,
// the light client's progress, and bridge withdrawals with their Solana release.
//
// Solana RPCs prune history (the local test validator keeps about 1,500 slots), so the
// indexer records Solana signatures as soon as it sees an account change and persists
// them; what it never saw is reported as "not recorded", never guessed.

import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { b58encode } from "./codec.ts";
import * as D from "./decode.ts";
import { JsonRpc, SolanaRpc, chunkedBatch } from "./rpc.ts";

export interface TxRec {
  hash: string;
  kind: string;
  from: string;
  fee: string;
  facts: D.TxFacts;
}

export interface BlockRec {
  height: number;
  hash: string;
  prev: string;
  time: number;
  miner: string;
  jobId: string | null; // null = fallback block
  fees: string;
  subsidy: string;
  reward: string;
  proofHash: string;
  proofLen: number;
  targetBits: number;
  txs: TxRec[];
}

export interface SolTx {
  sig: string;
  slot: number;
  time: number | null;
  ok: boolean;
  ix: string[]; // Anchor instruction names for the program of interest
  buyback?: { executed: D.BuybackExecuted[]; accrued: D.BuybackAccrued[] };
  /** Reserve cap split of the buys in this tx (BuybackToReserve / BuybackBurned, 2026-10-19). */
  parts?: D.BuybackPart[];
  /** buy_job* txs: the order's SOL in (OrderPlaced sol_spent + BuybackShareHeld lamports). */
  paid?: { nonce: string; solSpent: string; share: string }[];
}

export interface OrderRec extends D.Order {
  txs: SolTx[];
  /** x402 payment: a System transfer from the chain recipient (the payer) to the buyer wallet before the order. */
  payment: { sig: string; slot: number; lamports: string } | null;
  paymentChecked: boolean;
  firstSeen: number;
  sigsFetchedFor: string; // status the signatures were last fetched for
}

export interface WithdrawalRec {
  nonce: number;
  from: string;
  solana_recipient: string;
  amount_spl: string;
  height: number;
  block_hash: string;
  final: boolean;
  released: boolean;
  releaseTxs: SolTx[];
  releaseChecked: boolean;
}

export interface LcSnapshot {
  config: D.LcConfig | null;
  acc: D.LcAccumulator | null;
}

export interface IndexerOptions {
  nodeUrl: string;
  solUrl: string;
  stateDir: string;
  pollMs?: number;
  /** Solana side at most this often (default: every poll). A public RPC such as Solana devnet
   * allows about 100 calls per 10 s per IP, shared by every service on the box. */
  solPollMs?: number;
  /** blocks fetched per poll while backfilling */
  backfillPerPoll?: number;
  log?: (msg: string) => void;
}

const STATE_VERSION = 3; // 2: tx facts carry signers / deliverer / to / name / uri (agent profiles); 3: DM facts (dm.ts)

export class Indexer {
  private lastSolPoll = 0;
  o: Required<IndexerOptions>;
  node: JsonRpc;
  sol: SolanaRpc;

  genesis: string | null = null;
  chainInfo: any = null;
  params: any = null;
  blocks = new Map<number, BlockRec>();
  byHash = new Map<string, number>();
  jobs = new Map<string, any>();
  jobBlocks = new Map<string, number[]>();
  orders = new Map<string, OrderRec>();
  withdrawals = new Map<number, WithdrawalRec>();
  lc: LcSnapshot = { config: null, acc: null };
  /** Storefront buyback totals (Config) when the deployed program has them, and the buy txs seen. */
  buyback: D.StoreBuyback | null = null;
  buybackTxs: SolTx[] = [];
  /** Light-client txs per block hash, captured while the Solana ledger still had them. */
  lcTxs = new Map<string, SolTx[]>();
  backfillCursor = 0; // next height to fetch going down (0 = done)
  status = { nodeOk: false, solOk: false, nodeError: "", solError: "", lastPoll: 0, polls: 0, resets: 0 };

  private dirty = false;
  private lastSave = 0;
  private timer: NodeJS.Timeout | null = null;
  private running = false;
  private listeners = new Set<() => void>();

  constructor(opts: IndexerOptions) {
    this.o = { pollMs: 3000, solPollMs: 0, backfillPerPoll: 2000, log: (m) => console.log(m), ...opts };
    this.node = new JsonRpc(opts.nodeUrl);
    this.sol = new SolanaRpc(opts.solUrl);
  }

  onUpdate(f: () => void) {
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }

  start() {
    const loop = async () => {
      if (!this.running) return;
      await this.poll();
      if (this.running) this.timer = setTimeout(loop, this.o.pollMs);
    };
    this.running = true;
    loop();
  }

  stop() {
    this.running = false;
    if (this.timer) clearTimeout(this.timer);
    this.save(true);
  }

  async poll() {
    try {
      await this.pollChain();
      this.status.nodeOk = true;
      this.status.nodeError = "";
    } catch (e) {
      this.status.nodeOk = false;
      this.status.nodeError = (e as Error).message;
    }
    if (this.genesis && Date.now() - this.lastSolPoll >= this.o.solPollMs) {
      this.lastSolPoll = Date.now();
      try {
        await this.pollSolana();
        this.status.solOk = true;
        this.status.solError = "";
      } catch (e) {
        this.status.solOk = false;
        this.status.solError = (e as Error).message;
      }
    }
    this.status.lastPoll = Date.now();
    this.status.polls++;
    this.save(false);
    for (const f of this.listeners) f();
  }

  // ------------------------------------------------------------ chain

  private reset(genesis: string) {
    this.genesis = genesis;
    this.blocks.clear();
    this.byHash.clear();
    this.jobs.clear();
    this.jobBlocks.clear();
    this.orders.clear();
    this.withdrawals.clear();
    this.lcTxs.clear();
    this.buyback = null;
    this.buybackTxs = [];
    this.backfillCursor = 0;
    this.status.resets++;
    if (!this.load(genesis)) this.o.log(`explorer: new chain ${genesis.slice(0, 16)}, indexing from scratch`);
  }

  async pollChain() {
    const [info, params] = await this.node.batch<any>([
      ["getChainInfo", {}],
      ["getParams", {}],
    ]);
    if (info instanceof Error) throw info;
    if (!(params instanceof Error)) this.params = params;
    if (info.genesis_hash !== this.genesis) {
      if (this.genesis) this.o.log(`explorer: genesis changed (${this.genesis.slice(0, 16)} -> ${info.genesis_hash.slice(0, 16)}): the stack was restarted, re-indexing`);
      this.reset(info.genesis_hash);
    }
    this.chainInfo = info;
    const tip: number = info.height;
    const K: number = info.finality_k ?? 30;

    // drop heights above a shorter canonical tip
    for (const h of [...this.blocks.keys()]) if (h > tip) this.dropBlock(h);

    // the top: re-read everything not final (reorg-safe), plus new blocks
    let known = 0;
    for (const h of this.blocks.keys()) if (h > known) known = h;
    const startTop = Math.max(1, Math.min(known, tip) - K - 2);
    const firstRun = this.blocks.size === 0;
    const topFrom = firstRun ? Math.max(1, tip - 299) : startTop;
    const newTop = Math.max(topFrom, tip - 4999); // never more than 5000 in one go
    await this.fetchRange(newTop, tip);
    if (firstRun || this.backfillCursor === 0) {
      // find the highest missing height below the top window
      let h = newTop - 1;
      while (h >= 1 && this.blocks.has(h)) h--;
      this.backfillCursor = h;
    }
    // backfill downwards
    if (this.backfillCursor >= 1) {
      const lo = Math.max(1, this.backfillCursor - this.o.backfillPerPoll + 1);
      await this.fetchRange(lo, this.backfillCursor);
      let h = lo - 1;
      while (h >= 1 && this.blocks.has(h)) h--;
      this.backfillCursor = h;
    }

    // jobs (paged)
    const jobs: any[] = [];
    for (let offset = 0; ; offset += 200) {
      const r = await this.node.call<any>("listJobs", { offset, limit: 200 });
      jobs.push(...r.jobs);
      if (jobs.length >= r.total || r.jobs.length === 0) break;
    }
    this.jobs.clear();
    for (const j of jobs) this.jobs.set(j.job_id, j);
    this.rebuildJobBlocks();

    // bridge withdrawals (for release / release_proof on Solana)
    const ev = await this.node.call<any>("getBridgeEvents", { from_nonce: 0, limit: 1000 });
    for (const w of ev.withdrawals ?? []) {
      const prev = this.withdrawals.get(w.nonce);
      this.withdrawals.set(w.nonce, {
        nonce: w.nonce, from: w.from, solana_recipient: w.solana_recipient, amount_spl: w.amount_spl, height: w.height,
        block_hash: w.block_hash, final: w.final,
        released: prev?.released ?? false, releaseTxs: prev?.releaseTxs ?? [], releaseChecked: prev?.releaseChecked ?? false,
      });
    }
  }

  private dropBlock(h: number) {
    const b = this.blocks.get(h);
    if (b) this.byHash.delete(b.hash);
    this.blocks.delete(h);
    this.dirty = true;
  }

  private async fetchRange(lo: number, hi: number) {
    if (hi < lo) return;
    const calls: [string, unknown][] = [];
    for (let h = lo; h <= hi; h++) calls.push(["getBlock", { height: h }]);
    const res = await chunkedBatch<any>(this.node, calls, 100, 4);
    for (const r of res) if (!(r instanceof Error) && r) this.putBlock(toBlockRec(r));
  }

  putBlock(b: BlockRec) {
    const old = this.blocks.get(b.height);
    if (old && old.hash === b.hash) return;
    if (old) this.byHash.delete(old.hash);
    this.blocks.set(b.height, b);
    this.byHash.set(b.hash, b.height);
    this.dirty = true;
  }

  rebuildJobBlocks() {
    this.jobBlocks.clear();
    for (const b of this.blocks.values()) {
      if (!b.jobId) continue;
      const l = this.jobBlocks.get(b.jobId) ?? [];
      l.push(b.height);
      this.jobBlocks.set(b.jobId, l);
    }
    for (const l of this.jobBlocks.values()) l.sort((a, b) => a - b);
  }

  /** Chain txs naming a job: its post (PostJob / order unlock), SubmitResult, CompleteJob, ExpireJob. */
  jobTxs(job: any): { height: number; tx: TxRec }[] {
    const out: { height: number; tx: TxRec }[] = [];
    const hi = job.closed_height && job.closed_height > 0 ? job.closed_height : this.chainInfo?.height ?? 0;
    for (let h = job.posted_height; h <= hi; h++) {
      const b = this.blocks.get(h);
      if (!b) continue;
      for (const t of b.txs) {
        const f = t.facts;
        if (f.jobId === job.job_id || f.orderJobId === job.job_id) out.push({ height: h, tx: t });
        else if (t.kind === "PostJob" && h === job.posted_height && t.from === job.poster && f.postHashA === job.hash_a) out.push({ height: h, tx: t });
      }
    }
    return out;
  }

  // ------------------------------------------------------------ solana

  async pollSolana() {
    const [cfg, acc] = await this.sol.getMultipleAccounts([D.lcConfigPda(), D.lcAccPda()]);
    this.lc = {
      config: cfg && cfg.owner === D.LIGHTCLIENT_ID ? D.decodeLcConfig(cfg.data) : null,
      acc: acc && acc.owner === D.LIGHTCLIENT_ID ? D.decodeAccumulator(acc.data) : null,
    };

    // storefront buyback: Config totals + the vault's txs (crank buys carry BuybackExecuted)
    const [sfc] = await this.sol.getMultipleAccounts([D.storeConfigPda()]);
    const bb = sfc && sfc.owner === D.STOREFRONT_ID ? D.decodeStoreBuyback(sfc.data) : null;
    if (bb && (!this.buyback || bb.buys !== this.buyback.buys || bb.accrued !== this.buyback.accrued || this.buybackTxs.length === 0)) {
      await this.captureTxs(D.buybackVaultPda(), D.STOREFRONT_ID, this.buybackTxs);
      this.dirty = true;
    }
    this.buyback = bb;

    const accts = await this.sol.getProgramAccounts(D.STOREFRONT_ID, b58encode(D.accountDisc("Order")));
    for (const a of accts) {
      const o = D.decodeOrder(a.address, a.data);
      if (!o) continue;
      const prev = this.orders.get(o.nonce);
      const rec: OrderRec = {
        ...o,
        txs: prev?.txs ?? [],
        payment: prev?.payment ?? null,
        paymentChecked: prev?.paymentChecked ?? false,
        firstSeen: prev?.firstSeen ?? Date.now(),
        sigsFetchedFor: prev?.sigsFetchedFor ?? "",
      };
      this.orders.set(o.nonce, rec);
      if (!prev || prev.status !== o.status) this.dirty = true;
      if (rec.sigsFetchedFor !== o.status) {
        await this.captureTxs(a.address, D.STOREFRONT_ID, rec.txs);
        rec.sigsFetchedFor = o.status;
        this.dirty = true;
      }
      if (!rec.paymentChecked) {
        rec.payment = await this.findPayment(rec);
        rec.paymentChecked = true;
        this.dirty = true;
      }
    }

    // light-client txs for job blocks (verify + final), captured once the record is final
    const lcfg = this.lc.config;
    if (lcfg) {
      const want: BlockRec[] = [];
      for (const hs of this.jobBlocks.values())
        for (const h of hs) {
          const b = this.blocks.get(h)!;
          const have = this.lcTxs.get(b.hash);
          if (h >= lcfg.checkpointHeight && h <= lcfg.tipHeight && !(have && have.some((t) => t.ix.includes("MarkFinal")))) want.push(b);
        }
      if (want.length) {
        const recs = await this.sol.getMultipleAccounts(want.map((b) => D.headerPda(b.hash)));
        for (let i = 0; i < want.length; i++) {
          const r = recs[i];
          if (!r) continue;
          const list = this.lcTxs.get(want[i].hash) ?? [];
          await this.captureTxs(r.address, D.LIGHTCLIENT_ID, list);
          this.lcTxs.set(want[i].hash, list);
          this.dirty = true;
        }
      }
    }

    // bridge withdrawals: released on Solana when the Processed record exists
    const open = [...this.withdrawals.values()].filter((w) => !w.releaseChecked || !w.released);
    if (open.length) {
      const pdas = open.map((w) => D.processedWithdrawalPda(w.nonce));
      const accs = await this.sol.getMultipleAccounts(pdas);
      for (let i = 0; i < open.length; i++) {
        const w = open[i];
        if (accs[i] && !w.released) {
          w.released = true;
          await this.captureTxs(pdas[i], D.BRIDGE_ID, w.releaseTxs);
          this.dirty = true;
        }
        w.releaseChecked = true;
      }
    }
  }

  /** Records the Solana txs touching `address` (newest 50) that are still in the RPC's ledger. */
  async captureTxs(address: string, programId: string, into: SolTx[]) {
    const sigs = await this.sol.getSignatures(address, 50);
    const have = new Set(into.map((t) => t.sig));
    for (const s of sigs) {
      if (have.has(s.signature)) continue;
      const tx = await this.sol.getTransaction(s.signature).catch(() => null);
      const ev = programId === D.STOREFRONT_ID && tx ? D.buybackEvents(tx.meta?.logMessages) : null;
      const parts = programId === D.STOREFRONT_ID && tx ? D.buybackParts(tx.meta?.logMessages) : [];
      const paid = programId === D.STOREFRONT_ID && tx ? D.orderPaid(tx.meta?.logMessages) : [];
      into.push({
        sig: s.signature,
        slot: s.slot,
        time: s.blockTime ?? null,
        ok: !s.err,
        ix: tx ? D.anchorInstructions(tx.meta?.logMessages, programId) : [],
        ...(ev && (ev.executed.length || ev.accrued.length) ? { buyback: ev } : {}),
        ...(parts.length ? { parts } : {}),
        ...(paid.length ? { paid } : {}),
      });
    }
    into.sort((a, b) => a.slot - b.slot);
  }

  /** x402 settles as a System transfer payer -> storefront wallet just before buy_job. */
  async findPayment(o: OrderRec): Promise<OrderRec["payment"]> {
    if (o.recipient === o.buyer) return null;
    const sigs = await this.sol.getSignatures(o.recipient, 25).catch(() => []);
    for (const s of sigs) {
      if (s.err || s.slot > o.createdSlot) continue;
      const tx = await this.sol.getTransaction(s.signature).catch(() => null);
      const t = tx && systemTransfer(tx, o.recipient, o.buyer);
      if (t) return { sig: s.signature, slot: s.slot, lamports: t };
    }
    return null;
  }

  // ------------------------------------------------------------ persistence

  private file(genesis: string) {
    return join(this.o.stateDir, `index-${genesis.slice(0, 16)}.json`);
  }

  save(force: boolean) {
    if (!this.genesis || (!this.dirty && !force)) return;
    if (!force && Date.now() - this.lastSave < 20_000) return;
    mkdirSync(this.o.stateDir, { recursive: true });
    const f = this.file(this.genesis);
    const data = {
      v: STATE_VERSION,
      genesis: this.genesis,
      blocks: [...this.blocks.values()],
      orders: [...this.orders.values()],
      withdrawals: [...this.withdrawals.values()],
      lcTxs: [...this.lcTxs.entries()],
      buybackTxs: this.buybackTxs,
      backfillCursor: this.backfillCursor,
    };
    writeFileSync(f + ".tmp", JSON.stringify(data));
    renameSync(f + ".tmp", f);
    this.dirty = false;
    this.lastSave = Date.now();
  }

  private load(genesis: string): boolean {
    const f = this.file(genesis);
    if (!existsSync(f)) return false;
    try {
      const d = JSON.parse(readFileSync(f, "utf8"));
      if (d.v !== STATE_VERSION || d.genesis !== genesis) return false;
      for (const b of d.blocks) this.putBlock(b);
      for (const o of d.orders) this.orders.set(o.nonce, o);
      for (const w of d.withdrawals ?? []) this.withdrawals.set(w.nonce, w);
      for (const [k, v] of d.lcTxs ?? []) this.lcTxs.set(k, v);
      this.buybackTxs = d.buybackTxs ?? [];
      this.backfillCursor = d.backfillCursor ?? 0;
      this.rebuildJobBlocks();
      this.o.log(`explorer: loaded ${this.blocks.size} blocks, ${this.orders.size} orders from ${f}`);
      return true;
    } catch (e) {
      this.o.log(`explorer: could not load ${f}: ${(e as Error).message}`);
      return false;
    }
  }
}

export function toBlockRec(r: any): BlockRec {
  const hf = r.header_fields;
  const zero = /^0+$/.test(hf.job_id);
  return {
    height: hf.height,
    hash: r.hash,
    prev: hf.prev_hash,
    time: hf.timestamp_ms,
    miner: hf.miner,
    jobId: zero ? null : hf.job_id,
    fees: r.fees ?? "0",
    subsidy: r.subsidy ?? "0",
    reward: r.reward ?? "0",
    proofHash: hf.proof_hash,
    proofLen: r.proof_len ?? 0,
    targetBits: hf.target_bits,
    txs: (r.txs ?? []).map((t: any) => ({ hash: t.tx_hash, kind: t.kind, from: t.from, fee: t.fee, facts: D.txFacts(t.kind, t.tx) })),
  };
}

/** Lamports of a System transfer from -> to inside a getTransaction ("json" encoding) result, or null. */
export function systemTransfer(tx: any, from: string, to: string): string | null {
  const msg = tx?.transaction?.message;
  if (!msg) return null;
  const keys: string[] = [...(msg.accountKeys ?? []), ...(tx.meta?.loadedAddresses?.writable ?? []), ...(tx.meta?.loadedAddresses?.readonly ?? [])];
  for (const ix of msg.instructions ?? []) {
    if (keys[ix.programIdIndex] !== "11111111111111111111111111111111") continue;
    const data = D.b58decode(ix.data);
    if (data.length < 12 || Buffer.from(data).readUInt32LE(0) !== 2) continue;
    if (keys[ix.accounts[0]] === from && keys[ix.accounts[1]] === to) return Buffer.from(data).readBigUInt64LE(4).toString();
  }
  return null;
}
