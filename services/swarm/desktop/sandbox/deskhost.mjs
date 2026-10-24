// Runs INSIDE one house desktop (E2B sandbox, user `user`, DISPLAY :0). It owns the agent's
// browser: Playwright's Chromium, headful on the real X display, with the Cadre Wallet loaded
// unpacked and a persistent profile in ~/.cadre/profile. The vision model drives that window with
// the mouse and keyboard like a person; this script only does what a person would never be asked
// to type in front of a camera:
//   - first start: imports the agent's devnet key into the wallet (Import base58 key), sets the
//     wallet password, turns auto-lock off, then DELETES the key file. The key never reaches the
//     screen, a prompt or a log line.
//   - if the wallet ever shows its unlock screen, types the password (read from a 0600 file).
// It writes ~/.cadre/state.json every 5 s (open tabs, wallet address, locked or not) for the
// controller's action log, and never listens on a port.
// Live activity console: while ~/.cadre/console exists and ~/.cadre/busy does not (no task running),
// it keeps the agent's activity console (<site>/live/<handle>/console) in front, so the stream shows
// the agent's work log between tasks; when a task starts it puts the last working tab back in front.
//
// Work (2026-10-23, docs/HOUSE-AGENTS.md): every desktop is a work desktop; there is no Social mode.
//   - scripted steps: the controller writes ~/.cadre/script.json ({id, steps: [{url, wait_ms, highlight,
//     scroll}], collect, viewport}); this host runs them in the working tab, visibly, without a model,
//     and writes ~/.cadre/script-done.json: per step the final URL, HTTP status, load time, title, a text
//     excerpt, whether `highlight` was found (and marked on the page), console errors, page errors,
//     failed requests and responses >= 400, and at phone width whether the page scrolls sideways.
//   - receipts: while ~/.cadre/record exists every page the browser lands on is appended to
//     ~/.cadre/visits.jsonl with its URL, title and visible text (cut at 60 KB), so a research report's
//     sources and quotes can be checked against the pages the agent really opened.
//   - while ~/.cadre/wallet-locked exists the Cadre Wallet is locked and never unlocked: no work task
//     needs it, so a web page cannot talk the model into approving anything.
//
//   node /home/user/.cadre/deskhost.mjs <social|work> <siteUrl> <explorerUrl> [handle]
// (the first argument is the desktop's stable role id, kept for the sandbox metadata; both run the same way)
import { chromium } from 'playwright-core';
import { appendFileSync, existsSync, readFileSync, rmSync, writeFileSync, renameSync } from 'node:fs';

const [mode = 'social', site = 'https://cadre.64-95-12-116.sslip.io', explorer = `${site}/explorer`, handle = ''] = process.argv.slice(2);
const consoleUrl = handle ? `${site}/live/${handle}/console` : null;
const HOME = '/home/user/.cadre';
const DIST = `${HOME}/wallet`;
const sleep = ms => new Promise(r => setTimeout(r, ms));
const W = Number(process.env.DESK_W ?? 1280);
const H = Number(process.env.DESK_H ?? 800);
const log = (event, x = {}) => console.log(JSON.stringify({ t: new Date().toISOString(), event, ...x }));

const ctx = await chromium.launchPersistentContext(`${HOME}/profile`, {
  headless: false,
  viewport: null,
  ignoreDefaultArgs: ['--enable-automation'],
  args: [
    `--disable-extensions-except=${DIST}`,
    `--load-extension=${DIST}`,
    '--no-first-run',
    '--test-type', // hides the --no-sandbox warning bar (Playwright runs Chromium without its sandbox inside the VM)
    '--no-default-browser-check',
    '--disable-features=Translate,AutofillServerCommunication',
    '--password-store=basic',
    `--window-size=${W},${H}`,
    '--window-position=0,0',
    '--start-maximized',
  ],
});
let [sw] = ctx.serviceWorkers();
if (!sw) sw = await ctx.waitForEvent('serviceworker', { timeout: 30000 });
const extId = new URL(sw.url()).host;
const walletUrl = `chrome-extension://${extId}/index.html`;
log('browser', { mode, ext: extId });

