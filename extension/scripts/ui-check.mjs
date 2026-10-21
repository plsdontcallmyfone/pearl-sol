// Headless UI check: loads dist/ into Playwright's own Chromium (--headless=new,
// throwaway profile; never the owner's Chrome), drives onboarding, every tab and
// both dApp providers, and writes screenshots to extension/screenshots/.
// RPC: the real localnet if :10600 and :10630 both answer (live smoke, read only),
// else scripts/mock-rpc.mjs on 10671/10672 (+ test page on 10673).
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { ed25519 } from "@noble/curves/ed25519";
import { base58, base64 } from "@scure/base";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "dist");
const shots = resolve(root, "screenshots");
const profile = resolve(root, ".ui-profile");
mkdirSync(shots, { recursive: true });
rmSync(profile, { recursive: true, force: true });

const seed = new Uint8Array(32).map((_, i) => (i * 37 + 11) & 0xff);
const pub = ed25519.getPublicKey(seed);
const ADDRESS = base58.encode(pub);
const SECRET = base58.encode(new Uint8Array([...seed, ...pub]));
const PASSWORD = "localnet-pass-1";

const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
};

async function rpcUp(url, body) {
  try {
    const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body), signal: AbortSignal.timeout(1500) });
    return r.ok;
  } catch {
    return false;
  }
}

const live = (await rpcUp("http://127.0.0.1:10600", { jsonrpc: "2.0", id: 1, method: "getHealth" })) && (await rpcUp("http://127.0.0.1:10630", { jsonrpc: "2.0", id: 1, method: "getChainInfo", params: {} }));
console.log(live ? "live localnet found on 10600 + 10630: read-only smoke" : "no live localnet on 10600 + 10630: using mock RPC on 10671 / 10672");

// The mock also serves the test dApp page (10673), so it runs in live mode too
// (its RPC ports are then unused). Live mode never writes to the real chains.
let mock;
{
  mock = spawn(process.execPath, [resolve(root, "scripts/mock-rpc.mjs")], { env: { ...process.env, MOCK_ACCOUNT: ADDRESS }, stdio: ["ignore", "pipe", "inherit"] });
  await new Promise((r) => mock.stdout.on("data", (d) => String(d).includes("dapp page") && r()));
}
const SOL = live ? "http://127.0.0.1:10600" : "http://127.0.0.1:10671";
const CHAIN = live ? "http://127.0.0.1:10630" : "http://127.0.0.1:10672";

