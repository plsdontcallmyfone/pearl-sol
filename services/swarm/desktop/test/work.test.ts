// The house agents' jobs (src/work.ts and friends): roster, personas, monitors, quote checks, RSS,
// QA issue tracking, reports, and every job unit against a fake host and a fake market. No network.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createKeyFile, loadWallet, type Wallet } from "../../src/wallet.ts";
import { PERSONAS } from "../../src/personas.ts";
import { profileKeyOf } from "../../src/swarm.ts";
import { handlesOf, jobOf, ROSTER } from "../src/roster.ts";
import { applyRules, MONITORS, type Point, sampleOf } from "../src/monitors.ts";
import { checkAgainstVisits, newsRequest, norm, parseRss, pickStory, quoteMatch, researchTask, verdictOf } from "../src/research.ts";
import { checkReport, lineChart, ReportStore } from "../src/reports.ts";
import { issuesOf, mergeIssues, needsOf, WorkEngine, type WorkHost, workDefaults, type WorkAgent } from "../src/work.ts";
import { readReport, runTask, type Chat } from "../src/vision.ts";
import { Desk, type SandboxLike } from "../src/desk.ts";
import type { StepResult } from "../src/desk.ts";

// ------------------------------------------------------------------ roster and personas

test("roster: every house agent has exactly one job, with outputs; nothing left on filler", () => {
  assert.equal(PERSONAS.length, 30);
  for (const p of PERSONAS) {
    const j = jobOf(p.handle);
    assert.ok(j, `${p.handle} has a job`);
    assert.ok(j!.outputs.length > 10);
    if (j!.kind === "monitor") assert.ok(MONITORS[j!.target!], `${p.handle} monitor target`);
    if (j!.kind === "qa") assert.ok(j!.suite);
    if (j!.kind === "manager") assert.ok(j!.desk);
  }
  assert.equal(Object.keys(ROSTER).length, 30);
  assert.ok(handlesOf("research").length >= 6 && handlesOf("reviewer").length >= 2 && handlesOf("manager").length >= 1);
  // the six two-desktop agents (the end of the persona list) all have work for two desktops
  for (const h of PERSONAS.slice(-6).map((p) => p.handle)) assert.ok(["research", "qa", "manager"].includes(jobOf(h)!.kind), `${h}: two desktops, a job that uses both`);
});

test("personas: plain role descriptions, no character blurbs; a changed text republishes the profile", () => {
  const whimsy = /tinker|garden|tavern|poem|poet|riddle|bake|loaf|monk|ferry|lighthouse|weav|street kid|gossip|appearance|flower|petal|bloom|cloak|apron/i;
  for (const p of PERSONAS) {
    assert.ok(!whimsy.test(`${p.archetype} ${p.persona}`), `${p.handle}: ${p.archetype} / ${p.persona}`);
    assert.equal(p.appearance, "");
    assert.ok(p.persona.length > 60 && p.persona.length < 300);
  }
  const a = PERSONAS[0]!;
  assert.notEqual(profileKeyOf(a), profileKeyOf({ ...a, persona: `${a.persona} x` }));
  assert.equal(profileKeyOf(a), profileKeyOf({ ...a, quiet: [0, 1], cadenceMin: 1 }), "town cadence is not profile text");
});

// ------------------------------------------------------------------ research checks

test("quotes: exact, close (a misread character) or none; normalization folds case, quotes and punctuation", () => {
  const page = "Solana’s devnet RPC allows 100 requests per 10 seconds per IP, according to the docs. Other text.";
  assert.equal(quoteMatch(page, "Solana's devnet RPC allows 100 requests per 10 seconds per IP").match, "exact");
  assert.equal(quoteMatch(page, "devnet RPC allovvs 100 requests per 10 seconds per IP according").match, "close");
  assert.equal(quoteMatch(page, "mainnet RPC allows unlimited requests for everyone forever").match, "none");
  assert.equal(quoteMatch(page, "short").match, "none", "too short to prove anything");
  assert.equal(norm("A\u2014B \u201cc\u201d"), "a b c");
});

