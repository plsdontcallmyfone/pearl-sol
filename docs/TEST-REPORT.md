# Test report: full sweep (2026-10-17 / 18)

Test-sweep subagent, run on the Mac while the owner's demo stack was up on 10600-10662
(never stopped or restarted). One heavy build at a time. Times are wall-clock seconds of
the final run (cached builds are fast; cold builds noted). Counts are tests, not files.

## pearl-sol (HEAD at the start: f699594; fixes 96131e0, a113437, a3d85a9)

| Suite | Command (cwd) | Passed | Failed | Ignored | Time |
|---|---|---:|---:|---:|---:|
| chain workspace (agent-types, state, executor, consensus-pow, sync, net, mempool, rpc, blobs, bin/agent-node incl. network + open_network) | `cargo test --workspace --offline` (chain/) | 82 | 0 | 0 | 6 |
| crates workspace (pearl-job, pearl-chain, pearl-node, miner, attester, pool) | `cargo test --workspace --offline` (crates/) | 72 | 0 | 1 | 86 |
| pearl-cli (own workspace, Rust 1.95.0) | `cargo test --offline` (crates/pearl-cli) | 35 | 0 | 0 | 1 |
| pearl-verify-sbf (standalone) | `cargo +1.95.0 test --offline` (crates/pearl-verify-sbf) | 12 | 0 | 0 | 163 |
| header-relay (standalone) | `cargo test --offline` (services/header-relay) | 0 | 0 | 0 | 68 |
| bridge LiteSVM | `cargo test --offline` (token/tests/bridge-litesvm) | 13 | 0 | 0 | 16 |
| light client LiteSVM (incl. real devnet fixture + replay) | `cargo test --offline` (token/tests/lightclient-litesvm) | 13 | 0 | 0 | 140 |
| reserve LiteSVM (real pump bytecode) | `npm test` (token/tests/reserve) | 16 | 0 | 0 | 2 |
| storefront LiteSVM | `npm test` (token/tests/storefront) | 15 | 0 | 0 | 5 |
| relayer | `npm test` + `npm run typecheck` (services/relayer) | 33 | 0 | 0 | 6 + 3 |
| storefront-x402 | `npm test` + `npm run typecheck` (services/storefront-x402) | 10 | 0 | 0 | 2 + 3 |
| crank | `npm run typecheck` (services/crank; no test script) | tsc clean | | | 3 |
| extension unit | `npx vitest run` (extension/) | 63 | 0 | 0 | 4 |
| extension build + typecheck | `npm run build` (extension/) | clean | | | 11 |
| extension headless UI (Playwright Chromium, throwaway profile) | `node scripts/ui-check.mjs` (extension/) | 38 | 0 | 0 | 117 |
| chain-litesvm e2e (superseded SVM agent chain) | `cargo test --offline` (crates/chain-litesvm) | 1 | 0 | 0 | 5 (cold 241) |
| escrow LiteSVM (superseded agent/) | `cargo test --offline` (agent/tests/escrow-litesvm) | 7 | 0 | 0 | 2 |
| mining LiteSVM (superseded agent/) | `cargo test --offline` (agent/tests/mining-litesvm) | 18 | 0 | 0 | 1 |
| agent-cli (superseded) | `npm run build`; `node dist/index.js selftest [--pearl-job]` | 2 of 2 modes ok | 0 | | 2 |
| SBF build, token programs | `cargo build-sbf --manifest-path programs/<p>/Cargo.toml --sbf-out-dir <scratch>` (token/) for bridge, lightclient, reserve, storefront | 4 built | 0 | | 25 |
| SBF build, agent/ programs (superseded) | same, agents, escrow, jobs, mining (agent/) | 4 built | 0 | | 124 |
| token suites against the FRESH bridge.so + storefront.so | bridge LiteSVM, light client LiteSVM, relayer, storefront-x402, storefront, reserve with the scratch builds swapped into token/target/deploy, originals restored after (hashes checked) | 13 + 13 + 33 + 10 + 15 + 16 | 0 | 0 | 26 |
| demo e2e (scripts/demo.sh) | not run here: the owner's stack holds 10600-10669 | being run on the VPS by another agent | | | |

Totals (pearl-sol, distinct suites; the fresh-.so reruns not counted again): 428 tests
passed, 0 failed, 1 ignored, plus both agent-cli selftest modes and clean tsc on relayer,
storefront-x402, crank and extension; 8 SBF programs build.

