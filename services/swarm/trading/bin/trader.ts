// House trading agents (docs/TRADING.md 5 "House traders"):
//   node bin/trader.ts run --keys <dir> --cadre <deploy/trading-devnet.json> --solana <deploy/trading-devnet-solana.json>
//   node bin/trader.ts profiles --keys <dir>     publish the traders' Cadre profiles (SetProfile, flags agent + trades)
//   node bin/trader.ts once --keys <dir> ... --agent desk_maker --venue pumpswap --side buy --amount <quote units>
//        one order through the fast path, with the latency row printed (used for the latency table)
// Env: CADRE_RPC, CADRE_WS, SOLANA_RPC, SOLANA_WS, GATEWAY_URL (default cadre-api's /v1/openai),
// CADRE_API (receipt visibility), PRIORITY_MICROLAMPORTS, TRADING_KILL_FILE, TRADING_LATENCY_LOG,
// TRADING_THINK_MIN (minutes between paid thoughts, default 20), TRADING_BRAIN=off to trade on a fixed stance.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Keypair, PublicKey } from "@solana/web3.js";
import { key32 } from "../../src/b58.ts";
import { loadWallet, type Wallet } from "../../src/wallet.ts";
import { Brain } from "../src/brain.ts";
import { Cadre } from "../src/cadre.ts";
import { MandateProgram, poolInfo } from "../src/solmandate.ts";
import { type RiskRules, Trader } from "../src/trader.ts";
import { CadreAmm, PumpSwapMandate, quoteOut, type Venue, withSlippage } from "../src/venues.ts";

function arg(name: string, def?: string): string {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1]) return process.argv[i + 1];
  if (def !== undefined) return def;
  throw new Error(`--${name} is required`);
}
const env = process.env;
const log = (event: string, x: Record<string, unknown> = {}) => console.log(JSON.stringify({ t: new Date().toISOString(), event, ...x }, (_k, v) => (typeof v === "bigint" ? v.toString() : v)));

/** The house traders: persona text is ours (no real people), every one flagged as a house trader. */
export const HOUSE = [
  { key: "trader-1.json", handle: "desk_maker", name: "Desk Maker", persona: "Desk Maker, who launched the DESK token and keeps its pool honest", venues: ["cadre-amm", "pumpswap"] },
  { key: "trader-2.json", handle: "tape_reader", name: "Tape Reader", persona: "Tape Reader, a patient trend follower", venues: ["cadre-amm-mandate", "pumpswap"] },
  { key: "trader-3.json", handle: "slow_hand", name: "Slow Hand", persona: "Slow Hand, a contrarian who buys weakness and sells strength", venues: ["pumpswap"] },
] as const;

const CADRE_UNIT = 1_000_000_000n;
const riskFor = (venue: string): RiskRules => venue === "pumpswap"
  ? { maxOrder: 10_000_000n, maxPosition: 60_000_000n, dailyLossStop: 30_000_000n, cooldownMs: 120_000, maxOrdersPerHour: 6, slippageBps: 250, maxChaseBps: 300 }
  : { maxOrder: 10n * CADRE_UNIT, maxPosition: 80n * CADRE_UNIT, dailyLossStop: 40n * CADRE_UNIT, cooldownMs: 180_000, maxOrdersPerHour: 4, slippageBps: 250, maxChaseBps: 300 };

async function setup() {
  const dir = arg("keys");
  const cad = JSON.parse(readFileSync(arg("cadre"), "utf8"));
  const sol = JSON.parse(readFileSync(arg("solana"), "utf8")).solana;
  const cadre = new Cadre(env.CADRE_RPC ?? cad.rpc ?? "https://rpc.64-95-12-116.sslip.io");
  await cadre.init();
  const wsUrl = env.CADRE_WS ?? (env.CADRE_RPC ?? cad.rpc ?? "https://rpc.64-95-12-116.sslip.io").replace(/^http/, "ws").replace(/\/$/, "") + "/ws";
  const solRpc = env.SOLANA_RPC ?? "https://api.devnet.solana.com";
  const mp = new MandateProgram(sol.program);
  const { Connection } = await import("@solana/web3.js");
  const pool = await poolInfo(new Connection(solRpc), new PublicKey(sol.mint));
  return { dir, cad, sol, cadre, wsUrl, solRpc, mp, pool };
}

function venueFor(s: Awaited<ReturnType<typeof setup>>, v: string, wallet: Wallet, kp: Keypair): Venue {
  if (v === "pumpswap") {
    const m = s.sol.mandates[wallet.address];
    if (!m) throw new Error(`no Solana mandate for ${wallet.address}`);
    return new PumpSwapMandate({ rpc: s.solRpc, ws: env.SOLANA_WS, agent: kp, mp: s.mp, mandate: new PublicKey(m.mandate), pool: s.pool, priorityMicroLamports: Number(env.PRIORITY_MICROLAMPORTS ?? 10_000) });
  }
  const mandate = v === "cadre-amm-mandate" ? { contract: s.cad.mandate_contract, id: BigInt(s.cad.cadre_mandate_id) } : undefined;
  return new CadreAmm({ cadre: s.cadre, wsUrl: s.wsUrl, wallet, pool: s.cad.amm_pool, token: s.cad.agent_token, mandate });
}

const loadKp = (f: string) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(f, "utf8"))));