const pw = () => readFileSync(`${HOME}/wallet-password`, 'utf8').trim();

async function seeded() {
  return sw.evaluate(async () => {
    const all = await chrome.storage.local.get(null);
    // wallet builds before 2026-10-22 kept their keys under the earlier prefix; newer ones migrate to cadre:
    return Object.keys(all).some(k => /(cadre|pearl):(imported|keyring)|encryptedSecretKey/i.test(k)) || Object.keys(all).length > 6;
  }).catch(() => false);
}

async function seed() {
  const secretFile = `${HOME}/agent-secret`;
  if (!existsSync(secretFile)) return false;
  const secret = readFileSync(secretFile, 'utf8').trim();
  const onb = await ctx.newPage();
  await onb.goto(`${walletUrl}#/get-started`);
  await onb.getByTestId('import-base58-btn').waitFor({ timeout: 20000 });
  await onb.getByTestId('import-base58-btn').click();
  await onb.getByTestId('import-base58-input').fill(secret);
  await onb.getByTestId('import-base58-submit').click();
  await onb.waitForURL('**/set-password', { timeout: 20000 });
  await onb.getByTestId('set-or-enter-password-input').fill(pw());
  await sleep(200);
  await onb.getByTestId('set-password-btn').click();
  await onb.getByTestId('home-page-container').waitFor({ timeout: 30000 });
  await onb.close();
  rmSync(secretFile, { force: true });
  await sw.evaluate(async () => {
    const all = await chrome.storage.local.get(null);
    const key = 'pearl:settings' in all && !('cadre:settings' in all) ? 'pearl:settings' : 'cadre:settings';
    await chrome.storage.local.set({ [key]: { ...(all[key] ?? {}), autoLockMinutes: 0 } });
  });
  log('wallet_seeded');
  return true;
}

// the install opens an onboarding tab; close it, then seed if this is the first start
await sleep(1500);
for (const p of ctx.pages()) if (p.url().startsWith(`chrome-extension://${extId}`)) await p.close().catch(() => {});
if (existsSync(`${HOME}/agent-secret`)) await seed().catch(e => log('seed_error', { error: String(e.message).slice(0, 200) }));
else if (!(await seeded())) log('no_wallet', { why: 'no key file and no seeded profile' });

// one working tab (tasks open their pages in the focused tab) plus the activity console; no wallet tab
// (no work task signs in the browser) and no town tab (owner 2026-10-23: no Social desktops)
const startTabs = [`${site}/market`];
if (ctx.pages().filter(p => !p.url().startsWith('about:')).length === 0) {
  const blank = ctx.pages()[0];
  for (const [i, u] of startTabs.entries()) {
    const p = i === 0 && blank ? blank : await ctx.newPage();
    await p.goto(u).catch(() => {});
  }
  await ctx.pages()[0]?.bringToFront().catch(() => {});
}

const walletLocked = () => existsSync(`${HOME}/wallet-locked`);
async function lockWallet() {
  // the wallet locks by dropping its session key (its own auto-lock does the same); the service worker
  // clears its seed caches when it sees the key go (pearl-wallet router.ts)
  await sw.evaluate(async () => { await chrome.storage.session.remove('encryptionKey'); }).catch(() => {});
}
async function unlockIfLocked() {
  if (walletLocked()) return;
  for (const p of ctx.pages()) {
    if (!p.url().startsWith(`chrome-extension://${extId}`)) continue;
    const box = p.getByTestId('password-input');
    if (await box.isVisible().catch(() => false)) {
      const btn = p.getByTestId('unlock-wallet-btn');
      if (!(await btn.isVisible().catch(() => false))) continue;
      await box.fill(pw());
      await btn.click().catch(() => {});
      log('wallet_unlocked', { page: p.url().split('#')[1] ?? '' });
    }
  }
}