test("brief check: a finding counts only on a page opened in the agent's browser with its quote on it", () => {
  const visits = [
    { t: 1, url: "https://html.duckduckgo.com/html/?q=x", title: "search", text: "the quote one is here on the search page" },
    { t: 2, url: "https://www.example.com/a", title: "A", text: "The fee rose to 2.5 percent on Monday according to the filing." },
    { t: 3, url: "https://news.example.org/b/", title: "B", text: "Volume reached 40 million dollars in the first week of trading." },
  ];
  const r = readReport({ title: "T", summary: "S", findings: [
    { claim: "fee", source_url: "https://example.com/a", quote: "The fee rose to 2.5 percent on Monday" },
    { claim: "vol", source_url: "https://news.example.org/b", quote: "Volume reached 40 million dollars in the first week" },
    { claim: "made up", source_url: "https://never-opened.example.net/c", quote: "something nobody opened in this browser at all" },
  ] });
  const c = checkAgainstVisits(r, visits);
  assert.equal(c.ok, true, c.why);
  assert.equal(c.verified, 2);
  assert.equal(c.findings[2]!.visited, false);
  const only = checkAgainstVisits(readReport({ title: "T", summary: "S", findings: [r.findings[0]] }), visits);
  assert.equal(only.ok, false, "one source is not enough");
  // the search page never counts as a source
  const s = checkAgainstVisits(readReport({ title: "T", summary: "S", findings: [{ claim: "x", source_url: "https://html.duckduckgo.com/html/?q=x", quote: "the quote one is here on the search page" }, r.findings[0]] }), visits);
  assert.equal(s.verified, 1);
  // the reviewer's rule
  assert.equal(verdictOf([{ host: "a", match: "exact" }, { host: "b", match: "close" }, { host: "c", match: "none" }]).accept, true);
  assert.equal(verdictOf([{ host: "a", match: "exact" }, { host: "a", match: "exact" }]).accept, false, "two quotes from one site");
  assert.equal(verdictOf([{ host: "a", match: "exact" }, { host: "b", match: "exact" }, { host: "c", match: "none" }, { host: "d", match: "none" }, { host: "e", match: "none" }]).accept, false, "under half found");
  // the task quotes the request as data and opens a search
  const t = researchTask('Brief on a current story: "X happened" (CoinDesk, 2026-10-23).', ["https://www.coindesk.com/x"]);
  assert.ok(t.url.startsWith("https://html.duckduckgo.com/html/?q="));
  assert.ok(t.text.includes('"""') && t.text.includes("https://www.coindesk.com/x") && /never|not/i.test(t.text));
});

test("news: RSS items parsed (CDATA, entities), newest uncommissioned story picked, request names the source", () => {
  const xml = `<rss><channel><item><title><![CDATA[Fund &amp; bank launch a stablecoin]]></title><link>https://www.coindesk.com/a</link><pubDate>Thu, 23 Oct 2026 10:00:00 GMT</pubDate></item>
  <item><title>Older story</title><link>https://www.coindesk.com/b</link><pubDate>Wed, 22 Oct 2026 10:00:00 GMT</pubDate></item><item><title>No link</title></item></channel></rss>`;
  const items = parseRss(xml, "CoinDesk");
  assert.equal(items.length, 2);
  assert.equal(items[0]!.title, "Fund & bank launch a stablecoin");
  const now = Date.parse("2026-10-23T12:00:00Z");
  assert.equal(pickStory(items, new Set(), now)!.link, "https://www.coindesk.com/a");
  assert.equal(pickStory(items, new Set(["https://www.coindesk.com/a"]), now)!.link, "https://www.coindesk.com/b");
  assert.equal(pickStory(items, new Set(), now + 5 * 86_400_000), null, "stale stories are not commissioned");
  assert.match(newsRequest(items[0]!), /CoinDesk, 2026-10-23.*other than coindesk\.com/);
});

// ------------------------------------------------------------------ monitors

const pts = (vals: Record<string, number | null>[], stepMs = 300_000, t0 = 1_800_000_000_000): Point[] => vals.map((s, i) => ({ t: t0 + i * stepMs, s }));

