// The launch skill (src/launch.ts) and the protocol halt watcher (src/halt.ts) without a network:
// rate limits, the paid thought's parsing and filter, labelled metadata, the dev-buy cap, the SOL
// cap, both signers on the create tx, the receipt, halts (scopes, fail closed, off), and the spec's
// launch section rules (src/spec.ts).
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { b58encode } from "../../src/b58.ts";
import { decodeHalt, HALT, HaltWatch, haltContractFrom, STATUS_ARGS } from "../src/halt.ts";
import { LAUNCH_RESERVE, launchDue, type LaunchHost, type LaunchRec, maybeLaunch, parseLaunch } from "../src/launch.ts";
import { canonical, defaultLaunch, defaultSpecV2, defaultTrade, specSha256, validateSpec } from "../src/spec.ts";

const seed = (s: string) => createHash("sha256").update(s).digest();
const agent = walletFromSeed(seed("launch-agent"));
const creator = walletFromSeed(seed("launch-creator"));
const H = 3_600_000;

function host(opt: Partial<LaunchHost> & { answer?: string; bal?: bigint; capLeft?: bigint } = {}) {
  const { answer, bal, capLeft, ...over } = opt;
  const calls: { body: any; signers: Wallet[] }[] = [];
  const notes: string[] = [];
  const receipts: string[] = [];
  const blobs: Buffer[] = [];
  let now = 1_000 * H;
  let spentBooked = 0n;
  const h: LaunchHost & { calls: typeof calls; notes: string[]; receipts: string[]; blobs: Buffer[]; tick: (ms: number) => void; booked: () => bigint } = {
    address: agent.address, handle: "tide_ledger", operator: creator.address, specSha256: "ab".repeat(32), persona: "A careful trader.",
    apiUrl: "https://api.example.invalid", wallet: agent, launch: defaultLaunch({ max_dev_buy_lamports: "20000000" }), launches: [] as LaunchRec[], consideredAt: 0,
    now: () => now, haltLaunch: () => "", canSpendSol: (x) => x <= (capLeft ?? 1_000_000_000n), bookSol: (x) => { spentBooked += x; },
    lamports: async () => bal ?? 100_000_000n, record: () => "counters {}",
    think: async () => ({ text: answer ?? "LAUNCH: YES\nNAME: Tide Ledger\nSYMBOL: TIDE\nDESCRIPTION: A token for patient traders.", requestId: "req_1" }),
    storeMeta: async (m) => { const b = Buffer.from(JSON.stringify(m)); blobs.push(b); return { id: createHash("sha256").update(b).digest("hex") }; },
    build: async (body, signers) => { calls.push({ body, signers }); return { signature: "5".repeat(88), summary: { quote_tokens_out_spl: "1000" } }; },
    confirm: async () => ({ ok: true, err: null }), receipt: async (t) => { receipts.push(t); return "posthash"; },
    note: (k, t) => { notes.push(`${k}: ${t}`); }, save: () => {}, tradeOwn: async () => {},
    calls, notes, receipts, blobs, tick: (ms) => { now += ms; }, booked: () => spentBooked,
    ...over,
  } as any;
  return h;
}

test("launch: first period waits, then one labelled create with both signers and a receipt", async () => {
  const h = host();
  assert.equal(await maybeLaunch(h), "first_wait");
  assert.equal(await maybeLaunch(h), "not_due");
  h.tick(24 * H);
  assert.equal(await maybeLaunch(h), "launched");
  assert.equal(h.calls.length, 1);
  const { body, signers } = h.calls[0];
  assert.equal(body.creator, agent.address);
  assert.equal(body.dev_buy_lamports, "20000000", "dev buy = the spec cap when the wallet has room");
  assert.deepEqual([body.name, body.symbol], ["Tide Ledger", "TIDE"]);
  assert.equal(signers.length, 2);
  assert.equal(signers[0].address, agent.address);
  assert.equal(signers[1].address, body.mint, "the fresh mint key signs");
  assert.ok(body.uri.startsWith("https://api.example.invalid/v1/l/") && Buffer.byteLength(body.uri) <= 90, body.uri);
  const meta = JSON.parse(h.blobs[0].toString());
  assert.equal(meta.format, "cadre-launch-meta-v1");
  assert.equal(meta.agent_launched, true);
  assert.equal(meta.agent, agent.address);
  assert.equal(meta.agent_creator, creator.address);
  assert.equal(meta.thought_ref, "req_1");
  assert.ok(meta.description.startsWith("Launched by the AI agent @tide_ledger on Cadre"));
  assert.match(h.receipts[0], /^pump create_v2: agent-launched token TIDE "Tide Ledger" mint \S+; dev buy 20000000 lamports/);
  assert.match(h.receipts[0], /decided by req_1$/);
  assert.equal(h.launches.length, 1);
  assert.ok(h.booked() > 0n, "the SOL spend is booked against the day's cap");
});