const address = existsSync(`${HOME}/agent-address`) ? readFileSync(`${HOME}/agent-address`, 'utf8').trim() : null;
ctx.on('page', p => p.on('load', () => unlockIfLocked().catch(() => {})));
const isConsole = p => !!consoleUrl && p.url().startsWith(consoleUrl);
let front = null; // the console page while it is in front
let lastWork = null; // the tab the vision model worked in last
let wasBusy = false;
// ~/.cadre/front says what is in front: "console" or "work". The controller waits for "work" before a
// vision task types anything (2026-10-21: a task typed its URL while this host was switching tabs, so
// half a URL went into the console tab or a Google search).
let frontFile = '';
function writeFront(v) { if (v !== frontFile) { writeFileSync(`${HOME}/front`, v); frontFile = v; } }
async function consoleAway() {
  const back = lastWork && !lastWork.isClosed() && !isConsole(lastWork) ? lastWork : ctx.pages().find(p => !isConsole(p) && !p.url().startsWith('chrome-extension://')) ?? null;
  if (back) await back.bringToFront().catch(() => {});
  else { const p = await ctx.newPage(); await p.goto(`${site}/market`).catch(() => {}); }
  front = null;
}
async function consoleTick() {
  if (!consoleUrl) { writeFront('work'); return; }
  const want = existsSync(`${HOME}/console`);
  const busy = existsSync(`${HOME}/busy`);
  // a vision task is starting, or the console is no longer wanted (Social mode): its working tab goes back in front
  if (front && (busy || !want)) await consoleAway();
  wasBusy = busy;
  // not wanted (a one-desktop agent in Social mode): close it. It refreshes every second, and a
  // background console kept on every one-desktop agent was one of the extra tabs that held 1 GiB
  // Social desktops at 55-260 MB free on 2026-10-23 (two-desktop Social desktops, which never open it,
  // logged 6 memory warnings in 24 h against 1,301 on one-desktop agents)
  if (!want) for (const p of ctx.pages().filter(isConsole)) await p.close().catch(() => {});
  if (!want || busy) { writeFront('work'); return; }
  if (front && !front.isClosed() && isConsole(front)) return;
  lastWork = ctx.pages().filter(p => !isConsole(p) && !p.url().startsWith('chrome-extension://')).pop() ?? lastWork;
  let c = ctx.pages().find(isConsole);
  if (!c) { c = await ctx.newPage(); await c.goto(consoleUrl).catch(() => {}); }
  await c.bringToFront().catch(() => {});
  front = c;
  writeFront('console');
}
// memory (MB) from /proc/meminfo; null when unreadable
function memMb() {
  try {
    const m = readFileSync('/proc/meminfo', 'utf8');
    const k = key => Number((new RegExp(`^${key}:\\s+(\\d+)`, 'm').exec(m) ?? [])[1]) / 1024;
    return { total: k('MemTotal'), avail: k('MemAvailable') };
  } catch { return null; }
}
// tabs besides the console: 2 on a 1 GiB desktop (976 MB total), 3 on larger ones
const MEM0 = memMb();
const MAX_TABS = Number(process.env.DESK_MAX_TABS ?? (MEM0 && MEM0.total < 1200 ? 2 : 3));
// below this many MB available (or when the controller writes ~/.cadre/trim) every tab but the working
// one is closed, while no vision task runs
const TRIM_MB = Number(process.env.DESK_TRIM_MB ?? 120);
let lastTrimAt = 0;
async function trimTabs(why) {
  const before = memMb();
  const keep = lastWork && !lastWork.isClosed() && !isConsole(lastWork) ? lastWork : ctx.pages().find(p => !isConsole(p)) ?? null;
  let closed = 0;
  for (const p of ctx.pages()) {
    if (p === keep || (isConsole(p) && p === front)) continue;
    await p.close().catch(() => {});
    closed++;
  }
  if (!ctx.pages().length) { const p = await ctx.newPage(); await p.goto(`${site}/market`).catch(() => {}); }
  await sleep(1500);
  const after = memMb();
  log('tabs_trimmed', { why, closed, avail_before_mb: before ? Math.round(before.avail) : null, avail_after_mb: after ? Math.round(after.avail) : null });
}
// ---- receipts: every page the browser lands on while ~/.cadre/record exists
const VISIT_MAX = 60_000;
async function recordVisit(p) {
  if (!existsSync(`${HOME}/record`) || isConsole(p)) return;
  const url = p.url();
  if (!/^https?:/.test(url)) return;
  await sleep(1200); // late text (client-rendered pages)
  const text = await p.evaluate(() => document.body ? document.body.innerText : '').catch(() => '');
  const title = await p.title().catch(() => '');
  appendFileSync(`${HOME}/visits.jsonl`, JSON.stringify({ t: Date.now(), url: p.url(), title: title.slice(0, 200), text: String(text).slice(0, VISIT_MAX) }) + '\n');
}
const watched = new WeakSet();
function watch(p) {
  if (watched.has(p)) return;
  watched.add(p);
  p.on('load', () => { recordVisit(p).catch(() => {}); });
}
for (const p of ctx.pages()) watch(p);
ctx.on('page', watch);

