// Client for the Solana mandate program (token/programs/mandate, docs/TRADING.md): PDAs, the
// instructions (open, add_mint, swap, settle, deposit, withdraw_sol, set_limits, set_agent,
// revoke), the Mandate account and event decoders, and PumpSwap pool reserves read exactly the
// way the program reads them (pump.rs read_pool). Built on @solana/web3.js only; no Anchor client.
import { createHash } from "node:crypto";
import { PublicKey, SystemProgram, TransactionInstruction, type Connection, type AccountInfo } from "@solana/web3.js";

export const PUMP = new PublicKey("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
export const AMM = new PublicKey("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
export const FEE = new PublicKey("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
export const WSOL = new PublicKey("So11111111111111111111111111111111111111112");
export const TOKEN = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const T22 = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const ATA = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
export const VAULT_RENT = 890_880n;

const seedBuf = (s: string | Buffer | PublicKey | Uint8Array) => (typeof s === "string" ? Buffer.from(s) : s instanceof PublicKey ? s.toBuffer() : Buffer.from(s));
export const pda = (seeds: (string | Buffer | PublicKey | Uint8Array)[], prog: PublicKey) => PublicKey.findProgramAddressSync(seeds.map(seedBuf), prog)[0];
export const ata = (owner: PublicKey, mint: PublicKey, tp: PublicKey) => pda([owner, tp, mint], ATA);
const le64 = (n: bigint | number) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const disc = (ns: string, name: string) => createHash("sha256").update(`${ns}:${name}`).digest().subarray(0, 8);
const ixDisc = (name: string) => disc("global", name);

export function canonicalPool(mint: PublicKey): PublicKey {
  const authority = pda(["pool-authority", mint], PUMP);
  const idx = Buffer.alloc(2);
  return pda(["pool", idx, authority, mint, WSOL], AMM);
}

export interface Limits {
  venues: number; anyMint: boolean; mints: PublicKey[]; maxInPerTrade: bigint; windowCap: bigint; windowSecs: number;
  maxTradesPerWindow: number; maxSlippageBps: number; minPoolQuoteReserve: bigint; maxTradeShareBps: number; expiryTs: bigint; paused: boolean;
}
export interface Terms { perfFeeBps: number; settleIntervalSecs: number }

export function encodeLimits(l: Limits): Buffer {
  if (l.mints.length > 4) throw new Error("at most 4 mints");
  const mints = [...l.mints, ...Array(4 - l.mints.length).fill(PublicKey.default)].map((m) => m.toBuffer());
  const b = Buffer.alloc(1 + 1 + 1 + 128 + 8 + 8 + 4 + 2 + 2 + 8 + 2 + 8 + 1);
  let o = 0;
  b.writeUInt8(l.venues, o++); b.writeUInt8(l.anyMint ? 1 : 0, o++); b.writeUInt8(l.mints.length, o++);
  for (const m of mints) { m.copy(b, o); o += 32; }
  b.writeBigUInt64LE(l.maxInPerTrade, o); o += 8; b.writeBigUInt64LE(l.windowCap, o); o += 8;
  b.writeUInt32LE(l.windowSecs, o); o += 4; b.writeUInt16LE(l.maxTradesPerWindow, o); o += 2; b.writeUInt16LE(l.maxSlippageBps, o); o += 2;
  b.writeBigUInt64LE(l.minPoolQuoteReserve, o); o += 8; b.writeUInt16LE(l.maxTradeShareBps, o); o += 2;
  b.writeBigInt64LE(l.expiryTs, o); o += 8; b.writeUInt8(l.paused ? 1 : 0, o++);
  return b;
}
function readLimits(d: Buffer, o: number): [Limits, number] {
  const venues = d[o++], anyMint = d[o++] === 1, n = d[o++];
  const mints: PublicKey[] = [];
  for (let i = 0; i < 4; i++) { if (i < n) mints.push(new PublicKey(d.subarray(o, o + 32))); o += 32; }
  const l: Limits = {
    venues, anyMint, mints,
    maxInPerTrade: d.readBigUInt64LE(o), windowCap: d.readBigUInt64LE(o + 8), windowSecs: d.readUInt32LE(o + 16), maxTradesPerWindow: d.readUInt16LE(o + 20),
    maxSlippageBps: d.readUInt16LE(o + 22), minPoolQuoteReserve: d.readBigUInt64LE(o + 24), maxTradeShareBps: d.readUInt16LE(o + 32), expiryTs: d.readBigInt64LE(o + 34), paused: d[o + 42] === 1,
  };
  return [l, o + 43];
}
export const encodeTerms = (t: Terms) => { const b = Buffer.alloc(6); b.writeUInt16LE(t.perfFeeBps, 0); b.writeUInt32LE(t.settleIntervalSecs, 2); return b; };

export interface MandateAccount {
  user: PublicKey; agent: PublicKey; feeTo: PublicKey; mandateId: bigint; revoked: boolean; limits: Limits; terms: Terms;
  openedTs: bigint; windowStart: bigint; windowUsed: bigint; windowTrades: number; deposited: bigint; withdrawn: bigint;
  feesAgent: bigint; feesProtocol: bigint; hwm: bigint; lastSettle: bigint; trades: bigint; volume: bigint;
}
export function decodeMandate(data: Buffer): MandateAccount {
  if (!data.subarray(0, 8).equals(disc("account", "Mandate"))) throw new Error("not a Mandate account");
  let o = 8;
  const pk = () => { const p = new PublicKey(data.subarray(o, o + 32)); o += 32; return p; };
  const user = pk(), agent = pk(), feeTo = pk();
  const mandateId = data.readBigUInt64LE(o); o += 8;
  o += 2; // bump, vault_bump
  const revoked = data[o++] === 1;
  const [limits, o2] = readLimits(data, o); o = o2;
  const terms = { perfFeeBps: data.readUInt16LE(o), settleIntervalSecs: data.readUInt32LE(o + 2) }; o += 6;
  const i64 = () => { const v = data.readBigInt64LE(o); o += 8; return v; };
  const u64 = () => { const v = data.readBigUInt64LE(o); o += 8; return v; };
  const openedTs = i64(), windowStart = i64(), windowUsed = u64();
  const windowTrades = data.readUInt16LE(o); o += 2;
  return { user, agent, feeTo, mandateId, revoked, limits, terms, openedTs, windowStart, windowUsed, windowTrades, deposited: u64(), withdrawn: u64(), feesAgent: u64(), feesProtocol: u64(), hwm: i64(), lastSettle: i64(), trades: u64(), volume: u64() };
}

/** Realized SOL PnL of a mandate: equity + withdrawn + fees - deposited (the program's base). */
export function pnlOf(m: MandateAccount, vaultLamports: bigint): bigint {
  const eq = vaultLamports > VAULT_RENT ? vaultLamports - VAULT_RENT : 0n;
  return eq + m.withdrawn + m.feesAgent + m.feesProtocol - m.deposited;
}

export interface SwapEvent { mandate: string; agent: string; venue: number; side: number; mint: string; amountIn: bigint; amountOut: bigint; quote: bigint; poolBase: bigint; poolQuote: bigint; equityAfter: bigint; slot: bigint }
const EV_SWAP = disc("event", "MandateSwap");
/** Decode MandateSwap events from a tx's log lines ("Program data: <base64>"). */
export function swapEventsFromLogs(logs: string[]): SwapEvent[] {
  const out: SwapEvent[] = [];
  for (const l of logs) {
    const m = l.match(/^Program data: (.+)$/);
    if (!m) continue;
    const d = Buffer.from(m[1], "base64");
    if (d.length < 8 + 32 + 32 + 2 + 32 + 56 || !d.subarray(0, 8).equals(EV_SWAP)) continue;
    let o = 8;
    const pk = () => { const p = new PublicKey(d.subarray(o, o + 32)).toBase58(); o += 32; return p; };
    const mandate = pk(), agent = pk();
    const venue = d[o++], side = d[o++];
    const mint = pk();
    const u = () => { const v = d.readBigUInt64LE(o); o += 8; return v; };
    out.push({ mandate, agent, venue, side, mint, amountIn: u(), amountOut: u(), quote: u(), poolBase: u(), poolQuote: u(), equityAfter: u(), slot: u() });
  }
  return out;
}

export class MandateProgram {
  id: PublicKey;
  config: PublicKey;
  constructor(id: PublicKey | string) {
    this.id = typeof id === "string" ? new PublicKey(id) : id;
    this.config = pda(["mconfig"], this.id);
  }
  mandate(user: PublicKey, id: bigint | number) { return pda(["mandate", user, le64(id)], this.id); }
  vault(mandate: PublicKey) { return pda(["vault", mandate], this.id); }

  initConfig(admin: PublicKey, protocolBps: number, buybackVault: PublicKey) {
    const data = Buffer.concat([ixDisc("init_config"), Buffer.from([protocolBps & 0xff, protocolBps >> 8]), buybackVault.toBuffer()]);
    return new TransactionInstruction({ programId: this.id, data, keys: [
      { pubkey: admin, isSigner: true, isWritable: true }, { pubkey: this.config, isSigner: false, isWritable: true }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ] });
  }
  /** set_config: each field null = unchanged (the swap kill switch is `swapsPaused`). */
  setConfig(admin: PublicKey, p: { protocolBps?: number; buybackVault?: PublicKey; swapsPaused?: boolean; admin?: PublicKey }) {
    const opt = (v: Buffer | null) => (v ? Buffer.concat([Buffer.from([1]), v]) : Buffer.from([0]));
    const u16 = (n: number) => Buffer.from([n & 0xff, n >> 8]);
    const data = Buffer.concat([ixDisc("set_config"), opt(p.protocolBps !== undefined ? u16(p.protocolBps) : null), opt(p.buybackVault ? p.buybackVault.toBuffer() : null), opt(p.swapsPaused !== undefined ? Buffer.from([p.swapsPaused ? 1 : 0]) : null), opt(p.admin ? p.admin.toBuffer() : null)]);
    return new TransactionInstruction({ programId: this.id, data, keys: [{ pubkey: admin, isSigner: true, isWritable: false }, { pubkey: this.config, isSigner: false, isWritable: true }] });
  }
  open(user: PublicKey, id: bigint, agent: PublicKey, feeTo: PublicKey, limits: Limits, terms: Terms, deposit: bigint) {
    const m = this.mandate(user, id);
    const data = Buffer.concat([ixDisc("open"), le64(id), agent.toBuffer(), feeTo.toBuffer(), encodeLimits(limits), encodeTerms(terms), le64(deposit)]);
    return new TransactionInstruction({ programId: this.id, data, keys: [
      { pubkey: user, isSigner: true, isWritable: true }, { pubkey: m, isSigner: false, isWritable: true }, { pubkey: this.vault(m), isSigner: false, isWritable: true }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ] });
  }
  /** PumpSwap init_user_volume_accumulator for the vault (needed once before its first trade). */
  initVolumeAccumulator(payer: PublicKey, mandate: PublicKey) {
    const v = this.vault(mandate);
    return new TransactionInstruction({ programId: AMM, data: Buffer.from([94, 6, 202, 115, 255, 96, 232, 183]), keys: [
      { pubkey: payer, isSigner: true, isWritable: true }, { pubkey: v, isSigner: false, isWritable: false }, { pubkey: pda(["user_volume_accumulator", v], AMM), isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false }, { pubkey: pda(["__event_authority"], AMM), isSigner: false, isWritable: false }, { pubkey: AMM, isSigner: false, isWritable: false },
    ] });
  }
  addMint(payer: PublicKey, mandate: PublicKey, mint: PublicKey, tokenProgram = T22) {
    const v = this.vault(mandate);
    return new TransactionInstruction({ programId: this.id, data: ixDisc("add_mint"), keys: [
      { pubkey: payer, isSigner: true, isWritable: true }, { pubkey: mandate, isSigner: false, isWritable: false }, { pubkey: v, isSigner: false, isWritable: false },
      { pubkey: mint, isSigner: false, isWritable: false }, { pubkey: ata(v, mint, tokenProgram), isSigner: false, isWritable: true }, { pubkey: tokenProgram, isSigner: false, isWritable: false },
      { pubkey: ATA, isSigner: false, isWritable: false }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ] });
  }
  private userOp(name: string, user: PublicKey, mandate: PublicKey, arg: Buffer) {
    return new TransactionInstruction({ programId: this.id, data: Buffer.concat([ixDisc(name), arg]), keys: [
      { pubkey: user, isSigner: true, isWritable: true }, { pubkey: mandate, isSigner: false, isWritable: true }, { pubkey: this.vault(mandate), isSigner: false, isWritable: true }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ] });
  }
  deposit(user: PublicKey, mandate: PublicKey, lamports: bigint) { return this.userOp("deposit", user, mandate, le64(lamports)); }
  withdrawSol(user: PublicKey, mandate: PublicKey, lamports: bigint) { return this.userOp("withdraw_sol", user, mandate, le64(lamports)); }
  private userOnly(name: string, user: PublicKey, mandate: PublicKey, arg: Buffer) {
    return new TransactionInstruction({ programId: this.id, data: Buffer.concat([ixDisc(name), arg]), keys: [{ pubkey: user, isSigner: true, isWritable: false }, { pubkey: mandate, isSigner: false, isWritable: true }] });
  }
  setLimits(user: PublicKey, mandate: PublicKey, l: Limits) { return this.userOnly("set_limits", user, mandate, encodeLimits(l)); }
  setAgent(user: PublicKey, mandate: PublicKey, agent: PublicKey) { return this.userOnly("set_agent", user, mandate, agent.toBuffer()); }
  revoke(user: PublicKey, mandate: PublicKey) { return this.userOnly("revoke", user, mandate, Buffer.alloc(0)); }
  withdrawToken(user: PublicKey, mandate: PublicKey, mint: PublicKey, amount: bigint, tokenProgram = T22) {
    const v = this.vault(mandate);
    return new TransactionInstruction({ programId: this.id, data: Buffer.concat([ixDisc("withdraw_token"), le64(amount)]), keys: [
      { pubkey: user, isSigner: true, isWritable: true }, { pubkey: mandate, isSigner: false, isWritable: false }, { pubkey: v, isSigner: false, isWritable: true },
      { pubkey: mint, isSigner: false, isWritable: false }, { pubkey: ata(v, mint, tokenProgram), isSigner: false, isWritable: true }, { pubkey: ata(user, mint, tokenProgram), isSigner: false, isWritable: true },
      { pubkey: tokenProgram, isSigner: false, isWritable: false }, { pubkey: ATA, isSigner: false, isWritable: false }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ] });
  }
  settle(mandate: PublicKey, buybackVault: PublicKey, feeTo: PublicKey) {
    return new TransactionInstruction({ programId: this.id, data: ixDisc("settle"), keys: [
      { pubkey: this.config, isSigner: false, isWritable: false }, { pubkey: mandate, isSigner: false, isWritable: true }, { pubkey: this.vault(mandate), isSigner: false, isWritable: true },
      { pubkey: buybackVault, isSigner: false, isWritable: true }, { pubkey: feeTo, isSigner: false, isWritable: true }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ] });
  }
  /** The agent's swap: side 0 buy (amount_in lamports), 1 sell (amount_in base units); PumpSwap v2 accounts appended. */
  swap(agent: PublicKey, mandate: PublicKey, pool: PoolInfo, side: 0 | 1, amountIn: bigint, minOut: bigint) {
    const v = this.vault(mandate);
    const data = Buffer.concat([ixDisc("swap"), Buffer.from([side]), le64(amountIn), le64(minOut)]);
    const rem: [PublicKey, boolean][] = [
      [pool.pool, true], [v, true], [pda(["global_config"], AMM), false], [pool.mint, false], [WSOL, false],
      [ata(v, pool.mint, pool.baseTokenProgram), true], [ata(v, WSOL, TOKEN), true], [pool.baseVault, true], [pool.quoteVault, true],
      [pool.baseTokenProgram, false], [TOKEN, false], [SystemProgram.programId, false], [pda(["user_volume_accumulator", v], AMM), true],
      [pda(["fee_config", AMM], FEE), false], [pool.buybackFeeRecipient, true], [pda(["__event_authority"], AMM), false], [AMM, false],
    ];
    return new TransactionInstruction({ programId: this.id, data, keys: [
      { pubkey: agent, isSigner: true, isWritable: false }, { pubkey: this.config, isSigner: false, isWritable: false }, { pubkey: mandate, isSigner: false, isWritable: true },
      { pubkey: v, isSigner: false, isWritable: true }, { pubkey: ATA, isSigner: false, isWritable: false }, { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ...rem.map(([pubkey, isWritable]) => ({ pubkey, isWritable, isSigner: false })),
    ] });
  }
}

export interface PoolInfo { pool: PublicKey; mint: PublicKey; baseVault: PublicKey; quoteVault: PublicKey; baseTokenProgram: PublicKey; buybackFeeRecipient: PublicKey }
export interface Reserves { base: bigint; quote: bigint; slot: number }

/** Static pool accounts (pool, vaults, the global config's first buyback fee recipient). */
export async function poolInfo(conn: Connection, mint: PublicKey, baseTokenProgram = T22): Promise<PoolInfo> {
  const pool = canonicalPool(mint);
  const [p, gc] = await conn.getMultipleAccountsInfo([pool, pda(["global_config"], AMM)]);
  if (!p) throw new Error(`no canonical PumpSwap pool for ${mint.toBase58()}`);
  const d = p.data;
  const baseVault = new PublicKey(d.subarray(139, 171)), quoteVault = new PublicKey(d.subarray(171, 203));
  const recipients = buybackRecipients(gc!);
  return { pool, mint, baseVault, quoteVault, baseTokenProgram, buybackFeeRecipient: ata(recipients[0], WSOL, TOKEN) };
}

/** PumpSwap GlobalConfig.buyback_fee_recipients (IDL order: disc 8, admin 32, lp/protocol fee bps 16,
 *  disable_flags 1, protocol_fee_recipients 256, coin_creator bps 8, 3 keys 96, mayhem 1,
 *  reserved_fee_recipients 224, cashback 1 = offset 643, then 8 keys). */
export function buybackRecipients(gc: AccountInfo<Buffer>): PublicKey[] {
  const d = gc.data;
  const out: PublicKey[] = [];
  for (let i = 0; i < 8; i++) out.push(new PublicKey(d.subarray(643 + 32 * i, 675 + 32 * i)));
  return out.filter((k) => !k.equals(PublicKey.default));
}

/** Reserves as the program reads them: vaults minus protocol and creator fees, plus virtual quote. */
export function reservesOf(poolData: Buffer, baseVaultData: Buffer, quoteVaultData: Buffer, slot = 0): Reserves {
  const base = baseVaultData.readBigUInt64LE(64);
  let vq = poolData.length >= 261 ? poolData.readBigInt64LE(245) + (poolData.readBigInt64LE(253) << 64n) : 0n;
  if (vq < 0n) vq = 0n;
  const quote = quoteVaultData.readBigUInt64LE(64) - poolData.readBigUInt64LE(271) - poolData.readBigUInt64LE(279) + vq;
  return { base, quote, slot };
}
export async function readReserves(conn: Connection, p: PoolInfo): Promise<Reserves> {
  const r = await conn.getMultipleAccountsInfoAndContext([p.pool, p.baseVault, p.quoteVault], "processed");
  const [a, b, q] = r.value;
  if (!a || !b || !q) throw new Error("pool accounts missing");
  return reservesOf(a.data, b.data, q.data, r.context.slot);
}
export const cpOut = (rin: bigint, rout: bigint, x: bigint) => (rin + x === 0n ? 0n : (rout * x) / (rin + x));