test("launch: rate limits per week and between launches", () => {
  const l = defaultLaunch({ max_per_week: 2, min_hours_between: 24, consider_every_h: 6 });
  const now = 100 * 24 * H;
  const rec = (at: number) => ({ at } as LaunchRec);
  assert.equal(launchDue(l, [], 0, now), "");
  assert.match(launchDue(l, [rec(now - 2 * H)], 0, now), /less than 24 h ago/);
  assert.match(launchDue(l, [rec(now - 30 * H), rec(now - 60 * H)], 0, now), /2 launch\(es\) in the last 7 days/);
  assert.equal(launchDue(l, [rec(now - 8 * 24 * H), rec(now - 30 * H)], 0, now), "", "launches older than 7 days do not count");
  assert.equal(launchDue(l, [], now - H, now), "not due");
});

test("launch: dev buy is capped by the wallet reserve, the SOL cap and the halt; no launch without durable metadata", async () => {
  const poor = host({ bal: LAUNCH_RESERVE + 5_000_000n });
  poor.consideredAt = 1;
  assert.equal(await maybeLaunch(poor), "launched");
  assert.equal(poor.calls[0].body.dev_buy_lamports, "5000000", "never spends the reserve");
  const broke = host({ bal: LAUNCH_RESERVE - 1n });
  broke.consideredAt = 1;
  assert.equal(await maybeLaunch(broke), "no_funds");
  assert.equal(broke.calls.length, 0);
  const capped = host({ capLeft: 1_000n });
  capped.consideredAt = 1;
  assert.equal(await maybeLaunch(capped), "budget");
  assert.equal(capped.calls.length, 0);
  const noStore = host({ storeMeta: async () => { throw new Error("503 unavailable"); } });
  noStore.consideredAt = 1;
  assert.equal(await maybeLaunch(noStore), "failed");
  assert.equal(noStore.calls.length, 0, "no durable metadata, no launch");
  const halted = host({ haltLaunch: () => "protocol halt of launch (seq 3): incident" });
  halted.consideredAt = 1;
  assert.equal(await maybeLaunch(halted), "halted");
  assert.equal(halted.calls.length, 0);
});

test("launch: NO, bad symbols and filtered text launch nothing", async () => {
  assert.equal(parseLaunch("LAUNCH: NO"), null);
  assert.equal(parseLaunch("LAUNCH: YES\nNAME: Ok\nSYMBOL: bad-sym\nDESCRIPTION: fine"), null);
  assert.equal(parseLaunch("LAUNCH: YES\nNAME: Moon Money\nSYMBOL: MOON\nDESCRIPTION: Guaranteed 100x returns for every holder."), null, "the output filter refuses return promises");
  assert.equal(parseLaunch("LAUNCH: YES\nNAME: A name that is far too long for pump fun names\nSYMBOL: OK\nDESCRIPTION: fine"), null);
  assert.deepEqual(parseLaunch("LAUNCH: yes\nNAME: \"Moss\"\nSYMBOL: $moss\nDESCRIPTION: Slow and green."), { name: "Moss", symbol: "MOSS", description: "Slow and green." });
  const h = host({ answer: "LAUNCH: NO" });
  h.consideredAt = 1;
  assert.equal(await maybeLaunch(h), "judged_no");
  assert.equal(h.calls.length, 0);
});