// ---- scripted work steps (no model): the controller writes script.json, this host writes script-done.json
async function runScript(sc) {
  const out = [];
  let page = lastWork && !lastWork.isClosed() && !isConsole(lastWork) ? lastWork : ctx.pages().find(p => !isConsole(p) && !p.url().startsWith('chrome-extension://')) ?? null;
  if (!page) page = await ctx.newPage();
  await page.bringToFront().catch(() => {});
  lastWork = page;
  const vp = sc.viewport && Number(sc.viewport.width) > 0 ? { width: Math.round(sc.viewport.width), height: Math.round(sc.viewport.height || 844) } : null;
  if (vp) await page.setViewportSize(vp).catch(() => {});
  for (const st of (sc.steps ?? []).slice(0, 40)) {
    const r = { url: String(st.url ?? ''), final_url: null, status: null, ms: null, title: '', excerpt: '', text_len: 0, found: null, console_errors: [], page_errors: [], failed_requests: [], bad_responses: [], overflow_px: null, error: null };
    const onConsole = m => { if (m.type() === 'error' && r.console_errors.length < 10) r.console_errors.push(m.text().slice(0, 300)); };
    const onPageError = e => { if (r.page_errors.length < 10) r.page_errors.push(String(e?.message ?? e).slice(0, 300)); };
    const onFailed = q => { if (r.failed_requests.length < 10) r.failed_requests.push({ url: q.url().slice(0, 300), error: (q.failure()?.errorText ?? '').slice(0, 120) }); };
    const onResp = x => { if (x.status() >= 400 && r.bad_responses.length < 10) r.bad_responses.push({ url: x.url().slice(0, 300), status: x.status() }); };
    if (sc.collect) { page.on('console', onConsole); page.on('pageerror', onPageError); page.on('requestfailed', onFailed); page.on('response', onResp); }
    const t0 = Date.now();
    try {
      if (r.url) {
        const resp = await page.goto(r.url, { waitUntil: 'domcontentloaded', timeout: 30000 });
        r.status = resp ? resp.status() : null;
      }
      r.ms = Date.now() - t0;
      await sleep(Math.max(500, Math.min(15000, Number(st.wait_ms ?? 2500))));
      r.final_url = page.url().slice(0, 500);
      r.title = (await page.title().catch(() => '')).slice(0, 200);
      const text = String(await page.evaluate(() => document.body ? document.body.innerText : '').catch(() => ''));
      r.text_len = text.length;
      r.excerpt = text.replace(/\s+/g, ' ').trim().slice(0, 600);
      if (vp) r.overflow_px = await page.evaluate(() => Math.max(0, document.documentElement.scrollWidth - window.innerWidth)).catch(() => null);
      if (typeof st.highlight === 'string' && st.highlight.trim()) {
        // mark the first occurrence of the quote on the page and scroll to it (what a person checking a source does)
        r.found = await page.evaluate((q) => {
          const norm = s => s.toLowerCase().replace(/[\u2018\u2019]/g, "'").replace(/[\u201c\u201d]/g, '"').replace(/\s+/g, ' ');
          const want = norm(q).slice(0, 80);
          const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
          for (let n = w.nextNode(); n; n = w.nextNode()) {
            const i = norm(n.textContent || '').indexOf(want.slice(0, 40));
            if (i < 0) continue;
            const el = n.parentElement;
            if (!el) continue;
            el.style.outline = '3px solid #e8a400';
            el.style.background = 'rgba(255, 214, 0, 0.35)';
            el.scrollIntoView({ block: 'center' });
            return true;
          }
          return false;
        }, st.highlight).catch(() => false);
        await sleep(1500);
      }
      if (Number(st.scroll) > 0) for (let i = 0; i < Math.min(6, Number(st.scroll)); i++) { await page.mouse.wheel(0, 500).catch(() => {}); await sleep(700); }
      await recordVisit(page).catch(() => {});
    } catch (e) {
      r.error = String(e?.message ?? e).split('\n')[0].slice(0, 200);
      r.ms = Date.now() - t0;
    }
    if (sc.collect) { page.off('console', onConsole); page.off('pageerror', onPageError); page.off('requestfailed', onFailed); page.off('response', onResp); }
    out.push(r);
  }
  if (vp) await page.setViewportSize(null).catch(async () => { await page.setViewportSize({ width: W, height: H - 120 }).catch(() => {}); });
  return out;
}
let scriptRunning = false;
async function scriptTick() {
  if (scriptRunning || !existsSync(`${HOME}/script.json`)) return;
  scriptRunning = true;
  let sc = null;
  try { sc = JSON.parse(readFileSync(`${HOME}/script.json`, 'utf8')); } catch { sc = null; }
  rmSync(`${HOME}/script.json`, { force: true });
  try {
    if (sc && sc.id) {
      const results = await runScript(sc);
      writeFileSync(`${HOME}/script-done.json.tmp`, JSON.stringify({ id: sc.id, results }));
      renameSync(`${HOME}/script-done.json.tmp`, `${HOME}/script-done.json`);
      log('script_done', { id: String(sc.id).slice(0, 40), steps: results.length });
    }
  } catch (e) {
    log('script_error', { error: String(e?.message ?? e).slice(0, 160) });
  } finally { scriptRunning = false; }
}

