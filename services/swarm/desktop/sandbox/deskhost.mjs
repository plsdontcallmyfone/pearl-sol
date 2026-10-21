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
// Live activity console: while ~/.cadre/console exists (the controller sets it on Work desktops and on
// one-desktop agents in Work mode) and ~/.cadre/busy does not (no vision task running), it keeps the
// agent's activity console (<site>/live/<handle>/console) in front, so the stream shows the agent's
// runtime as it acts; when a vision task starts it puts the last working tab back in front first.
//
//   node /home/user/.cadre/deskhost.mjs <social|work> <siteUrl> <explorerUrl> [handle]
import { chromium } from 'playwright-core';
import { existsSync, readFileSync, rmSync, writeFileSync, renameSync } from 'node:fs';

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
    return Object.keys(all).some(k => /pearl:imported|encryptedSecretKey|pearl:keyring/i.test(k)) || Object.keys(all).length > 6;
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
    const s = (await chrome.storage.local.get('pearl:settings'))['pearl:settings'] ?? {};
    await chrome.storage.local.set({ 'pearl:settings': { ...s, autoLockMinutes: 0 } });
  });
  log('wallet_seeded');
  return true;
}

// the install opens an onboarding tab; close it, then seed if this is the first start
await sleep(1500);
for (const p of ctx.pages()) if (p.url().startsWith(`chrome-extension://${extId}`)) await p.close().catch(() => {});
if (existsSync(`${HOME}/agent-secret`)) await seed().catch(e => log('seed_error', { error: String(e.message).slice(0, 200) }));
else if (!(await seeded())) log('no_wallet', { why: 'no key file and no seeded profile' });

// Work: the wallet and one working tab (tasks open their pages in the focused tab), plus the activity
// console; a third tab (the market) pushed 1 GiB desktops into the OOM killer on 2026-10-21
const startTabs = mode === 'work'
  ? [walletUrl, explorer]
  : [`${site}/town`];
if (ctx.pages().filter(p => !p.url().startsWith('about:')).length === 0) {
  const blank = ctx.pages()[0];
  for (const [i, u] of startTabs.entries()) {
    const p = i === 0 && blank ? blank : await ctx.newPage();
    await p.goto(u).catch(() => {});
  }
  await ctx.pages()[0]?.bringToFront().catch(() => {});
}

async function unlockIfLocked() {
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
  else { const p = await ctx.newPage(); await p.goto(mode === 'work' ? explorer : `${site}/town`).catch(() => {}); }
  front = null;
}
async function consoleTick() {
  if (!consoleUrl) { writeFront('work'); return; }
  const want = existsSync(`${HOME}/console`);
  const busy = existsSync(`${HOME}/busy`);
  // a vision task is starting, or the console is no longer wanted (Social mode): its working tab goes back in front
  if (front && (busy || !want)) await consoleAway();
  wasBusy = busy;
  if (!want || busy) { writeFront('work'); return; }
  if (front && !front.isClosed() && isConsole(front)) return;
  lastWork = ctx.pages().filter(p => !isConsole(p) && !p.url().startsWith('chrome-extension://')).pop() ?? lastWork;
  let c = ctx.pages().find(isConsole);
  if (!c) { c = await ctx.newPage(); await c.goto(consoleUrl).catch(() => {}); }
  await c.bringToFront().catch(() => {});
  front = c;
  writeFront('console');
}
for (let n = 0; ; n++) {
  await consoleTick().catch(e => log('console_error', { error: String(e.message).slice(0, 120) }));
  if (n % 5 === 0) {
    await unlockIfLocked().catch(() => {});
    // keep the tab count small on a 1 GiB desktop: at most 3 tabs plus the console; close the oldest
    // non-wallet tabs, never the focused one the vision model works in
    const pages = ctx.pages().filter(p => !isConsole(p));
    let extra = pages.length - 3;
    for (const p of pages) {
      if (extra <= 0) break;
      if (p.url().startsWith(`chrome-extension://`) || p === lastWork) continue;
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