const ctx = await chromium.launchPersistentContext(profile, {
  headless: false,
  args: ["--headless=new", `--disable-extensions-except=${dist}`, `--load-extension=${dist}`, "--no-first-run"],
  viewport: { width: 360, height: 600 },
});
const consoleErrors = [];
try {
  let [sw] = ctx.serviceWorkers();
  if (!sw) sw = await ctx.waitForEvent("serviceworker");
  const extId = new URL(sw.url()).host;
  const popupUrl = `chrome-extension://${extId}/popup.html`;
  const page = await ctx.newPage();
  page.on("console", (m) => m.type() === "error" && consoleErrors.push(`popup: ${m.text()}`));
  page.on("pageerror", (e) => consoleErrors.push(`popup: ${e.message}`));
  page.on("dialog", (d) => d.accept());
  const shot = async (name, p = page) => {
    await p.waitForSelector(".toast", { state: "detached", timeout: 4000 }).catch(() => {});
    await p.waitForTimeout(350);
    await p.screenshot({ path: resolve(shots, `${name}.png`) });
    if (p === page) await clearOfTabbar(name);
  };
  // Scroll the active screen to the bottom and make sure its last row ends above the tab
  // bar (the tab bar must never cover content, on any route).
  const clearOfTabbar = async (route) => {
    const r = await page.evaluate(async () => {
      const bar = document.querySelector(".tabbar");
      const sc = [...document.querySelectorAll(".scroll")].pop();
      if (!bar || !sc) return { skip: true };
      const before = sc.scrollTop;
      sc.scrollTop = sc.scrollHeight;
      await new Promise((res) => setTimeout(res, 120));
      const kids = [...sc.querySelectorAll(":scope > *, :scope > * > *")].filter((e) => e.getBoundingClientRect().height > 0);
      const last = Math.max(...kids.map((e) => e.getBoundingClientRect().bottom));
      const top = bar.getBoundingClientRect().top;
      const vh = window.innerHeight;
      sc.scrollTop = before;
      return { last: Math.round(last), top: Math.round(top), barBottom: Math.round(bar.getBoundingClientRect().bottom), vh };
    });
    if (r.skip) return;
    check(`${route}: last row clear of the tab bar`, r.last <= r.top && r.barBottom <= r.vh, `last row ends ${r.last}, tab bar ${r.top} to ${r.barBottom}, viewport ${r.vh}`);
  };
  const tab = async (t) => {
    await page.click(`[data-testid=tab-${t}]`);
    await page.waitForTimeout(500);
  };

  // ---------------- onboarding
  await page.goto(popupUrl);
  await page.waitForSelector("[data-testid=import-secret]");
  await shot("01-welcome");
  await page.click("[data-testid=import-secret]");
  await page.fill("[data-testid=secret-input]", SECRET);
  await shot("02-import-key");
  await page.click("[data-testid=secret-next]");
  await page.fill("[data-testid=pw1]", PASSWORD);
  await page.fill("[data-testid=pw2]", PASSWORD);
  await page.click("[data-testid=create-submit]");
  await page.waitForSelector("[data-testid=acct-btn]");
  check("import base58 key -> address", (await page.textContent("[data-testid=acct-btn]")).includes(ADDRESS.slice(0, 4)), ADDRESS);

  // ---------------- point the Localnet preset at the RPCs used for this run
  if (!live) {
    await tab("settings");
    await page.locator(".list-row", { hasText: "Localnet" }).locator("button", { hasText: "Edit" }).click();
    await page.fill("[data-testid=net-sol]", SOL);
    await page.fill("[data-testid=net-chain]", CHAIN);
    await shot("13-network-edit");
    await page.click("[data-testid=net-save]");
    await page.waitForTimeout(300);
  }

  // ---------------- wallet
  await tab("wallet");
  await page.waitForFunction(() => !document.querySelector("[data-testid=balances] .skel"), null, { timeout: 15000 });
  await page.waitForTimeout(800);
  const balText = await page.textContent("[data-testid=balances]");
  check("balances render for both chains", /SOL/.test(balText) && /height/.test(balText), balText.replace(/\s+/g, " ").slice(0, 120));
  await shot("03-home-light");
  await page.emulateMedia({ colorScheme: "dark" });
  await shot("04-home-dark");
  await page.emulateMedia({ colorScheme: "light" });

  if (live) check("live smoke: both RPCs reachable", !/offline|cannot reach/.test(await page.textContent(".scroll")));
  if (!live) {
    await page.click("[data-testid=act-send]");
    await page.fill("[data-testid=send-to]", base58.encode(new Uint8Array(32).fill(9)));
    await page.fill("[data-testid=send-amount]", "12.5");
    await shot("05-send");
    await page.click("[data-testid=send-submit]");
    await page.waitForSelector(".toast", { timeout: 10000 }).catch(() => {});
    check("native send accepted by node", (await page.textContent(".toast").catch(() => "")).startsWith("Sent"));
  }
  await page.click("[data-testid=act-receive]");
  await shot("06-receive");
  await page.click("[aria-label=Back]");

  // ---------------- bridge
  await tab("bridge");
  if (!live) {
  await page.fill("[data-testid=bridge-amount]", "250");
  await shot("07-bridge-in");
  await page.click("[data-testid=bridge-submit]");
  await page.waitForSelector("[data-testid=pending-list]", { timeout: 10000 });
  await page.click(".seg button:has-text('Agent chain to Solana')");
  await page.fill("[data-testid=bridge-amount]", "100.5");
  await page.click("[data-testid=bridge-submit]");
  await page.waitForTimeout(6500);
  const pendText = await page.textContent("[data-testid=pending-list]");
  check("bridge in + out tracked as pending", /Solana to agent chain/.test(pendText) && /Agent chain to Solana/.test(pendText), pendText.replace(/\s+/g, " ").slice(0, 160));
  check("bridge out shows K-deep progress", /Finalizing \d+ of 30|Waiting for a block|Final/.test(pendText));
  await page.click(".seg button:has-text('Agent chain to Solana')");
  await page.fill("[data-testid=bridge-amount]", "");
  await shot("08-bridge-pending");
  }

  // ---------------- jobs
  await tab("jobs");
  await page.waitForSelector(live ? "[data-testid=agent-card]" : "[data-testid=job-row]", { timeout: 10000 });
  await shot("09-jobs-market");
  if (!live) {
    await page.click("[data-testid=agent-open]");
    await page.fill("[data-testid=agent-name]", "lab-agent-7");
    await shot("09b-register-agent");
    await page.click("[data-testid=agent-submit]");
    await page.waitForTimeout(2500);
    check("RegisterAgent shows registered", (await page.textContent("[data-testid=agent-card]")).includes("lab-agent-7"));
  }
  if (await page.$("[data-testid=job-row]")) {
    await page.locator("[data-testid=job-row]").first().click();
    await page.waitForSelector("[data-testid=header-check]", { timeout: 8000 }).catch(() => {});
    const hc = await page.textContent("[data-testid=header-check]").catch(() => "");
    check("job detail recomputes job_key from header_id", hc === "job_key verified", hc);
    const du = (await page.textContent("[data-testid=job-data-uri]").catch(() => "")) ?? "";
    check("job detail shows its data URI (blob: shortened)", /^(blob \S+ \(chain blob store\)|\w+:\/\/.+)$/.test(du), du);
    await shot("10-job-detail");
    await page.click("[aria-label=Back]");
  }
  await page.click(".seg button:has-text('Post')");
  await page.setInputFiles("[data-testid=file-a]", resolve(root, "test/fixtures/job/A.i8"));
  await page.setInputFiles("[data-testid=file-b]", resolve(root, "test/fixtures/job/B.i8"));
  await page.fill("[data-testid=job-w]", "5");
  await page.fill("[data-testid=job-deadline]", "2000");
  await page.fill("[data-testid=job-evals]", base58.encode(new Uint8Array(32).fill(5)));
  await page.click("button:has-text('Preview')");
  await page.waitForSelector("[data-testid=job-preview]", { timeout: 8000 }).catch(() => {});
  check("PostJob preview computes commitments", !!(await page.$("[data-testid=job-preview]")));
  await page.locator("[data-testid=job-preview]").scrollIntoViewIfNeeded().catch(() => {});
  await shot("11-post-job");
  await page.click(".seg button:has-text('Buy')");
  await page.fill("[data-testid=store-sol]", "0.5");
  await page.fill("[data-testid=store-m]", "8");
  await page.fill("[data-testid=store-n]", "16");
  await page.setInputFiles("[data-testid=store-file-a]", resolve(root, "test/fixtures/job/A.i8"));
  await page.setInputFiles("[data-testid=store-file-b]", resolve(root, "test/fixtures/job/B.i8"));
  await page.fill("[data-testid=store-evals]", base58.encode(new Uint8Array(32).fill(5)));
  await page.click("[data-testid=store-preview]");
  await page.waitForSelector("[data-testid=order-preview]", { timeout: 8000 }).catch(() => {});
  const op = (await page.textContent("[data-testid=order-preview]").catch(() => "")) ?? "";
  check("Buy form computes the order commitments from the matrices", /hash_a/.test(op) && /order_terms/.test(op), op.replace(/\s+/g, " ").slice(0, 120));
  await page.waitForTimeout(800);
  await shot("12-storefront");
  await page.click(".seg button:has-text('Mining')");
  await page.waitForSelector("[data-testid=rewards] .hero-amount:not(:has(.skel))", { timeout: 20000 }).catch(() => {});
  await page.waitForTimeout(1500);
  check("mining rewards scan", /blocks/.test(await page.textContent("[data-testid=rewards]")));
  if (!live) {
    const split = (await page.locator("[data-testid=block-split]").first().textContent().catch(() => "")) ?? "";
    check("mining view shows getBlock subsidy + fees per block", /^subsidy .+ \+ fees /.test(split), split);
  }
  await shot("14-mining");

  // ---------------- activity + settings
  await tab("activity");
  await page.waitForSelector("[data-testid=activity-list]", { timeout: 10000 }).catch(() => {});
  await shot("15-activity");

  // ---------------- dApp providers
  const dapp = await ctx.newPage();
  dapp.on("pageerror", (e) => consoleErrors.push(`dapp: ${e.message}`));
  const dappUrl = live ? "http://127.0.0.1:10673" : "http://127.0.0.1:10673";
  await dapp.goto(dappUrl);
  await dapp.waitForFunction(() => window.__wallets?.length > 0 && !!window.pearl, null, { timeout: 5000 });
  check("Wallet Standard wallet registered", await dapp.evaluate(() => window.__wallets[0].name === "Pearl Wallet" && window.__wallets[0].chains.includes("solana:localnet")));

  const approve = async (name, fn, arg) => {
    const pending = ctx.waitForEvent("page", { timeout: 10000 });
    const result = dapp.evaluate(fn, arg);
    const ap = await pending;
    ap.on("pageerror", (e) => consoleErrors.push(`approval: ${e.message}`));
    await ap.setViewportSize({ width: 376, height: 640 });
    await ap.waitForSelector("[data-testid=approve]");
    await ap.waitForTimeout(400);
    await ap.screenshot({ path: resolve(shots, `${name}.png`) });
    const origin = await ap.textContent("[data-testid=approval-origin]");
    const headline = await ap.textContent("[data-testid=approval-headline]").catch(() => null);
    await ap.click("[data-testid=approve]").catch(() => {});
    return { result: await result, origin, headline };
  };

  const c1 = await approve("16-approve-connect", async () => {
    const w = window.__wallets[0];
    const r = await w.features["standard:connect"].connect();
    return r.accounts.map((a) => a.address);
  });
  check("connect approval shows origin", c1.origin.includes("127.0.0.1:10673"), c1.origin);
  check("Solana connect returns the account", c1.result[0] === ADDRESS);

  const m1 = await approve("17-approve-solana-message", async () => {
    const w = window.__wallets[0];
    const [r] = await w.features["solana:signMessage"].signMessage({ account: w.accounts[0], message: new TextEncoder().encode("Sign in to the test dApp") });
    return Array.from(r.signature);
  });
  check("solana:signMessage signature verifies", ed25519.verify(Uint8Array.from(m1.result), new TextEncoder().encode("Sign in to the test dApp"), pub));

  // Solana signAndSendTransaction: a legacy SOL transfer built with web3.js
  const { Transaction, SystemProgram, PublicKey } = await import("@solana/web3.js");
  const solTx = new Transaction({ feePayer: new PublicKey(pub), recentBlockhash: base58.encode(new Uint8Array(32).fill(4)) })
    .add(SystemProgram.transfer({ fromPubkey: new PublicKey(pub), toPubkey: new PublicKey(new Uint8Array(32).fill(8)), lamports: 250_000_000 }));
  const solTxB64 = base64.encode(solTx.serialize({ requireAllSignatures: false, verifySignatures: false }));
  const s1 = await approve("17b-approve-solana-tx", async ([b64tx, send]) => {
    const w = window.__wallets[0];
    const bytes = Uint8Array.from(atob(b64tx), (c) => c.charCodeAt(0));
    if (!send) {
      const [r] = await w.features["solana:signTransaction"].signTransaction({ account: w.accounts[0], transaction: bytes, chain: "solana:localnet" });
      return Array.from(r.signedTransaction.slice(1, 65));
    }
    const [r] = await w.features["solana:signAndSendTransaction"].signAndSendTransaction({ account: w.accounts[0], transaction: bytes, chain: "solana:localnet" });
    return Array.from(r.signature);
  }, [solTxB64, !live]);
  const solSigned = Transaction.from(Buffer.from(solTxB64, "base64"));
  check(`solana:${live ? "signTransaction" : "signAndSendTransaction"} signature verifies`, ed25519.verify(Uint8Array.from(s1.result), new Uint8Array(solSigned.serializeMessage()), pub));
  check("Solana approval decodes the transfer", s1.headline === "Send 0.25 SOL", s1.headline);

  const c2 = await approve("18-approve-connect-pearl", async () => (await window.pearl.connect()).address);
  check("window.pearl connect", c2.result === ADDRESS);

  const t1 = await approve("19-approve-pearl-transfer", async (send) => {
    const input = { transaction: { kind: "Transfer", to: "BAWZgY5CUMzXJmnGEKzKBPuwq8EeuBPetJ3FtQKgN9km", amount: "2500000000" } };
    const r = send ? await window.pearl.signAndSendTransaction(input) : await window.pearl.signTransaction(input);
    return r.txHash;
  }, !live);
  check("window.pearl signAndSendTransaction returns a tx hash", /^[0-9a-f]{64}$/.test(t1.result), t1.result);

  // contract Call through window.pearl: method decoded from the ABI, label from getContract, site warning for value
  const cc = await approve("19b-approve-contract-call", async () => {
    const args = window.pearl.contracts.encodeCallArgs("increment", [{ u64: 5 }]);
    const r = await window.pearl.signTransaction({ transaction: { kind: "Call", contract: "Ad49vfymLfb1oY4veT8yudCHGL6DM6aUpTMZuPbM7wcZ", value: "1000000000", gasLimit: "1000000", args } });
    return r.txHash;
  });
  check("contract Call approval decodes method, label, value and gas", cc.headline === "Call increment on contract counter (Ad49...wcZ) with 5 (u64), sending 1 PEARL, gas up to 1,000,000", cc.headline);
  check("contract Call returns a tx hash", /^[0-9a-f]{64}$/.test(cc.result), cc.result);
  const sa = await approve("19c-approve-contract-immutable", async () => (await window.pearl.signTransaction({ transaction: { kind: "SetContractAdmin", contract: "Ad49vfymLfb1oY4veT8yudCHGL6DM6aUpTMZuPbM7wcZ", admin: null } })).txHash);
  check("SetContractAdmin approval says immutable", sa.headline === "Make contract counter (Ad49...wcZ) immutable", sa.headline);

  const pm = await approve("20-approve-pearl-message", async () => Array.from((await window.pearl.signMessage("hello agent chain")).signature));
  const framed = new Uint8Array([20, ...new TextEncoder().encode("pearl-sol:message:v1"), ...new TextEncoder().encode("hello agent chain")]);
  check("window.pearl.signMessage uses Commonware framing", ed25519.verify(Uint8Array.from(pm.result), framed, pub));

  // reject path
  const pending = ctx.waitForEvent("page");
  const rej = dapp.evaluate(() => window.pearl.signMessage("reject me").then(() => "signed", (e) => `rejected ${e.code}`));
  const ap = await pending;
  await ap.waitForSelector("[data-testid=reject]");
  await ap.click("[data-testid=reject]").catch(() => {});
  check("reject returns 4001", (await rej) === "rejected 4001");

  // permissions + revoke
  await page.bringToFront();
  await tab("settings");
  await page.click("[data-testid=sites-row]");
  await page.waitForSelector("[data-testid=sites-list]");
  await shot("21-connected-sites");
  await page.click("[data-testid=revoke]");
  await page.waitForTimeout(500);
  const afterRevoke = await dapp.evaluate(() => window.pearl.getAccount().then(() => "ok", (e) => `err ${e.code}`));
  check("revoke removes access", afterRevoke === "err 4100", afterRevoke);
  await page.click("[aria-label=Back]");
  await shot("22-settings");
  await page.emulateMedia({ colorScheme: "dark" });
  await tab("jobs");
  await page.click(".seg button:has-text('Market')");
  await page.waitForSelector("[data-testid=job-row]");
  await shot("23-jobs-dark");
  await tab("bridge");
  await shot("24-bridge-dark");
  await page.emulateMedia({ colorScheme: "light" });

  // lock / unlock
  await page.click("[data-testid=lock]");
  await page.waitForSelector("[data-testid=unlock-pw]");
  await shot("25-locked");
  await page.fill("[data-testid=unlock-pw]", "wrong-password");
  await page.click("[data-testid=unlock-submit]");
  await page.waitForSelector(".error-text");
  check("wrong password refused", (await page.textContent(".error-text")).includes("not right"));
  await page.fill("[data-testid=unlock-pw]", PASSWORD);
  await page.click("[data-testid=unlock-submit]");
  await page.waitForSelector("[data-testid=acct-btn]");
  check("unlock works", true);

  // no monospace anywhere
  const mono = await page.evaluate(() => [...document.querySelectorAll("*")].filter((e) => /mono/i.test(getComputedStyle(e).fontFamily)).length);
  check("no monospace fonts in the popup", mono === 0, `${mono} elements`);
  const font = await page.evaluate(() => getComputedStyle(document.body).fontFamily);
  check("Geist is the body font", font.startsWith('"Geist"') || font.startsWith("Geist"), font);
  const fontLoaded = await page.evaluate(async () => { await document.fonts.ready; return document.fonts.check('13px "Geist"'); });
  check("Geist font file loaded", fontLoaded);
} finally {
  check("no console errors", consoleErrors.length === 0, consoleErrors.join(" | ").slice(0, 400));
  await ctx.close();
  mock?.kill();
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} UI checks passed; screenshots in ${shots}`);
  process.exitCode = failed ? 1 : 0;
}