test("monitors: light client lag opens a finding and clears it; a stuck chain is a stall; sampleOf reads real fields", async () => {
  const def = MONITORS.bridge;
  let open: any[] = [];
  const p1 = pts([{ lc_lag: 10, lc_tip: 100, delta_spl: 0 }, { lc_lag: 1793, lc_tip: 100, delta_spl: 0 }]);
  let r = applyRules(def, p1, open, p1[1]!.t);
  assert.equal(r.opened.length, 1);
  assert.match(r.opened[0]!.text, /1793 blocks behind/);
  open = r.open;
  const p2 = [...p1, { t: p1[1]!.t + 300_000, s: { lc_lag: 20, lc_tip: 2000, delta_spl: 0 } }];
  r = applyRules(def, p2, open, p2[2]!.t);
  assert.equal(r.closed.length, 1, "cleared when the lag is back under the limit");
  // stall: the same height for 5 minutes or more
  const stall = pts([{ height: 9000 }, { height: 9000 }, { height: 9000 }], 150_000);
  assert.ok(applyRules(MONITORS.chain, stall, [], 0).opened.some((f) => f.rule === "stall"));
  const moving = pts([{ height: 9000 }, { height: 9010 }, { height: 9020 }], 150_000);
  assert.ok(!applyRules(MONITORS.chain, moving, [], 0).opened.some((f) => f.rule === "stall"));
  // sampleOf on recorded shapes (box 0, 2026-10-24)
  const summary = { chain: { height: 9815, finalized_height: 9755 }, light_client: { tip_height: 8022, final_height: 7962 }, storefront: { orders: 130, by_status: { pending: 88, finalized: 36, refunded: 6 }, pending_by_stage: { awaiting_light_client: 1, job_open: 87, job_unknown: 0 }, sol_pending: 887497521 }, blocks: { total: 9815, paid_job_blocks: 5854, paid_share: "59.6%" } };
  const ctx = { api: async (p: string) => (p === "/v1/summary" ? summary : p === "/v1/bridge" ? { invariant: { delta_spl: 0 } } : null), node: async () => null, now: () => 1_800_000_000_000 };
  const b = await sampleOf("bridge", ctx as any, []);
  assert.equal(b.s.lc_lag, 1793);
  const sf = await sampleOf("storefront", ctx as any, []);
  assert.equal(sf.s.pending, 88);
  assert.equal(sf.s.sol_pending, 0.887497521);
  const c = await sampleOf("chain", ctx as any, []);
  assert.equal(c.s.finality_lag, 60);
  assert.equal(c.s.paid_share, 59.6);
  // wallets: an agent under its job's minimum is listed
  const w = await sampleOf("wallets", { ...ctx, node: async (_m: string, p: any) => ({ balance: p.account === "A" ? "49996000" : "5000000000" }), solBalances: async () => [0.045, 0.5], houseAccounts: () => [{ handle: "mgr", account: "A", needs: { cadre: 3, sol: 0 } }, { handle: "res", account: "B", needs: { cadre: 0.3, sol: 0 } }] } as any, []);
  assert.equal(w.s.low, 1);
  assert.deepEqual((w.detail as any).low, ["mgr"]);
});

// ------------------------------------------------------------------ QA

const step = (o: Partial<StepResult>): StepResult => ({ url: "", final_url: null, status: 200, ms: 900, title: "", excerpt: "", text_len: 100, found: null, console_errors: [], page_errors: [], failed_requests: [], bad_responses: [], overflow_px: null, error: null, ...o });

test("QA: issues from a crawl step, deduplicated across runs, closed when a run no longer sees them", () => {
  const r = step({ status: 200, console_errors: ["Refused to frame 'https://auth.privy.io/' because an ancestor violates the Content Security Policy"], bad_responses: [{ url: "https://api.x/v1/market/offers?x=1", status: 502 }], failed_requests: [{ url: "https://x/_next/data.json", error: "net::ERR_ABORTED" }], excerpt: "Market offers Unavailable: the node did not answer.", overflow_px: 40 });
  const iss = issuesOf(r, "/market", { mobile: true });
  assert.deepEqual(iss.map((i) => i.kind).sort(), ["console_error", "request_error", "shows_unavailable", "sideways_scroll"].sort(), "ERR_ABORTED is not a bug");
  const at1 = "2026-10-24T00:00:00Z", at2 = "2026-10-24T02:00:00Z";
  const m1 = mergeIssues([], iss.map((i) => ({ page: "/market", ...i })), new Set(["/market"]), at1);
  assert.equal(m1.fresh.length, 4);
  const m2 = mergeIssues(m1.all, iss.slice(0, 2).map((i) => ({ page: "/market", ...i })), new Set(["/market"]), at2);
  assert.equal(m2.fresh.length, 0, "seen again, not new");
  assert.equal(m2.all.filter((i) => i.open).length, 2);
  assert.equal(m2.all.find((i) => i.kind === iss[0]!.kind)!.times, 2);
  assert.equal(issuesOf(step({ error: "net::ERR_NAME_NOT_RESOLVED" }), "/")[0]!.kind, "load_failed");
  assert.equal(issuesOf(step({ status: 404 }), "/x")[0]!.kind, "http_error");
});

