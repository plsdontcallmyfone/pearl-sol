// Headless screenshots of the contract pages against the test fixture (test/contract-fixture.ts):
// a fake node with the contract RPC + an empty Solana RPC, the explorer on 10691 (block 10690-10694),
// Playwright's bundled headless Chromium from extension/node_modules (never a real browser).
//   node tools/contract-screens.mjs [outDir=../../screenshots/explorer]
// Fails on console errors, any monospace computed font, horizontal overflow or em dashes.
import { createRequire } from "node:module";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(resolve(here, "../../../extension/package.json"));
const { chromium } = require("playwright");
const out = resolve(process.argv[2] ?? resolve(here, "../../../screenshots/explorer"));
mkdirSync(out, { recursive: true });

const PORT = 10691, NODE = 10692, SOL = 10693;
const { fakeNode, fakeSolana, COUNTER, CALL } = await import("../test/contract-fixture.ts");
const { Indexer } = await import("../src/indexer.ts");
const { serve } = await import("../src/server.ts");
const node = fakeNode(), sol = fakeSolana();
await Promise.all([[node, NODE], [sol, SOL]].map(([s, p]) => new Promise((r, j) => s.once("error", j).listen(p, "127.0.0.1", r))));
const dir = mkdtempSync(join(tmpdir(), "explorer-contract-shots-"));
const ix = new Indexer({ nodeUrl: `http://127.0.0.1:${NODE}`, solUrl: `http://127.0.0.1:${SOL}`, stateDir: dir, log: () => {} });
await ix.poll();
const srv = serve(ix, PORT, "127.0.0.1");
const base = `http://127.0.0.1:${PORT}`;
const pages = [["contract", `/contract/${COUNTER}`], ["contracts", "/contracts"], ["contract-tx", `/tx/${CALL.tx_hash}`], ["block-contracts", "/block/24"]];
const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH, headless: false, args: ["--headless=new"] } : { headless: true });
let problems = 0;
for (const [scheme, vp, suffix] of [["light", { width: 1366, height: 900 }, ""], ["dark", { width: 1366, height: 900 }, "-dark"], ["light", { width: 390, height: 844 }, "-phone"]]) {
  const ctx = await browser.newContext({ viewport: vp, colorScheme: scheme, deviceScaleFactor: vp.width < 500 ? 2 : 1 });
  const page = await ctx.newPage();
  page.on("response", (r) => { if (r.status() >= 400) console.log(`HTTP ${r.status()} ${r.url()}`); });
  page.on("console", (m) => { if (m.type() === "error" && !/favicon\.ico$/.test(m.location()?.url ?? "")) { problems++; console.log(`console error on ${page.url()}: ${m.text()}`); } });
  for (const [name, path] of pages) {
    if (suffix && name !== "contract") continue;
    await page.goto(base + path, { waitUntil: "networkidle" });
    await page.evaluate(() => document.fonts.ready);
    const bad = await page.evaluate(() => {
      const fonts = new Set();
      for (const el of document.querySelectorAll("body *")) fonts.add(getComputedStyle(el).fontFamily);
      return { mono: [...fonts].filter((f) => /mono|courier|consolas|menlo/i.test(f)), overflow: document.documentElement.scrollWidth > window.innerWidth + 1, dash: document.body.innerText.includes("\u2014") };
    });
    if (bad.mono.length || bad.overflow || bad.dash) { problems++; console.log(`${name}${suffix}: ${JSON.stringify(bad)}`); }
    const file = resolve(out, `${name}${suffix}.png`);
    await page.screenshot({ path: file, fullPage: true });
    console.log(file);
  }
  await ctx.close();
}
await browser.close();
for (const s of [srv, node, sol]) s?.close?.();
ix.stop();
rmSync(dir, { recursive: true, force: true });
console.log(problems ? `${problems} problem(s)` : "ok: no console errors, no monospace, no overflow, no em dashes");
process.exit(problems ? 1 : 0);