Ignored: `services/miner/tests/producer.rs::bench_rates`, a nonce-rate benchmark (no
assertion of correctness), meant for `cargo test --release -p pearl-miner --test producer
-- --ignored --nocapture bench`. Not run: a release build would relink
crates/target/release binaries that the running demo uses.

### Failures found and fixed

1. `crates/chain-litesvm/tests/e2e.rs` FAILED: `job ... config k=2048 but k field=1024`.
   The test hard-coded k 1024 while `MiningConfig::localnet_default()` is k 2048 / r 128
   since the rank penalty. Fix: k comes from the config (a113437). The test itself is the
   regression test; it passes.
2. `services/agent-cli selftest --pearl-job` FAILED: `pearl-job config failed: k must be
   >= 16r`. The pearl-job round trip asked for k 1024 (refused since the rank penalty).
   Fix: k 2048 for the gen/config/commit round trip only; the terms_hash vector keeps the
   old k 1024 on purpose (it mirrors the superseded agent/ vector) (a3d85a9). Also
   commits crates/chain-litesvm/Cargo.lock, which picks up pearl-node.
3. Port hygiene, not a failure: `services/miner/tests/producer.rs::over_http_jsonrpc` bound
   a free port in 10663-10669 (the demo's service block). Moved to 10690-10699 (96131e0).

### Found, not changed (for the owner or the owning lane)

- **token/target/deploy/bridge.so and storefront.so are older than their source.** A
  fresh build differs: bridge `f571bc74...5e78` (deployed `599467df...de29`), storefront
  `c650bf02...4f4a` (deployed `5878849d...e6`). lightclient (`b0816cad...7b48`) and
  reserve (`9c484073...2c5a`) rebuild byte for byte. Both fresh builds pass every suite
  above, so source and tests agree; the deployed files were left as they are because
  ~/agent-credit pins `bridge.so 599467df...` (checks/m55-pearl.sh) and the running
  demo loaded the old ones. Rebuild into target/deploy (and re-pin agent-credit) when the
  demo is next restarted.
- `crates/pearl-verify-sbf` had no rust-toolchain.toml: `cargo test` on the machine
  default (1.94.1) failed on sysinfo 0.39.6 (needs 1.95). FIXED in the bug-fix pass below.
- `services/header-relay` has 0 tests (it compiles; covered by the light-client LiteSVM
  fixture and the demo).
- extension ui-check picks its mode itself: with the demo stack up it ran the read-only
  live smoke (38 checks); the 45-check mock mode only runs when 10600/10630 do not answer.
- token/package.json `lint` (prettier) and Anchor `[scripts] test` (ts-mocha
  tests/**/*.ts) are scaffold leftovers: no node_modules, no .ts tests. Not run.

## pearl-wallet (branch dev, HEAD 4f1c4b5 + CLAUDE.md row)

| Suite | Command | Passed | Failed | Time |
|---|---|---:|---:|---:|
| unit (vitest) | `pnpm test:unit` | 103 (17 files) | 0 | 15 |
| typecheck | `pnpm typecheck` | clean | | 20 |
| lint (prettier, unused-exports, deps, eslint, ls-lint) | `pnpm lint` | clean (deps: 4 no-circular warnings, 0 errors) | | 25 |
| build | `pnpm build` | ok (2 webpack size warnings) | | 13 |
| headless UI (Playwright Chromium, mock RPC 10681-10684) | `node scripts/pearl/ui-check.mjs` | 56 of 56 | 0 | 273 |
| live check | `node scripts/pearl/live-check.mjs` | SKIPPED | | |

Live check skipped: it is not read-mostly. With the demo user key it sends on both chains,
bridges in and out, buys from the storefront, posts a job and sets/clears a payout lien on
the stack the owner is using. Run it on the VPS stack or after the owner is done.
Note: `run-checks.sh` (the `pnpm checks` hook) still called `pnpm lint:remote-wallet-config`,
which no longer exists in package.json. FIXED in the bug-fix pass below.
The UI check rewrites screenshots/*.png; they were restored to the committed ones.

## agent-credit (pushed to danishseal/agent-credit: 71e68ca, 8fd9cc4, 8fdc0a6)

| Suite | Command | Result | Time |
|---|---|---|---:|
| full check set | `PEARL_E2E_RPC_PORT=10693 ./scripts/check-all.sh` at 71e68ca | build + recorded hashes ok; checks 81 of 88 first pass | 13,894 |
| Rust workspace (ac_core, ac_pearl, credit_register, LiteSVM suites) | `cargo test --offline --workspace` (step 02) | 278 passed, 0 failed, 0 ignored | 152 |
| node tests (client, keeper incl. pearl jobs, decoder, services, scripts) | `node --test` (step 99) | 659 of 659 | 3 |
| ac_pearl math + LiteSVM pearl suite | checks/m55-pearl.sh | PASS | 24 |
| pearl e2e, real validator + live pearl-sol agent-node on 10693-10695 | checks/m56-pearl-e2e.sh | PASS, 80 steps | 40 |
| rerun of the 7 failures at 8fd9cc4 | `check-all.sh --only <name>` each, with AC_CHROME set | 7 of 7 PASS (so 88 of 88) | 13,058 |

First-pass failures:
- m0-merge: its list of allowed program dirs predated ac_pearl. Fixed (ac_pearl allowed, any other new dir still fails).
- m42-review-red: the review patch's keeper hunk no longer applied once AC_PEARL joined KEEPER_PROGRAMS. Re-anchored; still 7 of 7 red on the unfixed code.
- m5-render, m21-web, m27-web-sign, m41-click-through, m51-devnet-sign-pass: environment. The Playwright headless shell is now installed as `chromium_headless_shell-1169/chrome-mac/headless_shell`, and the scripts' `chromePath()` does not find it at that path. They pass with `AC_CHROME` pointing at that binary. The scripts were not changed.
- Port change: `scripts/pearl-e2e.sh` now reads `PEARL_E2E_RPC_PORT` (default 10656 as before), so the run stayed off the demo's 10600-10669.
No program change. Nothing sent on any chain.

## Bug-fix pass (2026-10-18)

Owner: "fix any bugs". Mac demo stack untouched; nothing sent on a public chain.

| Item | Fix (commit) | Result |
|---|---|---|
| pearl-sol: `crates/pearl-verify-sbf` plain `cargo test` failed on 1.94.1 | `rust-toolchain.toml` pins 1.95.0, same as chain/ and crates/pearl-cli (c8e38d8). The SBF build of lightclient runs from token/ and does not read it | `cargo test --offline`: 12 passed, 0 failed |
| pearl-wallet: `pnpm checks` failed at once (`lint:remote-wallet-config` gone) | removed it; the audit moved out to its own `pnpm audit:prod` (live advisory DB); vitest `testTimeout` 30 s because the vault KDF specs timed out under the parallel run (47f539c) | `pnpm checks`: 8 of 8 SUCCESS (prettier, eslint, ls-lint, deps, unused exports, typecheck, unit 103 of 103, build) |
| pearl-wallet: Leather CI leftovers | removed 10 workflows for Leather's org, stores, Chromatic, semantic release and the deleted tests/specs + tests/api suites; code-checks lost lint-message-schema and lint-commit (no commitlint config), audit is report-only; build workflow without Leather secrets; check-version-lock no longer imports the absent `@actions/core` (47f539c) | every remaining job run locally: lint x5, typecheck, unit, build + build-ext.sh, Firefox build + `web-ext lint` (0 errors, 17 warnings), version lock (139 exact) |
| pearl-wallet: high advisories in bundled code | react-router 7.6.2 -> 7.18.4, lodash-es override 4.18.1 (81951c9) | unit 103, UI 56 of 56 on the new build |
| agent-credit: 5 web checks did not find the headless shell | `web/scripts/chrome-path.mjs`: AC_CHROME, then newest `chromium_headless_shell-*` in either layout, then full Chromium with `--headless=new` (65f0c8b, 5f7f08a, pushed) | without AC_CHROME: m5-render, m21-web, m27-web-sign, m41-click-through, m51-devnet-sign-pass all PASS; node tests 665 of 665 (6 new) |

pearl-wallet suites after the fixes: unit 103 of 103, headless UI (mock RPC) 56 of 56 (284 s;
screenshots restored to the committed ones).

Still open (reported, not changed):
- `pnpm audit:prod` still reports 21 low, 82 moderate, 106 high, 6 critical, nearly all in build
  tooling listed as dependencies (expo and react-native under @leather.io/ui, css-loader,
  style-loader, postcss-preset-env, storybook, @typescript-eslint). Moving those to
  devDependencies or overriding them is a dependency overhaul, not a small fix.
- A later `pnpm checks` rerun failed 3 unit specs (drift.spec storefront snapshot, solana.spec
  buy_job / buy_job_amm 26 vs 27 accounts): the storefront buyback lane changed
  token/target/idl/storefront.json at 10:25 (WIP 3afa7c5). The wallet's storefront snapshot
  and buy_job builders must follow once that lands; left to that lane.

### Wallet live check on the TEST VPS (64.95.13.106)

Run ON the box so the demo keys (.demo/keys) never left it: pearl-wallet dev 81951c9 sent as a
`git archive` (the Mac repo is shallow, a bundle cannot carry it) to /home/pearl/pearl-wallet,
`pnpm install --frozen-lockfile`, `pnpm build`, Playwright 1.52 headless Chromium plus its apt
deps (`playwright install-deps chromium` as root; the box says a reboot is pending for a new
kernel, not done). `node scripts/pearl/live-check.mjs` against the box's kept-up stack, 448 s.

Result: **20 of 21 PASS.** The one FAIL: "bridge out (BridgeLock) tracked to the Solana release:
stage relaying, withdrawal #3, 194/6 conf". Cause on the box, not the wallet: the bridge is in
proof mode (relayer signer path off), so releases wait for header-relay, and header-relay has
fallen far behind the chain on this long-running stack: it was verifying header ~12,600 while
the chain was at ~24,460 (relayer log), about 9 headers a minute on a box at load 26 to 31
(int7 experiment + buyback builds), slower than blocks are made. So no bridge-out on that stack
can be released until header-relay catches up or the stack restarts. Not restarted here (other
lanes use the stack). Worth a look by the header-relay owner: on a long-lived stack the relay
never catches up.

## Token-side buyback cap + permissionless fund_mining + public numbers (2026-10-19)

Programs built on the test VPS in a separate clone (`~/pearl-tok`, same program ids, keypairs
from keys/localnet); LiteSVM JS suites run on the Mac with `TOKEN_TARGET` pointing at the
fetched build (the litesvm 0.3.3 node binding aborts with std::bad_alloc on the box, also
on the untouched tree, so those suites stay on the Mac).

| Suite | Command (cwd) | Passed | Failed |
|---|---|---:|---:|
| reserve LiteSVM (real pump bytecode + bridge) | `TOKEN_TARGET=... npm test` (token/tests/reserve) | 18 | 0 |
| storefront LiteSVM | `TOKEN_TARGET=... npm test` (token/tests/storefront) | 23 | 0 |
| pearl-cli (VPS) | `cargo test --release` (crates/pearl-cli) | 36 | 0 |
| explorer | `npm test` (apps/explorer, at 992d459) | 28 | 0 |
| DefiLlama adapter unit | `npm test` (tools/defillama) | 2 | 0 |
| crank typecheck | `npm run typecheck` (services/crank) | ok | |
| pearl-wallet vitest (dev, IDL fixtures refreshed) | `npx vitest run` | 134 (+3 skipped) | 0 |

New tests: reserve: fund defaults, permissionless fund by a stranger, per-call cap, interval,
window cap and rollover, pause (admin included), destination frozen after the first funding,
set_fund_limits admin-only + event, migrate_config on a 276-byte old config. Storefront: cap
default, set_buyback_cap admin-only + event, a buy crossing the cap (reserve exactly at cap,
excess burned, supply down by the excess, events), a buy at the cap (all burned), cap 0 = no
cap, the PumpSwap crank under the cap, bridge vault untouched and `1e18 - escrow <= 1000 x
mint supply`. CU: crank_buyback with the split 127.5k.

Live, read-only against the VPS demo stack through a tunnel:
- `tools/defillama` `npm run live`: all-time fees 22,240,866 lamports, holders revenue
  1,112,043, supply side 20,016,780; all three equal the storefront Config counters
  (total_sol, total_sol + every share ever held, bb_accrued). Rerun later at slot 4,891 the
  validator had pruned the history and every check reported a mismatch (as designed: the
  adapter needs an RPC with full history for its window).
- explorer `/api/supply`: reserve 25,062,907,355,180 = 35,047,085,691,636 bought - 1e13 funded
  + 15,821,663,544 from the buyback; bridge in_flight 0; circulating = supply - reserve - pool
  exactly. The first run found reserve/bridge reading 0 (wrong ATA program id), fixed in
  992d459 with a vector test. `/revenue` screenshots (desktop + phone, headless):
  screenshots/explorer/revenue*.png.