// ------------------------------------------------------------------ reports

test("reports: stored with a checkable sha256, newest first, capped per agent; charts render", () => {
  const dir = mkdtempSync(join(tmpdir(), "reports-"));
  const st = new ReportStore(dir, 3);
  for (let i = 0; i < 5; i++) st.put({ agent: "a1", job: "j", kind: "monitor", title: `r${i}`, summary: "", body_md: "# x", links: [], receipts: {}, data: { i }, created: `2026-10-24T00:00:0${i}.000Z` });
  const l = st.list("a1");
  assert.deepEqual(l.map((r) => r.title), ["r4", "r3", "r2"]);
  assert.ok(l.every(checkReport));
  assert.equal(checkReport({ ...l[0]!, title: "edited" }), false);
  assert.equal(new ReportStore(dir, 3).list("a1").length, 3, "reloaded from disk");
  const svg = lineChart([{ t: 0, v: 1 }, { t: 60_000, v: 3 }], { title: "lag", unit: "blocks", threshold: 2 });
  assert.match(svg, /^<svg/); assert.match(svg, /polyline/); assert.match(svg, /limit 2/);
  rmSync(dir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ the engine against fakes

class FakeMarket {
  height = 1000;
  offersList: any[] = [];
  hiresList: any[] = [];
  blobs = new Map<string, any>();
  calls: string[] = [];
  contract = "MARKET";
  ix = { quote: async (_id: number, jf: bigint) => ({ price: "200000000", total: (200_000_000n + 40_000_000n + jf).toString() }) };
  async refresh() { return this; }
  offers() { return this.offersList; }
  hires() { return this.hiresList; }
  async getBlob(uri: string) { return this.blobs.get(uri) ?? null; }
  async ensureOffer(w: Wallet, _h: string, o: any) { let x = this.offersList.find((y) => y.provider === w.address); if (!x) { x = { id: this.offersList.length, provider: w.address, title: o.title, active: true, price: o.price.toString(), turnaround: o.turnaround, hires: 0 }; this.offersList.push(x); this.calls.push(`offer ${x.id}`); } return x; }
  async hire(buyer: Wallet, _h: string, offer: any, request: any, evaluator: string, jf: bigint, deadline: number) {
    const id = this.hiresList.length;
    this.blobs.set(`blob:req${id}`, { request: request.request, links: request.links });
    this.hiresList.push({ id, offer: offer.id, buyer: buyer.address, provider: offer.provider, status: "open", deadline: this.height + deadline, evaluators: [evaluator], voted: [], request_uri: `blob:req${id}`, result_uri: null });
    this.calls.push(`hire ${id} -> ${offer.provider.slice(0, 4)} eval ${evaluator.slice(0, 4)}`);
    return { id, tx: `tx-hire-${id}`, total: 240_000_000n + jf };
  }
  async deliver(_w: Wallet, _h: string, hire: number, doc: any) { const h = this.hiresList[hire]; h.status = "delivered"; h.result_uri = `blob:res${hire}`; this.blobs.set(h.result_uri, doc); this.calls.push(`deliver ${hire}`); return { tx: `tx-del-${hire}`, uri: h.result_uri, id: "x" }; }
  async verdict(w: Wallet, _h: string, hire: number, accept: boolean, doc: any) { const h = this.hiresList[hire]; h.voted.push(w.address); h.status = accept ? "completed" : "rejected"; this.calls.push(`verdict ${hire} ${accept}`); this.blobs.set(`blob:v${hire}`, doc); return { tx: `tx-v-${hire}`, uri: `blob:v${hire}` }; }
  async decline(_w: Wallet, _h: string, hire: number, reason: string) { this.hiresList[hire].status = "declined"; this.calls.push(`decline ${hire}: ${reason}`); return { tx: `tx-dec-${hire}`, height: 1, ret: null }; }
  async expire(_w: Wallet, _h: string, hire: number) { this.hiresList[hire].status = "expired"; this.calls.push(`expire ${hire}`); return { tx: `tx-exp-${hire}`, height: 1, ret: null }; }
}

function harness(handles: string[], over: Partial<WorkHost> = {}) {
  const dir = mkdtempSync(join(tmpdir(), "work-"));
  const agents: WorkAgent[] = handles.map((h) => { createKeyFile(join(dir, `${h}.json`)); return { handle: h, name: h, wallet: loadWallet(join(dir, `${h}.json`)), roles: ["social"], job: jobOf(h)! }; });
  const notes: string[] = [], scripts: { h: string; kind: string; steps: any[] }[] = [], events: any[] = [];
  const market = new FakeMarket();
  let t = Date.parse("2026-10-24T01:00:00Z");
  const host: WorkHost = {
    site: "https://cadre.example", explorer: "https://cadre.example/explorer", publicApi: "https://api.example",
    now: () => t, event: (e) => events.push(e), note: (h, k, text) => notes.push(`${h} ${k}: ${text}`),
    freeRoles: () => ["social"],
    script: async (h, _r, kind, steps) => { scripts.push({ h, kind, steps }); return steps.map((s) => step({ url: s.url ?? "", found: s.highlight ? true : null })); },
    vision: async () => null, visionReady: () => true,
    api: async () => ({}), node: async () => ({ balance: "5000000000" }),
    fetchPage: async () => ({ status: 200, url: "", title: "", text: "" }), fetchRaw: async () => "",
    self: () => ({ spent: 10, cap: 64, e2b: 9.5, vision: 0.5, unhealthy: 0, desktops: 36, running: 36, vision_paused: false, seconds_left: 3600, running_usd_per_s: 0.0007 }),
    practice: () => null, fund: async () => "tx-fund", market: market as any, ...over,
  };
  const store = new ReportStore(join(dir, "reports"));
  const eng = new WorkEngine(host, agents, store, workDefaults(join(dir, "work.json")));
  const drain = async () => { for (let i = 0; i < 200 && eng.running.size; i++) await new Promise((r) => setTimeout(r, 5)); };
  return { dir, agents, notes, scripts, events, market, host, store, eng, drain, advance: (ms: number) => { t += ms; } };
}

test("monitor job: samples, opens a finding, shows the page on its desktop, writes a report with a chart", async () => {
  const H = harness(["celandine62"], { api: async (p: string) => (p === "/v1/summary" ? { chain: { height: 9815 }, light_client: { tip_height: 8022 } } : { invariant: { delta_spl: 0 } }) });
  await H.eng.tick();
  await H.drain();
  const reps = H.store.list("celandine62");
  assert.equal(reps.length, 1);
  assert.match(reps[0]!.title, /light client more than 200 blocks/);
  assert.match(reps[0]!.body_md, /1793 blocks behind/);
  assert.match(reps[0]!.chart_svg ?? "", /<svg/);
  assert.equal(H.scripts[0]!.kind, "monitor_show");
  assert.ok(H.notes.some((n) => n.includes("finding")));
  // the next sample within the interval does nothing; after it, one more sample, no new report (same finding, under an hour)
  await H.eng.tick(); await H.drain();
  assert.equal(H.store.list("celandine62").length, 1);
  H.advance(300_000);
  await H.eng.tick(); await H.drain();
  assert.equal(H.eng.st.agents.celandine62!.mon!.pts.length, 2);
  assert.equal(H.store.list("celandine62").length, 1);
  rmSync(H.dir, { recursive: true, force: true });
});

test("market jobs: the manager hires the least busy research desk, the desk researches and delivers only checked briefs, the reviewer checks sources and votes", async () => {
  const pageA = "The exchange said fees will fall to 0.1 percent starting next month for all users.";
  const pageB = "Analysts at the firm expect trading volume to double by the end of the year.";
  const visionCalls: any[] = [];
  const H = harness(["tansywren82", "nigella71", "lunaria38", "helleborefawn39"], {
    vision: async (_h, _r, t) => {
      visionCalls.push(t);
      return { ok: true, summary: "s", steps: 9, usd: 0.006, inTokens: 1, outTokens: 1, log: [], visits: [{ t: 1, url: "https://a.example/news", title: "A", text: pageA }, { t: 2, url: "https://b.example/report", title: "B", text: pageB }],
        report: readReport({ title: "Fees and volume", summary: "Fees fall; volume may double.", findings: [{ claim: "fees fall", source_url: "https://a.example/news", quote: "fees will fall to 0.1 percent starting next month" }, { claim: "volume", source_url: "https://b.example/report", quote: "expect trading volume to double by the end of the year" }] }) } as any;
    },
    fetchPage: async (u: string) => ({ status: 200, url: u, title: "", text: u.includes("a.example") ? pageA : pageB }),
  });
  // research desks list their offers first
  await H.eng.research(H.agents[1]!); await H.eng.research(H.agents[2]!);
  assert.equal(H.market.offersList.length, 2);
  // the ops lead hires one of them, naming a reviewer
  await H.eng.manager(H.agents[0]!);
  assert.equal(H.market.hiresList.length, 1);
  const hire = H.market.hiresList[0];
  assert.equal(hire.evaluators[0], H.agents[3]!.wallet.address, "the house reviewer is the evaluator");
  assert.ok(H.store.list("tansywren82")[0]!.kind === "commission");
  // the hired desk researches (vision with receipts) and delivers
  const desk = H.agents.find((a) => a.wallet.address === hire.provider)!;
  await H.eng.research(desk); await H.drain();
  assert.equal(visionCalls.length, 1);
  assert.equal(visionCalls[0].record, true, "pages are captured as receipts");
  assert.equal(visionCalls[0].report, true);
  assert.equal(hire.status, "delivered");
  const doc = H.market.blobs.get(hire.result_uri);
  assert.equal(doc.check.verified, 2);
  assert.equal(doc.pages_opened.length, 2);
  assert.match(doc.written_by.note, /not a paid Cadre thought/);
  assert.equal(H.store.list(desk.handle)[0]!.kind, "research");
  // a second manager pass hires the OTHER desk (least busy)
  H.eng.st.agents.tansywren82!.manager!.lastAt = 0;
  await H.eng.manager(H.agents[0]!);
  assert.notEqual(H.market.hiresList[1].provider, hire.provider);
  // the reviewer fetches both pages, finds both quotes, accepts, shows the sources highlighted
  await H.eng.reviewer(H.agents[3]!);
  assert.equal(hire.status, "completed");
  assert.ok(H.market.calls.includes("verdict 0 true"));
  assert.ok(H.scripts.some((s) => s.kind === "check_sources" && s.steps.every((x: any) => x.highlight)));
  assert.match(H.store.list("helleborefawn39")[0]!.body_md, /no language model/);
  rmSync(H.dir, { recursive: true, force: true });
});

test("research: a brief whose sources were not opened is not delivered; after two tries the hire is declined (buyer refunded)", async () => {
  const H = harness(["nigella71", "tansywren82", "helleborefawn39"], {
    vision: async () => ({ ok: true, summary: "s", steps: 5, usd: 0.003, inTokens: 1, outTokens: 1, log: [], visits: [], report: readReport({ title: "T", summary: "S", findings: [{ claim: "x", source_url: "https://made.up/a", quote: "a quote from a page nobody opened at all" }, { claim: "y", source_url: "https://made.up/b", quote: "another quote from a page nobody opened" }] }) } as any),
  });
  await H.eng.tick(); await H.drain();
  H.eng.st.agents.tansywren82!.manager!.lastAt = 0;
  await H.eng.manager(H.agents[1]!);
  await H.eng.research(H.agents[0]!); await H.drain();
  assert.equal(H.market.hiresList[0].status, "open", "not delivered after one failed check");
  await H.eng.research(H.agents[0]!); await H.drain();
  assert.equal(H.market.hiresList[0].status, "declined");
  assert.ok(H.market.calls.some((c) => c.startsWith("decline 0")));
  rmSync(H.dir, { recursive: true, force: true });
});

test("reviewer: quotes missing from the cited pages are rejected with the table; a manager expires its overdue hires", async () => {
  const H = harness(["gentianbloom68", "penstemon72"], { fetchPage: async (u: string) => ({ status: 200, url: u, title: "", text: "nothing relevant here at all" }), script: async (_h, _r, _k, steps) => steps.map((s) => step({ url: s.url ?? "", found: false })) });
  const rv = H.agents[0]!;
  H.market.hiresList.push({ id: 0, provider: "P", buyer: H.agents[1]!.wallet.address, status: "delivered", deadline: 2000, evaluators: [rv.wallet.address], voted: [], result_uri: "blob:r0" });
  H.market.blobs.set("blob:r0", { title: "T", findings: [{ claim: "a", source_url: "https://x.example/1", quote: "the quote that is not on that page" }, { claim: "b", source_url: "https://y.example/2", quote: "and neither is this second one here" }] });
  await H.eng.reviewer(rv);
  assert.equal(H.market.hiresList[0].status, "rejected");
  assert.match(H.store.list("gentianbloom68")[0]!.body_md, /quote not on the page/);
  // overdue: the buyer expires it
  H.market.hiresList.push({ id: 1, provider: "P", buyer: H.agents[1]!.wallet.address, status: "open", deadline: 900, evaluators: [], voted: [] });
  await H.eng.manager(H.agents[1]!);
  assert.equal(H.market.hiresList[1].status, "expired");
  rmSync(H.dir, { recursive: true, force: true });
});

test("QA, trader, digest and funding units", async () => {
  let practice: any = null;
  let funded = 0n;
  const H = harness(["delphinium37", "foxglove47", "yarrowvale63", "penstemon72"], {
    script: async (_h, _r, kind, steps) => steps.map((s, i) => step({ url: s.url ?? "", console_errors: kind === "qa_site" && i === 0 ? ["TypeError: x is undefined"] : [] })),
    practice: () => practice,
    node: async () => ({ balance: "100000000" }),
    fund: async (_h, u) => { funded += u; return "tx-fund"; },
  });
  await H.eng.tick(); await H.drain();
  // QA: one crawl, a bug list with the console error
  const qa = H.store.list("delphinium37")[0]!;
  assert.equal(qa.kind, "qa");
  assert.match(qa.body_md, /console_error/);
  assert.ok(H.notes.some((n) => n.includes("waiting for cadre-practice")));
  // funding: the manager (float 10) and the trader (float 2) were topped up from 0.1 CADRE
  assert.equal(funded, (10_000_000_000n - 100_000_000n) + (2_000_000_000n - 100_000_000n));
  // trader: a closed practice trade becomes a report
  practice = { version: 7, open: null, history: [{ entry: 41, version: 7, outcome: "win", pnl: "1200000", buckets: 0 }] };
  await H.eng.trader(H.agents[1]!);
  assert.match(H.store.list("foxglove47")[0]!.title, /Practice trade #41: win/);
  // digest: links what the others reported
  await H.eng.digest(H.agents[2]!);
  const d = H.store.list("yarrowvale63")[0]!;
  assert.match(d.body_md, /Practice trade #41/);
  assert.match(d.body_md, /QA site pages/);
  rmSync(H.dir, { recursive: true, force: true });
});

test("funding cap: the operator never sends more than the day's cap", async () => {
  let sent = 0;
  const H = harness(["penstemon72", "tansywren82", "nigella71"], { node: async () => ({ balance: "0" }), fund: async () => { sent++; return "tx"; } });
  H.eng.cfg.fundCapCadre = 12;
  await H.eng.tick(); await H.drain();
  assert.equal(sent, 2, "10 CADRE to the first manager and 1 to the desk fit 12; the second manager's 10 does not");
  assert.ok(H.events.some((e) => e.event === "work_fund_refused"));
  assert.deepEqual(needsOf(jobOf("foxglove47")!), { cadre: 0.5, sol: 0.03 });
  rmSync(H.dir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ desktop plumbing

test("vision: submit_report is offered only to work tasks and ends the task with the report", async () => {
  const sbx: any = { screenshot: async () => new Uint8Array([1]), press: async () => {}, write: async () => {}, leftClick: async () => {} };
  const tools: string[][] = [];
  const chat: Chat = async (b: any) => {
    tools.push(b.tools.map((t: any) => t.function.name));
    return { status: 200, json: { choices: [{ message: { content: "done", tool_calls: [{ id: "1", type: "function", function: { name: "submit_report", arguments: JSON.stringify({ title: "T", summary: "S", findings: [{ claim: "c", source_url: "https://a/x", quote: "q" }], problems: [{ page: "/m", what: "broken" }] }) } }] } }], usage: { cost: 0.001 } } };
  };
  const r = await runTask(sbx, chat, { model: "m", system: "s", task: "t", maxSteps: 3, maxUsd: 1, report: true });
  assert.equal(r.ok, true);
  assert.equal(r.report!.problems[0]!.what, "broken");
  assert.ok(tools[0]!.includes("submit_report"));
  const r2 = await runTask(sbx, chat, { model: "m", system: "s", task: "t", maxSteps: 1, maxUsd: 1 });
  assert.ok(!tools[1]!.includes("submit_report"));
  assert.equal(r2.report, undefined, "a non-work task ignores a stray submit_report");
});

test("desk: scripted steps go through the browser host's files; recordings come back as visits", async () => {
  const files = new Map<string, string>();
  const sbx: SandboxLike = {
    sandboxId: "s1",
    commands: { run: async (cmd: string) => {
      if (cmd.startsWith("cat /home/user/.cadre/script-done.json")) {
        const sc = files.get("/home/user/.cadre/script.json");
        if (!sc) return { exitCode: 0, stdout: "", stderr: "" };
        const j = JSON.parse(sc);
        return { exitCode: 0, stdout: JSON.stringify({ id: j.id, results: j.steps.map((s: any) => step({ url: s.url, found: !!s.highlight })) }), stderr: "" };
      }
      return { exitCode: 0, stdout: "", stderr: "" };
    } },
    files: { write: async (p: string, d: any) => { files.set(p, String(d)); }, read: async (p: string) => new TextEncoder().encode(p.endsWith("visits.jsonl") ? `${JSON.stringify({ t: 1, url: "https://a.example/x", title: "A", text: "hello" })}\n` : files.get(p) ?? "") },
    screenshot: async () => new Uint8Array(), leftClick: async () => {}, doubleClick: async () => {}, rightClick: async () => {}, moveMouse: async () => {}, scroll: async () => {}, write: async () => {}, press: async () => {},
  };
  const d = new Desk("h", "social", "addr", {} as any, { template: "t", resolution: [1280, 800], keepaliveS: 1, sessionMaxS: 1, siteUrl: "s", explorerUrl: "e", walletTgz: "", log: () => {} });
  d.sbx = sbx;
  const res = await d.runScript([{ url: "https://a.example/x", highlight: "hello world quote" }], { collect: true, timeoutMs: 5000 });
  assert.equal(res![0]!.found, true);
  assert.equal(JSON.parse(files.get("/home/user/.cadre/script.json")!).collect, true);
  const v = await d.stopRecording();
  assert.equal(v[0]!.url, "https://a.example/x");
});

test("browser host: scripted steps, visit receipts and a locked wallet; no town tab, no wallet tab at start", () => {
  const js = readFileSync(new URL("../sandbox/deskhost.mjs", import.meta.url), "utf8");
  assert.match(js, /script\.json/);
  assert.match(js, /script-done\.json/);
  assert.match(js, /visits\.jsonl/);
  assert.match(js, /chrome\.storage\.session\.remove\('encryptionKey'\)/);
  assert.match(js, /if \(walletLocked\(\)\) return;/);
  assert.match(js, /const startTabs = \[`\$\{site\}\/market`\];/);
  assert.ok(!/\/town`/.test(js), "the browser host never opens the town");
});

test("market ops: concurrent refreshes share one node read", async () => {
  const { MarketOps } = await import("../src/market.ts");
  let reads = 0;
  const rpc = { call: async (m: string) => { if (m === "queryContract") { reads++; await new Promise((r) => setTimeout(r, 20)); return { ok: true, return: "00".repeat(64) }; } if (m === "getChainInfo") return { height: 5 }; if (m === "getContractEvents") return { total: 0, events: [] }; return null; }, putBlob: async () => ({}) };
  const dir = mkdtempSync(join(tmpdir(), "mk-"));
  const mk = new MarketOps({ rpc, sendContract: async () => ({ txHash: "t" }), waitIncluded: async () => ({ height: 1, receipt: { ok: true } }) } as any, "M", dir, () => {});
  await Promise.all(Array.from({ length: 8 }, () => mk.refresh()));
  const once = reads;
  await mk.refresh();
  assert.equal(reads, once, "cached");
  assert.ok(once <= 3, `one refresh (config + counts), not eight: ${once}`);
  rmSync(dir, { recursive: true, force: true });
});

test("manager: a failed hire (float not landed yet) retries in 2 minutes and keeps the question", async () => {
  const H = harness(["tansywren82", "nigella71", "helleborefawn39"]);
  await H.eng.research(H.agents[1]!);
  const orig = H.market.hire.bind(H.market);
  let fail = true;
  (H.market as any).hire = async (...a: any[]) => { if (fail) throw new Error("simulation failed: balance too low"); return (orig as any)(...a); };
  await H.eng.manager(H.agents[0]!);
  const st = H.eng.st.agents.tansywren82!.manager!;
  assert.equal(H.market.hiresList.length, 0);
  assert.equal(st.q, 0, "the same question is asked next time");
  await H.eng.manager(H.agents[0]!);
  assert.equal(H.market.hiresList.length, 0, "not retried at once");
  H.advance(121_000);
  fail = false;
  await H.eng.manager(H.agents[0]!);
  assert.equal(H.market.hiresList.length, 1);
  rmSync(H.dir, { recursive: true, force: true });
});
