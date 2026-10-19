// Headless screenshots of the running explorer (Playwright's bundled headless Chromium,
// borrowed from extension/node_modules; never a real browser profile).
//   node tools/screens.mjs [base=http://127.0.0.1:10670] [outDir=../../screenshots/explorer]
// Also fails on console errors and on any monospace computed font.
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdirSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(resolve(here, "../../../extension/package.json"));
const { chromium } = require("playwright");
const base = process.argv[2] ?? "http://127.0.0.1:10670";
const out = resolve(process.argv[3] ?? resolve(here, "../../../screenshots/explorer"));
mkdirSync(out, { recursive: true });

const j = async (p) => (await fetch(base + p)).json();
const sm = await j("/api/summary");
const feed = await j("/api/feed?filter=paid&limit=200");
const paidBlock = feed.rows[0]?.block?.height;
const verified = feed.rows.map((r) => r.block).find((b) => ["final", "accumulated", "verified"].includes(b.light_client.state));
const jobs = (await j("/api/jobs")).jobs;
const sfJob = jobs.find((x) => x.paid_via === "storefront") ?? jobs[0];
const orders = (await j("/api/orders")).orders;
const pages = [
  ["home", "/"],
  ["blocks-paid", "/blocks?filter=paid"],
  ...(paidBlock ? [["block-paid", `/block/${paidBlock}`]] : []),
  ...(verified ? [["block-verified", `/block/${verified.height}`]] : []),
  ["block-fallback", `/block/${sm.chain.height - 3}`],
  ...(sfJob ? [["job", `/job/${sfJob.job_id}`]] : []),
  ["jobs", "/jobs"],
  ["orders", "/orders"],
  ...(orders[0] ? [["order", `/order/${orders[0].nonce}`]] : []),
  ["api", "/docs"],
  ["revenue", "/revenue"],
];

const browser = await chromium.launch({ headless: true });
let problems = 0;
for (const [scheme, vp, suffix] of [["light", { width: 1366, height: 900 }, ""], ["dark", { width: 1366, height: 900 }, "-dark"], ["light", { width: 390, height: 844 }, "-phone"]]) {
  const ctx = await browser.newContext({ viewport: vp, colorScheme: scheme, deviceScaleFactor: vp.width < 500 ? 2 : 1 });
  const page = await ctx.newPage();
  page.on("console", (m) => { if (m.type() === "error") { problems++; console.log(`console error on ${page.url()}: ${m.text()}`); } });
  for (const [name, path] of pages) {
    if (suffix && !["home", "block-paid", "job", "block-fallback"].includes(name)) continue;
    await page.goto(base + path, { waitUntil: "networkidle" });
    await page.evaluate(() => document.fonts.ready);
    const bad = await page.evaluate(() => {
      const fonts = new Set();
      for (const el of document.querySelectorAll("body *")) fonts.add(getComputedStyle(el).fontFamily);
      const mono = [...fonts].filter((f) => /mono|courier|consolas|menlo/i.test(f));
      const overflow = document.documentElement.scrollWidth > window.innerWidth + 1;
      return { mono, overflow };
    });
    if (bad.mono.length || bad.overflow) { problems++; console.log(`${name}${suffix}: mono ${JSON.stringify(bad.mono)} horizontal overflow ${bad.overflow}`); }
    const file = resolve(out, `${name}${suffix}.png`);
    await page.screenshot({ path: file, fullPage: true });
    console.log(file);
  }
  await ctx.close();
}
await browser.close();
if (problems) { console.log(`${problems} problem(s)`); process.exit(1); }
