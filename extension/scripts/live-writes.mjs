// Live write check against the real stack (`DEMO_KEEP_UP=1 scripts/demo.sh`): drives the
// popup in Playwright's own headless Chromium (throwaway profile, never the owner's
// Chrome) with a funded throwaway key and does, from the wallet UI:
//   1. bridge in (Solana bridge.deposit -> relayer BridgeUnlock), checked on the node;
//   2. a PostJob with matrices from `pearl-job gen`, checked with getJob and mined;
//   3. a storefront buy whose hash_a / hash_b the wallet computed (order header
//      recipient || order_terms), checked with getJob and mined (a miner can only mine
//      it if the commitments match the header the chain derives).
// Env: LIVE_KEY (keypair JSON byte array file, holds SOL + SPL tokens + a little native),
//      LIVE_EVALUATOR (base58, the chain attester), LIVE_JOB_DIR (pearl-job gen out dir
//      with manifest.json, A.i8, B.i8, m 64 n 64 k 1024), LIVE_MINE_WAIT_S (default 240).
import { chromium } from "playwright";
import { readFileSync, rmSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { blake3 } from "@noble/hashes/blake3";
import { base58 } from "@scure/base";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "dist");
const shots = resolve(root, "screenshots");
const profile = resolve(root, ".ui-profile-live");
mkdirSync(shots, { recursive: true });
rmSync(profile, { recursive: true, force: true });

const key = Uint8Array.from(JSON.parse(readFileSync(process.env.LIVE_KEY, "utf8")));
const SECRET = base58.encode(key);
const ADDRESS = base58.encode(key.subarray(32));
const EVAL = process.env.LIVE_EVALUATOR;
const JOB_DIR = process.env.LIVE_JOB_DIR;
const MINE_WAIT = Number(process.env.LIVE_MINE_WAIT_S ?? 240) * 1000;
const NODE = "http://127.0.0.1:10630";
const PASSWORD = "live-check-pass-1";