async function profiles() {
  const dir = arg("keys");
  const cadre = new Cadre(env.CADRE_RPC ?? "https://rpc.64-95-12-116.sslip.io");
  await cadre.init();
  const ops = loadWallet(join(dir, "ops.json"));
  for (const h of HOUSE) {
    const w = loadWallet(join(dir, h.key));
    const p = await cadre.rpc.call("getProfile", { account: w.address }).catch(() => null);
    if (p?.has_profile) { log("profile", { handle: h.handle, account: w.address, skipped: true }); continue; }
    const s = await cadre.sendNative(w, { kind: "SetProfile", handle: h.handle, name: h.name, profileUri: "", profileHash: new Uint8Array(32), flags: 1 | 8, operator: key32(ops.address), encKey: new Uint8Array(32), dmPrice: 0n, dmPolicy: 3, threadFreeBlocks: 0 } as any);
    log("profile", { handle: h.handle, account: w.address, tx: s.txHash });
  }
}

async function run() {
  const s = await setup();
  const houseBook = new Map();
  const traders: Trader[] = [];
  const brainOn = env.TRADING_BRAIN !== "off";
  for (const h of HOUSE) {
    const wallet = loadWallet(join(s.dir, h.key));
    const kp = loadKp(join(s.dir, h.key));
    const brain = brainOn ? new Brain({ gatewayUrl: env.GATEWAY_URL ?? "https://api.64-95-12-116.sslip.io/v1/openai", wallet, solRpc: s.solRpc, topupRows: 2_048, log }) : null;
    for (const v of h.venues) {
      const venue = venueFor(s, v, wallet, kp);
      const t = new Trader({
        handle: h.handle, persona: h.persona, house: true, venue, brain, cadre: s.cadre, poster: wallet, risk: riskFor(venue.name),
        thinkEveryMs: Number(env.TRADING_THINK_MIN ?? 20) * 60_000, killFile: env.TRADING_KILL_FILE ?? "/tmp/cadre-trading.stop",
        latencyLog: env.TRADING_LATENCY_LOG ?? "trading-latency.jsonl", houseBook, log, postReceipts: true, apiUrl: env.CADRE_API ?? "https://api.64-95-12-116.sslip.io",
      });
      if (!brainOn) t.strategy = { stance: (env.TRADING_STANCE as any) ?? "hold", conviction: 1, setAt: Date.now(), requestId: "", text: "fixed" };
      await t.start();
      traders.push(t);
      log("trader_started", { handle: h.handle, ...venue.describe() });
    }
  }
  const stop = () => { for (const t of traders) t.stop(); log("stopped"); process.exit(0); };
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
}

/** One measured order through the same fast path: event -> decide -> sign -> send -> land -> receipt -> visible. */
async function once() {
  const s = await setup();
  const handle = arg("agent");
  const h = HOUSE.find((x) => x.handle === handle);
  if (!h) throw new Error(`unknown agent ${handle}`);
  const wallet = loadWallet(join(s.dir, h.key));
  const kp = loadKp(join(s.dir, h.key));
  const venue = venueFor(s, arg("venue"), wallet, kp);
  const side = arg("side") === "sell" ? 1 : 0;
  const brain = env.TRADING_BRAIN === "off" ? null : new Brain({ gatewayUrl: env.GATEWAY_URL ?? "https://api.64-95-12-116.sslip.io/v1/openai", wallet, solRpc: s.solRpc, topupRows: 2_048, log });
  const t = new Trader({
    handle, persona: h.persona, house: true, venue, brain, cadre: s.cadre, poster: wallet, risk: { ...riskFor(venue.name), cooldownMs: 0, maxChaseBps: 100_000 },
    thinkEveryMs: 1e12, killFile: env.TRADING_KILL_FILE ?? "/tmp/cadre-trading.stop", latencyLog: env.TRADING_LATENCY_LOG ?? "trading-latency.jsonl",
    houseBook: new Map(), log, postReceipts: true, apiUrl: env.CADRE_API ?? "https://api.64-95-12-116.sslip.io",
  });
  t.book = await venue.book();
  if (brain) await t.think(); // the paid thought (timed); its stance is logged, the measured order below uses --side
  const amount = BigInt(arg("amount"));
  t.strategy = { ...t.strategy, stance: side === 0 ? "buy" : "sell", conviction: 5 };
  t.o.risk.maxOrder = amount;
  t.o.risk.maxPosition = amount * 1_000n;
  let fired = false;
  await venue.start(async (e) => {
    if (fired) return;
    fired = true;
    await t.onEvent(e);
    venue.stop();
    log("once_done", { agent: handle });
    setTimeout(() => process.exit(0), 500);
  });
  // a venue that only fires on new blocks / vault changes: nudge with a fresh read
  setTimeout(async () => {
    if (fired) return;
    const r = await venue.reserves();
    fired = true;
    await t.onEvent({ venue: venue.name, base: r.base, quote: r.quote, at: performance.now(), ref: "poll" });
    venue.stop();
    setTimeout(() => process.exit(0), 500);
  }, Number(env.ONCE_WAIT_MS ?? 90_000));
  void quoteOut; void withSlippage;
}

const cmd = process.argv[2];
(cmd === "run" ? run() : cmd === "profiles" ? profiles() : cmd === "once" ? once() : Promise.reject(new Error("usage: trader.ts run|profiles|once ..."))).catch((e) => {
  log("fatal", { error: (e as Error).message });
  process.exit(1);
});