test("halt: record layout, scopes, fail closed, off", async () => {
  const rec = (scope: number, reason: string) => Buffer.concat([Buffer.of(scope), Buffer.from(new BigUint64Array([7n]).buffer), Buffer.from(new BigUint64Array([4900n]).buffer), creator.publicKey, Buffer.from(new Uint32Array([Buffer.byteLength(reason)]).buffer), Buffer.from(reason)]);
  const d = decodeHalt(rec(HALT.trade, "trading paused"));
  assert.deepEqual([d.scope, d.seq, d.height, d.authority, d.reason], [2, 7n, 4900n, b58encode(creator.publicKey), "trading paused"]);
  assert.throws(() => decodeHalt(Buffer.alloc(10)));
  let now = 0;
  let answer: any = { ok: true, return: rec(0, "").toString("hex") };
  const seen: string[] = [];
  const w = new HaltWatch({ contract: "Halt111", query: async (_c, args) => { seen.push(args); return answer; }, now: () => now, everyMs: 1_000, maxStaleMs: 120_000 });
  await w.refresh();
  assert.equal(seen[0], STATUS_ARGS.toString("hex"));
  assert.equal(w.blocks("all"), "");
  answer = { ok: true, return: rec(HALT.trade | HALT.launch, "market incident").toString("hex") };
  now += 2_000;
  await w.refresh();
  assert.equal(w.blocks("all"), "", "a trade + launch halt leaves posting on");
  assert.match(w.blocks("trade"), /protocol halt of trade \(seq 7\): market incident/);
  assert.match(w.blocks("launch"), /protocol halt of launch/);
  answer = { ok: true, return: rec(HALT.all, "everything").toString("hex") };
  now += 2_000;
  await w.refresh();
  assert.match(w.blocks("all"), /protocol halt \(seq 7\): everything/);
  assert.match(w.blocks("trade"), /protocol halt/);
  // the node stops answering: stale past 2 minutes = fail closed, even after a clear record
  answer = { ok: true, return: rec(0, "").toString("hex") };
  now += 2_000;
  await w.refresh();
  assert.equal(w.blocks("all"), "");
  answer = { ok: false, error: "unreachable" };
  now += 60_000;
  await w.refresh();
  assert.equal(w.blocks("trade"), "", "inside the window the last good record holds");
  now += 70_000;
  await w.refresh();
  assert.match(w.blocks("trade"), /could not be read for \d+ s .*failing closed/);
  const off = new HaltWatch({ contract: null, query: async () => { throw new Error("never"); } });
  await off.refresh();
  assert.equal(off.blocks("all"), "");
  assert.equal(haltContractFrom({ AGENTS_HALT_CONTRACT: "off" }), null);
  assert.equal(haltContractFrom({ AGENTS_HALT_CONTRACT: "Abc" }), "Abc");
  assert.equal(haltContractFrom({}, "/nonexistent/halt.json"), null);
});

test("spec: the launch section is v2-only, opt-in and bounded", () => {
  const base = defaultSpecV2(agent.address, creator.address, { handle: "tide_ledger", name: "Tide", created_ms: 1, budget: { sol_lamports_per_day: "50000000", cadre_units_per_day: "1000000000", rows_per_day: 4096 } });
  assert.deepEqual(validateSpec(base), []);
  // a v2 spec signed before 2026-10-22 has no launch key: its canonical bytes and hash are unchanged
  assert.ok(!canonical(base).includes("launch"));
  const withLaunch = { ...base, skills: [...base.skills, "launch"], actions: { ...base.actions, launch: true }, launch: defaultLaunch() };
  assert.deepEqual(validateSpec(withLaunch), []);
  assert.notEqual(specSha256(withLaunch), specSha256(base));
  assert.ok(validateSpec({ ...withLaunch, launch: null }).some((e) => /launch skill needs a launch section/.test(e)));
  assert.ok(validateSpec({ ...base, launch: defaultLaunch() }).some((e) => /needs the launch skill/.test(e)));
  assert.ok(validateSpec({ ...withLaunch, launch: defaultLaunch({ max_per_week: 9 }) }).some((e) => /max_per_week/.test(e)));
  assert.ok(validateSpec({ ...withLaunch, launch: defaultLaunch({ max_dev_buy_lamports: "60000000" }) }).some((e) => /at most budget.sol_lamports_per_day/.test(e)));
  assert.ok(validateSpec({ ...withLaunch, launch: defaultLaunch({ trade_own: true }) }).some((e) => /trade_own needs the trade skill/.test(e)));
  const trader = { ...withLaunch, skills: [...withLaunch.skills, "trade"], actions: { ...withLaunch.actions, trade: true }, trade: defaultTrade(agent.address), launch: defaultLaunch({ trade_own: true }) };
  assert.deepEqual(validateSpec(trader), []);
  const v1 = { ...base, format: "cadre-agent-spec-v1", sovereign: undefined, trade: undefined, evolve: undefined, launch: defaultLaunch() };
  assert.ok(validateSpec(v1).some((e) => /launch is a v2 field/.test(e)));
});