const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
};
const hex = (b) => Buffer.from(b).toString("hex");
const u64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
async function node(method, params = {}) {
  const r = await fetch(NODE, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const j = await r.json();
  if (j.error) throw new Error(`${method}: ${j.error.message}`);
  return j.result;
}
async function waitFor(what, fn, ms) {
  const t0 = Date.now();
  for (;;) {
    const v = await fn().catch(() => null);
    if (v) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out: ${what}`);
    await new Promise((r) => setTimeout(r, 1500));
  }
}

const ctx = await chromium.launchPersistentContext(profile, {
  headless: false,
  args: ["--headless=new", `--disable-extensions-except=${dist}`, `--load-extension=${dist}`, "--no-first-run"],
  viewport: { width: 360, height: 600 },
});
const errors = [];
try {
  let [sw] = ctx.serviceWorkers();
  if (!sw) sw = await ctx.waitForEvent("serviceworker");
  const page = await ctx.newPage();
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("dialog", (d) => d.accept());
  const tab = async (t) => { await page.click(`[data-testid=tab-${t}]`); await page.waitForTimeout(500); };
  const shot = async (name) => { await page.waitForTimeout(300); await page.screenshot({ path: resolve(shots, `${name}.png`) }); };

  await page.goto(`chrome-extension://${new URL(sw.url()).host}/popup.html`);
  await page.click("[data-testid=import-secret]");
  await page.fill("[data-testid=secret-input]", SECRET);
  await page.click("[data-testid=secret-next]");
  await page.fill("[data-testid=pw1]", PASSWORD);
  await page.fill("[data-testid=pw2]", PASSWORD);
  await page.click("[data-testid=create-submit]");
  await page.waitForSelector("[data-testid=acct-btn]");
  await page.waitForFunction(() => !document.querySelector("[data-testid=balances] .skel"), null, { timeout: 20000 });
  check("live: balances load from :10600 + :10630", true, ADDRESS);
  await shot("live-01-home");

  // ---------------- 1. bridge in
  const before = BigInt((await node("getAccount", { account: ADDRESS })).balance);
  await tab("bridge");
  await page.fill("[data-testid=bridge-amount]", "1000");
  await page.click("[data-testid=bridge-submit]");
  await page.waitForSelector("[data-testid=pending-list]", { timeout: 30000 });
  await shot("live-02-bridge-pending");
  const after = await waitFor("bridge-in unlocked on the agent chain", async () => {
    const b = BigInt((await node("getAccount", { account: ADDRESS })).balance);
    return b >= before + 1_000_000_000_000n ? b : null;
  }, 180_000);
  check("live: bridge in 1000 tokens arrives on the agent chain", true, `${before} -> ${after}`);
  await page.waitForTimeout(4000);
  await shot("live-03-bridge-done");

  // ---------------- 2. PostJob
  const manifest = `file://${resolve(JOB_DIR, "manifest.json")}`;
  const info = await node("getChainInfo");
  const acct = await node("getAccount", { account: ADDRESS });
  const expectId = hex(blake3(Buffer.concat([Buffer.from(base58.decode(ADDRESS)), u64(acct.nonce)])));
  await tab("jobs");
  await page.click(".seg button:has-text('Post')");
  await page.fill("input[inputmode=numeric] >> nth=0", "64");
  await page.fill("input[inputmode=numeric] >> nth=1", "64");
  await page.setInputFiles("[data-testid=file-a]", resolve(JOB_DIR, "A.i8"));
  await page.setInputFiles("[data-testid=file-b]", resolve(JOB_DIR, "B.i8"));
  await page.fill("[data-testid=job-uri]", manifest);
  await page.fill("[data-testid=job-w]", "5");
  await page.fill("[data-testid=job-p]", "50"); // P > 0: the job's subsidy cap is alpha x P, so P = 0 is never mineable
  await page.fill("[data-testid=job-deadline]", String(info.height + 2000));
  await page.fill("[data-testid=job-evals]", EVAL);
  await page.click("button:has-text('Preview')");
  await page.waitForSelector("[data-testid=job-preview]", { timeout: 10000 });
  await shot("live-04-post-preview");
  await page.click("[data-testid=job-submit]");
  const pj = await waitFor("PostJob included", () => node("getJob", { job_id: expectId }), 60_000);
  check("live: PostJob from the wallet lands on the chain", pj.status === "open" && pj.header_id === expectId, `job ${expectId.slice(0, 16)} header_id = job_id`);
  await page.waitForSelector("[data-testid=header-check]", { timeout: 15000 });
  check("live: PostJob detail verifies job_key from header_id", (await page.textContent("[data-testid=header-check]")) === "job_key verified");
  await shot("live-05-post-detail");
  await page.click("[aria-label=Back]");

  // ---------------- 3. storefront buy, commitments computed by the wallet
  await page.click(".seg button:has-text('Buy')");
  await page.fill("[data-testid=store-sol]", "0.5");
  await page.fill("[data-testid=store-m]", "64");
  await page.fill("[data-testid=store-n]", "64");
  await page.setInputFiles("[data-testid=store-file-a]", resolve(JOB_DIR, "A.i8"));
  await page.setInputFiles("[data-testid=store-file-b]", resolve(JOB_DIR, "B.i8"));
  await page.fill("[data-testid=store-uri]", manifest);
  await page.fill("[data-testid=store-deadline]", "500");
  await page.fill("[data-testid=store-max-blocks]", "10");
  await page.fill("[data-testid=store-evals]", EVAL);
  await page.click("[data-testid=store-preview]");
  await page.waitForSelector("[data-testid=order-preview]", { timeout: 10000 });
  const kvTitles = async (sel) => page.$$eval(`${sel} .kv`, (els) => els.map((e) => e.getAttribute("title") ?? ""));
  const [, previewTerms, previewKey, previewA, previewB] = await kvTitles("[data-testid=order-preview]");
  await shot("live-06-buy-preview");
  await page.click("[data-testid=store-buy]");
  await page.waitForSelector("[data-testid=order-placed]", { timeout: 90_000 });
  await shot("live-07-buy-placed");
  const placedTitles = await kvTitles("[data-testid=order-placed]");
  const orderJobId = placedTitles[2];
  check("live: storefront buy placed (open_order + buy_job)", /^[0-9a-f]{64}$/.test(orderJobId), orderJobId);
  const oj = await waitFor("order job unlocked on the chain", () => node("getJob", { job_id: orderJobId }), 240_000);
  check("live: order job hash_a / hash_b = the wallet's", oj.hash_a === previewA && oj.hash_b === previewB, `${oj.hash_a.slice(0, 12)} / ${oj.hash_b.slice(0, 12)}`);
  check("live: order job header_id = the buyer, terms = order_terms", oj.header_id === hex(base58.decode(ADDRESS)) && oj.terms_hash === previewTerms && oj.job_key === previewKey, `job_key ${oj.job_key.slice(0, 12)}`);

  // ---------------- mined: the miner accepted both jobs' commitments
  const mined = await waitFor("both jobs mined at least once", async () => {
    const [a, b] = await Promise.all([node("getJob", { job_id: expectId }), node("getJob", { job_id: orderJobId })]);
    return a.blocks_mined > 0 && b.blocks_mined > 0 ? { post: a.blocks_mined, order: b.blocks_mined } : null;
  }, MINE_WAIT).catch((e) => ({ error: e.message }));
  if (mined.error) {
    const [a, b] = await Promise.all([node("getJob", { job_id: expectId }), node("getJob", { job_id: orderJobId })]);
    check("live: miner mined both jobs", false, `PostJob ${a.blocks_mined} (${a.status}, mineable ${a.mineable}), order ${b.blocks_mined} (${b.status}, mineable ${b.mineable})`);
  } else check("live: miner mined both jobs", true, `PostJob ${mined.post} blocks, order ${mined.order} blocks`);

  await page.click(".seg button:has-text('Market')");
  await page.waitForSelector("[data-testid=job-row]", { timeout: 15000 });
  await shot("live-08-market");
} finally {
  check("live: no page errors", errors.length === 0, errors.join(" | ").slice(0, 300));
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} live checks passed`);
  process.exitCode = failed ? 1 : 0;
}
