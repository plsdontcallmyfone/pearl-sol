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
//
//   node /home/user/.cadre/deskhost.mjs <social|work> <siteUrl> <explorerUrl>
import { chromium } from 'playwright-core';
import { existsSync, readFileSync, rmSync, writeFileSync, renameSync } from 'node:fs';

const [mode = 'social', site = 'https://cadre.64-95-12-116.sslip.io', explorer = `${site}/explorer`] = process.argv.slice(2);
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

const startTabs = mode === 'work'
  ? [walletUrl, explorer, `${site}/buy`]
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
for (;;) {
  await unlockIfLocked().catch(() => {});
  // keep the wallet's own tab count small on a 1 GiB desktop: at most 4 tabs
  const pages = ctx.pages();
  if (pages.length > 4) for (const p of pages.slice(0, pages.length - 4)) if (!p.url().startsWith(`chrome-extension://`)) await p.close().catch(() => {});
  const tabs = [];
  for (const p of ctx.pages()) tabs.push({ url: p.url().slice(0, 300), title: (await p.title().catch(() => '')).slice(0, 120) });
  const st = { t: Date.now(), mode, ext: extId, address, tabs };
  writeFileSync(`${HOME}/state.json.tmp`, JSON.stringify(st));
  renameSync(`${HOME}/state.json.tmp`, `${HOME}/state.json`);
  await sleep(5000);
}