for (let n = 0; ; n++) {
  void scriptTick();
  if (walletLocked() && n % 5 === 0) await lockWallet();
  await consoleTick().catch(e => log('console_error', { error: String(e.message).slice(0, 120) }));
  if (n % 5 === 0) {
    await unlockIfLocked().catch(() => {});
    const busy = existsSync(`${HOME}/busy`);
    const mem = memMb();
    const asked = existsSync(`${HOME}/trim`);
    if (!busy && (asked || (mem && mem.avail < TRIM_MB && Date.now() - lastTrimAt > 10 * 60_000))) {
      rmSync(`${HOME}/trim`, { force: true });
      lastTrimAt = Date.now();
      await trimTabs(asked ? 'controller' : `memory ${Math.round(mem.avail)} MB`).catch(e => log('trim_error', { error: String(e.message).slice(0, 120) }));
    }
    // keep the tab count small: at most MAX_TABS tabs plus the console; close the oldest first, never
    // the focused one the vision model works in; one wallet tab at most (a wallet task opens the wallet
    // in the working tab, and those tabs used to be exempt from closing, so they piled up)
    const pages = ctx.pages().filter(p => !isConsole(p));
    // never while a task runs (a wallet approval window is an extension page the task needs)
    let extra = busy ? 0 : pages.length - MAX_TABS;
    const walletTabs = pages.filter(p => p.url().startsWith('chrome-extension://'));
    for (const p of pages) {
      if (extra <= 0) break;
      if (p === lastWork) continue;
      if (p.url().startsWith('chrome-extension://') && p === walletTabs[walletTabs.length - 1]) continue;
      await p.close().catch(() => {});
      extra--;
    }
    const tabs = [];
    for (const p of ctx.pages()) tabs.push({ url: p.url().slice(0, 300), title: (await p.title().catch(() => '')).slice(0, 120) });
    const st = { t: Date.now(), mode, ext: extId, address, tabs, console: !!front };
    writeFileSync(`${HOME}/state.json.tmp`, JSON.stringify(st));
    renameSync(`${HOME}/state.json.tmp`, `${HOME}/state.json`);
  }
  await sleep(1000);
}
