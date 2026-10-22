// The launch skill (agent spec v2, opt-in; docs/TRADING.md 5.7, docs/ECOSYSTEM.md 7.9): a sovereign
// agent may launch its own pump.fun token on Solana. The flow, all from the agent's own key:
//   1. due?   the spec's launch section allows it (actions.launch + skill), no protocol halt of
//             launches, fewer than max_per_week launches in the last 7 days, the last one at least
//             min_hours_between ago, the last "launch now?" thought at least consider_every_h ago;
//   2. think  one paid thought (the agent's own prepaid balance) decides YES or NO and, on YES,
//             writes the name, symbol and description; only the agent's persona, the creator's
//             theme (part of the signed spec) and its own record go into the prompt, never other
//             accounts' text; the answer goes through the swarm's output filter;
//   3. label  a cadre-launch-meta-v1 blob on Cadre (agent_launched: true, the agent, its creator,
//             the deciding request id), served by cadre-api as the token's metadata uri; the
//             description starts with "Launched by the AI agent @handle on Cadre";
//   4. launch cadre-api POST /v1/tx/pump-create (create_v2 + the dev buy in one tx, dev buy at
//             most launch.max_dev_buy_lamports and inside the day's SOL cap), signed by the agent
//             key and a fresh mint key (thrown away after), sent through /v1/tx/solana;
//   5. anchor a receipt Post on Cadre naming the Solana signature and the mint;
//   6. trade  with launch.trade_own the trade engine adds the token (self-trades, labelled).
import { randomBytes } from "node:crypto";
import { checkOutput } from "../../src/filter.ts";
import { walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { b58encode } from "../../src/b58.ts";
import type { LaunchSpec } from "./spec.ts";

export interface LaunchRec {
  mint: string; signature: string; at: number; name: string; symbol: string;
  dev_buy_lamports: string; spent_lamports: string; meta_uri: string; decided_by: string; receipt: string | null;
}

/** Lamports kept back for the create's rent and fees on top of the dev buy (proposed default; the
 * same figure as the trading engine's fee reserve). The real change is measured and booked. */
export const LAUNCH_RESERVE = 30_000_000n;
const WEEK = 7 * 86_400_000;

/** "" when a launch may be considered now, else why not. */
export function launchDue(l: LaunchSpec, launches: LaunchRec[], consideredAt: number, now: number): string {
  const week = launches.filter((x) => now - x.at < WEEK);
  if (week.length >= l.max_per_week) return `rate limit: ${week.length} launch(es) in the last 7 days (max ${l.max_per_week})`;
  const last = launches.reduce((m, x) => Math.max(m, x.at), 0);
  if (last && now - last < l.min_hours_between * 3_600_000) return `rate limit: the last launch was less than ${l.min_hours_between} h ago`;
  if (consideredAt && now - consideredAt < l.consider_every_h * 3_600_000) return "not due";
  return "";
}

export function launchPrompt(persona: string, handle: string, theme: string, record: string) {
  const t = theme.trim() ? ` Your creator's hint for a token: ${theme.trim().slice(0, 300)}` : "";
  return [
    {
      role: "system", content: `You are ${persona.slice(0, 1200)} (@${handle}), an AI agent on Cadre. You may launch your own token on pump.fun on Solana; it will be labelled as launched by an AI agent, and buying some of it at launch costs you your own SOL.${t} Decide whether to launch one now. Answer only in this format:\nLAUNCH: YES or NO\nNAME: <token name, at most 32 characters>\nSYMBOL: <2 to 10 capital letters or digits>\nDESCRIPTION: <one sentence, at most 160 characters>`,
    },
    { role: "user", content: `Your record: ${record.slice(0, 600)}. Decide.` },
  ];
}

export interface LaunchIdea { name: string; symbol: string; description: string }

/** Parse and filter the thought; null = NO or unusable. */
export function parseLaunch(text: string): LaunchIdea | null {
  const field = (k: string) => { const m = new RegExp(`^\\s*${k}:\\s*(.+)$`, "mi").exec(text); return m ? m[1].trim() : null; };
  if (!/^yes\b/i.test(field("LAUNCH") ?? "")) return null;
  const clean = (x: string | null, n: number) => {
    if (!x) return null;
    const c = x.replace(/[\u0000-\u001f\u007f]/g, " ").replace(/^["'`]+|["'`]+$/g, "").trim();
    return c && Buffer.byteLength(c, "utf8") <= n ? c : null;
  };
  const name = clean(field("NAME"), 32);
  const symbol = (field("SYMBOL") ?? "").replace(/^\$/, "").trim().toUpperCase();
  const description = clean(field("DESCRIPTION"), 160);
  if (!name || !description || !/^[A-Z0-9]{2,10}$/.test(symbol)) return null;
  // the swarm's output filter over the words a buyer will read (no links, keys, return promises)
  if (!checkOutput(`${name}: ${description}`, { maxBytes: 256, allowHosts: [] }).ok) return null;
  return { name, symbol, description };
}

/** The runner surface the launcher needs (AgentRunner implements it). */
export interface LaunchHost {
  address: string;
  handle: string;
  operator: string;
  specSha256: string;
  persona: string;
  apiUrl: string;
  wallet: Wallet;
  launch: LaunchSpec;
  launches: LaunchRec[];
  consideredAt: number;
  now(): number;
  haltLaunch(): string; // "" or the reason launches are halted
  canSpendSol(lamports: bigint): boolean;
  bookSol(lamports: bigint): void;
  lamports(): Promise<bigint>;
  record(): string;
  think(messages: { role: string; content: string }[], maxTokens: number): Promise<{ text: string; requestId: string } | null>;
  putBlob(data: Buffer): Promise<{ uri: string }>;
  build(body: unknown, signers: Wallet[]): Promise<{ signature: string; summary: any }>;
  confirm(sig: string): Promise<{ ok: boolean; err: unknown }>;
  receipt(text: string, sigs: string[]): Promise<string | null>;
  note(kind: string, text: string, x?: Record<string, unknown>): void;
  save(): void;
  tradeOwn(mint: string): Promise<void>;
}

/** One launch attempt; returns what happened (for the tick log). */
export async function maybeLaunch(h: LaunchHost): Promise<string> {
  const halted = h.haltLaunch();
  if (halted) return "halted";
  const now = h.now();
  const why = launchDue(h.launch, h.launches, h.consideredAt, now);
  if (why) return why === "not due" ? "not_due" : "rate_limited";
  if (!h.consideredAt) { h.consideredAt = now; h.save(); return "first_wait"; } // the first idea comes one period after the agent goes live
  const have = await h.lamports();
  const cap = BigInt(h.launch.max_dev_buy_lamports);
  const room = have > LAUNCH_RESERVE ? have - LAUNCH_RESERVE : 0n;
  const devBuy = cap < room ? cap : room;
  if (have < LAUNCH_RESERVE) { h.note("launch", `no launch: the wallet holds ${have} lamports, a launch keeps ${LAUNCH_RESERVE} for rent and fees`, { ok: false }); h.consideredAt = now; h.save(); return "no_funds"; }
  if (!h.canSpendSol(devBuy + LAUNCH_RESERVE)) { h.note("budget", "launch skipped: the SOL cap leaves less than the dev buy plus the create's rent and fees today", { ok: false }); return "budget"; }
  h.consideredAt = now;
  h.save();
  const t = await h.think(launchPrompt(h.persona, h.handle, h.launch.theme, h.record()), 90);
  if (!t) return "budget";
  const idea = parseLaunch(t.text);
  if (!idea) { h.note("launch_no", "decided not to launch a token (or the idea did not pass the filter)", { request_id: t.requestId }); return "judged_no"; }
  const description = `Launched by the AI agent @${h.handle} on Cadre (agent ${h.address}). ${idea.description}`;
  const meta = { format: "cadre-launch-meta-v1", name: idea.name, symbol: idea.symbol, description, image: null, agent_launched: true, agent: h.address, agent_creator: h.operator, spec_sha256: h.specSha256, thought_ref: t.requestId, launched_ms: now };
  const put = await h.putBlob(Buffer.from(JSON.stringify(meta), "utf8"));
  const id = put.uri.replace(/^blob:/, "");
  // the short form (cadre-api GET /v1/l/<blob id in base58>): the create tx must fit 1,232 bytes
  const uri = `${h.apiUrl.replace(/\/$/, "")}/v1/l/${b58encode(Buffer.from(id, "hex"))}`;
  const mintKey = walletFromSeed(randomBytes(32));
  const before = have;
  let sig: string;
  let summary: any;
  try {
    ({ signature: sig, summary } = await h.build({ creator: h.address, mint: mintKey.address, name: idea.name, symbol: idea.symbol, uri, dev_buy_lamports: devBuy.toString() }, [h.wallet, mintKey]));
  } catch (e) {
    h.note("launch_failed", `the launch tx was not built or sent: ${(e as Error).message.slice(0, 200)}`, { request_id: t.requestId, ok: false });
    return "failed";
  }
  const c = await h.confirm(sig);
  const after = await h.lamports().catch(() => before);
  const spent = before > after ? before - after : 0n;
  h.bookSol(spent > 0n ? spent : devBuy + LAUNCH_RESERVE);
  if (!c.ok) { h.note("launch_failed", `Solana tx ${sig} failed: ${JSON.stringify(c.err).slice(0, 160)}`, { sol: sig, request_id: t.requestId, ok: false }); return "failed"; }
  const rec: LaunchRec = { mint: mintKey.address, signature: sig, at: now, name: idea.name, symbol: idea.symbol, dev_buy_lamports: devBuy.toString(), spent_lamports: spent.toString(), meta_uri: uri, decided_by: t.requestId, receipt: null };
  h.launches.push(rec);
  h.save();
  rec.receipt = await h.receipt(`pump create_v2: agent-launched token ${idea.symbol} "${idea.name}" mint ${mintKey.address}; dev buy ${devBuy} lamports (spent ${spent} incl. rent and fees, quoted ${summary?.quote_tokens_out_spl ?? "?"} SPL base units); metadata ${id}; decided by ${t.requestId}`, [sig]);
  h.save();
  h.note("launch", `launched its own token ${idea.symbol} (${mintKey.address}) on pump.fun`, { sol: sig, request_id: t.requestId, ok: true });
  if (h.launch.trade_own) await h.tradeOwn(mintKey.address).catch((e) => h.note("trade_error", `own token not added to trading: ${(e as Error).message}`, { ok: false }));
  return "launched";
}
