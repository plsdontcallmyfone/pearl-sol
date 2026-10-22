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

## Agent layer (2026-10-19: RejectResult, open evaluation, profiles, cadre-proxy)

| Suite | Where | Result |
|---|---|---|
| chain workspace (`cargo test --workspace`, HEAD + RejectResult) | test VPS | 95 pass, 0 fail (new: executor reject 4, node RPC reject_rpc 1, state reject roundtrip 1, vectors reject.json) |
| pearl-attester (`cargo test -p pearl-attester`) | test VPS | 13 pass (6 attester + 7 evaluator: accept -> CompleteJob, wrong C -> exact RejectResult bytes, hash/size rejects, holds, unavailable grace, threshold 2 via peer, unfunded) |
| storefront-x402 (`npm test`, LiteSVM) | Mac | 14 pass (+2: GET /evaluators, buyer-named evaluators posted as given) |
| extension (`npx vitest run`) | Mac | 62 pass, 3 skipped (reject vectors) |
| ~/pearl-wallet src/shared/pearl + src/background/pearl | Mac | 70 + 14 pass (reject vectors, snapshot fixture) |
| apps/explorer | Mac | 32 pass (+12 agent profile, card probe, routes) |
| pearl-cli | test VPS | 38 pass (+2 `pearl agent`) |
| services/cadre-proxy (`node --test test/*.test.mjs`) | Mac | 9 pass (mock gateway) |

Live: explorer agent profile + `pearl agent` read-only against the test box's chain (:10630).
cadre-proxy live check against the real gateway still pending (nothing on 10664 then).

## Cadre inference + Cadre web asks (2026-10-19, standing jobs / inference takeover)

Full matrix `bash deploy/test.sh` on the test VPS at b5638ea (log ~/test-takeover.log on the box),
plus the two LiteSVM node suites on the Mac (the litesvm node binding aborts with std::bad_alloc
on the box, as noted at the top of deploy/test.sh):

| Suite | Where | Result |
|---|---|---|
| chain workspace | test VPS | 95 pass, 0 fail (291 s) |
| crates workspace (pearl-job, pearl-node, miner, attester, pool, cadre-gateway) | test VPS | 97 pass, 0 fail, 1 ignored |
| pearl-cli | test VPS | 38 pass |
| header-relay (+ devnet feature build) | test VPS | 3 pass, build clean |
| bridge LiteSVM | test VPS | 13 pass |
| light client LiteSVM | test VPS | 18 pass |
| apps/explorer | test VPS | 32 pass |
| services/cadre-api | test VPS and Mac | 31 pass (6 new unsigned pump/crank/fund tx tests byte-equal to the crank-side builders; prepaid proxy, keys, receipt headers, CORS, credit line, card probe) |
| tools/cadre (int7 worker, `python -m unittest test_cadre`) | test VPS | 5 pass (new: int_mm owns only its rows) |
| services/relayer | Mac (box: bad_alloc) | 33 pass |
| services/storefront-x402 | Mac (box: bad_alloc) | 14 pass |
| ~/cadre-web unit / lint / typecheck | Mac | 13 pass, clean, clean |

Demo `scripts/demo.sh` on the test VPS: 11/11 PASS in 740.5 s (~/demo-takeover2.log), step 11
(paid inference) 566 s. Two earlier runs that day failed only step 11 ("finalized by null": the
token validator's 10000-shred history; fixed in scripts/localnet.sh).

cadre-web live e2e (`npm run e2e` on the Mac, tunnel 1:1 to the box stack, cadre-api on 10711,
stack up since 19:56 UTC): 18/18 once (21:25 UTC, before the x402 check was made stricter);
with the stricter check (the x402 call must produce its own new receipt) 16/18 at 22:15 UTC:
every AI check passed (prepaid top-up over x402, wallet-signed answer with receipt, x402 per
request with its own receipt), the crank page showed the program's NothingToBuy reason, and
bridge in failed to show a result within 60 s (the same step passed in the other runs; the
bridge-out and register steps failed the same way in the 21:13 run and passed in the others).
Two other runs broke on the environment (local index backfill at the first run, a network
drop of the tunnel). The cadre-worker was OOM-killed once during the e2e (fixed in bdf5a80,
measured: 64-token generation peak RSS 6.4 GB before, 2.3 GB after).

Round 2 (2026-10-19/20, Llama-3.2-1B-Instruct): demo 11/11 PASS in 740.6 s (~/demo-instruct.log on
the box; step 11 554.6 s, chat answer "Paris"); tools/cadre unittest 5 pass on the box; cadre-proxy
`node --test` 9 pass (the x402 test now also covers the storefront as fee payer and payTo, checked with storefront-x402
verifyExact) and a live paid request through the proxy; cadre-web e2e 18/18 at 01:21 UTC with the
strict x402 check, after fixing the e2e's approval pump (the earlier intermittent wallet-tx
failures).

## Public devnet (2026-10-19/20, "devnet finish" agent)

Commit under test for the chain and programs: fe8e04a (devnet nodes and Solana programs), later
commits for services (miner orphan sweep 9a352f1, header-relay fixes 2871eeb..16e89ca and the lanes
commit, explorer SOL_POLL_MS). Suites:

- On devnet box 0 (`deploy/test.sh`, commit dc8ee22): chain 95, crates 96 (1 ignored), pearl-cli 38,
  header-relay 3, header-relay devnet build, bridge LiteSVM 13, lightclient LiteSVM 18, explorer 32: all PASS.
  relayer and storefront-x402 abort there with litesvm's std::bad_alloc (known, HANDOFF item 13); on the
  Mac: relayer 33/33, storefront-x402 14/14 PASS.
- After the fixes (box 0): pearl-miner producer 10 + pool 4 + standing 4 PASS (new
  delivers_orphaned_mined_out_job); header-relay 3 and lightclient LiteSVM 16 + 2 PASS after each
  header-relay change; explorer 32 and cadre-api 31 PASS on the Mac after SOL_POLL_MS.
- pearl-wallet (dev, production build): unit 144/144, headless UI 73/73 (after merging danishseal
  cadre-chain's pixel wordmark; the font check now counts Geist under base.css's aliased family names).

End to end against the public devnet (scripts/devnet-e2e.sh from the Mac): see docs/DEVNET.md
"End to end results" for the step lines. Run 1 FAIL at step 4 (light client behind, see the fixes
there; its flows settled after the run by cadre-settle). Run 2 (2026-10-20 01:30-03:41 UTC): 7/7 PASS
in 130.9 min (steps 3 30.6, 4 33.7, 6 65.7 min), invariant OK after every step, light-client lag 0-3
from step 4 on. A second passing run (run 3) was not done: the owner chose a dedicated Solana RPC
(2026-10-20) and asked to rerun twice on it; its URL file was not provided by the end of this session.
Redaction and wrapper checks: explorer 32, cadre-api 31, relayer 33 PASS on the Mac; header-relay 3 PASS
on box 0; with-rpc falls back to the env value without the file and exports the file's URL with it.

## Devnet site + paid AI (2026-10-20, "devnet site + AI" agent)

- Website on box 0 (https://cadre.64-95-12-116.sslip.io, cadre-web a9c903a): headless Chromium over the
  hosted URL, 8 pages + 3 detail pages x desktop/phone, 0 console errors, no horizontal overflow, live data
  (twice: before and after the gateway came on). `curl -fsSL <site>/install-miner.sh | bash -s -- --dry-run`
  8/8 steps, devnet node as default RPC.
- Model copy test box -> box 0: 329 files, sha256 list identical on both boxes.
- Worker bench on box 0 (docs/measurements/cadre-worker-devnet-box0-2026-10-20.jsonl): 2.09 / 2.00 tok/s
  decode at 8 / 64 prompt tokens, 2 threads, load 2.3-2.5.
- Paid flow scripts/devnet-inference.sh run devnet-ai-1: top-up 201, chat 200 with receipt, close 202,
  Order 12 finalized by finalize_order_proof (result_hash = the ticket's), 144/144 segments mined. The
  script exited 1 at its last check (it read only the newest tx on the Order, a settle_share); the check
  is fixed and the finalize tx was decoded by hand (DEVNET.md "Inference").
- pearl-cli: release build on box 0 after the inclusion-wait change. pearl-wallet: tsc clean,
  src/shared/pearl unit 104/104 after the preset change.
- e2e run 3 by the header-relay agent ran in parallel and passed (exit 0, 162.0 min) across the box 0
  node restart (05:56 UTC) and cadre-api restart (06:17 UTC).

## Light client on devnet (2026-10-20, "light-client VerifyFailed" row, resumed)

- header-relay on box 0 (no devnet feature, against token/target/deploy/lightclient.so from fe8e04a):
  `cargo test --release` 6/6 PASS, incl. the new two_lanes_send_each_shared_step_once (two relays on one
  LiteSVM ledger, own payers: no grace 23 failed txs, errors Custom(0), 6023, 6027, the devnet set;
  owner rule 0 failed, tip 24) and expired_but_landed_txs_are_not_sent_again. Devnet build
  (`--features devnet`) clean, sha256 098fbb52...5c4b5931 at 1a1f4f8.
- Devnet logs (boxes 1-2), 06:38-08:40 UTC on 1a1f4f8: verified 65 + 65, work-verified 28 + 26, failed
  txs 1 + 1 (connect NotWorkDone 6027), zero 6015/6008/6016/6023/Custom(0), zero resends.
- scripts/devnet-e2e.sh runs 3-9 (table in docs/DEVNET.md "End to end results"): 3 and 6 PASS 7/7
  (162.0 and 169.1 min, invariant OK after every step); 4, 5, 7, 8, 9 FAIL, each on a Solana devnet
  public RPC 429 or dropped connection from the Mac. Not reached: two passes in a row.

## Cadre leftovers (2026-10-20: blob fetch, receipt links, AI page, wallet devnet check)

- chain (test box, `cargo test` in a copy of the repo): 96 passed, including the new network test
  model_manifest_uploaded_long_after_registration_reaches_peers (registered model, no upload, every node
  fetching through 3 x grace blocks and 10 simulated minutes; a late 190 KB manifest putBlob on node 0
  reaches nodes 1 and 2 with no restart; a pinned 3 MB chunk follows on demand). chain/Cargo.lock lacked
  executor's hex dependency, so `--locked` builds failed; fixed in b5a55b6.
- crates (test box): pearl-cli 38, cadre-gateway 7 (new receipt_links_use_the_public_api_root),
  pearl-node + pearl-miner + pearl-attester all pass (new model_file_asks_for_every_chunk_at_once).
- Devnet: boxes 1-2 nodes on the new agent-node with 8 GB blob quotas; model blobs uploaded to node 0
  (836 blobs, 32 s); nodes 1-2 `manifest_present: true` within seconds; all three miners mine segment
  blocks (DEVNET.md "Inference"). Receipt link https://api.64-95-12-116.sslip.io/v1/openai/requests/<id>
  resolves publicly; same on the test box (https://cadre-api.64-95-13-106.sslip.io/v1/openai/requests/<id>).
- Reboots into 6.8.0-147 one box at a time with finalized-hash agreement and relay lag 0-2 after each.
- cadre-web 651e702: headless Chromium at 1440, 1280 and 390 px, local and on the hosted devnet site:
  models table scrollWidth equals its clientWidth (was 1295 vs 1190 on desktop, 423 vs 356 on phone);
  `npm run screens` for the AI page clean; lint, typecheck, unit 13 clean.
- pearl-wallet 0a3cf15c3: devnet check 14/14 (the two "no gateway" assertions now check the devnet
  gateway: model list through /v1/openai, prepaid balance, wallet-signed GET /v1/keys), TESTBOX=1 18/18,
  unit 169 (+1 skipped), UI 93/93.

## GPU worker on RunPod (2026-10-20, "RunPod GPU inference + 8B int7 check" row)

One RunPod pod, NVIDIA A40 48 GB, secure cloud on demand at $0.59/hr (96 vCPU, torch 2.5.1+cu124,
transformers 4.46.3), up 15:47-16:21 UTC. Raw lines in docs/measurements/gpu-a40-2026-10-20/.
- CPU path unchanged by the CUDA change (test box, before the pod): tools/cadre/test_cadre.py 5/5; old vs new
  worker on Llama-3.2-1B-Instruct w7a7, same prompt, every A and C file of all 144 matrices identical.
- Integer part CPU vs GPU (tools/cadre/gpu_check.py, 4 chat prompts, 32 new tokens): for every matrix the
  GPU's C equals the CPU's torch._int_mm of the GPU's own A, 0 mismatches in 151,451,392 C elements (576
  matrix segments), and equals a numpy int64 A B on the first 4 rows of each, 0 mismatches. So a segment
  the GPU serves verifies on any machine.
- End to end CPU vs GPU (same prompt): the fp32 glue differs, so A does not: layer 0 A identical, layer 1
  o_proj differs in 0.002% of elements (max 1), and the difference grows through the layers (lm_head A
  differs in 79%). Tokens: 3 of 4 prompts identical, the 4th diverged partway through ("in which" on the GPU,
  "where" on the CPU). Two GPU runs of the same prompt are identical (A and C).
- Speed, 1B Instruct w7a7 (bench.py --device cuda, 128 new tokens): decode 24.5-25.1 tok/s at 8 to 1024
  prompt tokens (one process, one request at a time), ttft 0.05-0.12 s, prefill 9,942 rows/s at 1024;
  writing the 144 A/C segment files 1.9 s (135 rows) to 9.9 s (1,151 rows). Several worker processes on
  the one GPU: 82.6 tok/s summed over 4, 84.9 over 8, 78.1 over 16.
- Speed, Llama-3.1-8B-Instruct w7a7 (640 matrices, 64 new tokens): decode 8.0-8.6 tok/s, ttft 0.14-0.53 s,
  prefill 1,932 rows/s at 1024; segment files 6.2 s (71 rows) to 35.8 s (1,087 rows).
- Paid chat through the test box gateway with the GPU worker (tools/cadre/remote_worker.py on the box in
  the CPU worker's place, ssh tunnel to the pod, same registered model files): x402 top-up 256 rows
  (95,411,375 lamports, order 9) with a throwaway cadre-proxy wallet; "Write two sentences about why the
  sky is blue." 32 tokens: GPU 8.7 s wall, gateway-measured decode 21.13 tok/s, ttft 1.53 s, of which 5.4 s
  was copying the segment files back over the tunnel; the CPU worker on the same box just before: 110.6 s
  wall, 0.376 tok/s, ttft 26.9 s, same text. The box was switched back to its CPU worker (same command line,
  pid file updated) before the pod was terminated; the balance ticket was closed.
- 7-8B int7 accuracy: research/INT7-ACCURACY.md section 7 (WikiText-2 PPL +3.05% on Llama-3.1-8B-Instruct).

## RegisterModel matrix limit 384 -> 1024 (2026-10-20, "RegisterModel matrix limit for 8B" row)

Test box 64.95.13.106, own dir ~/regmodel, release builds (HANDOFF item 21).

| suite | result |
|---|---|
| chain workspace (`cargo test --release --workspace`) | 102 passed, incl. new: agent-types `register_model_limit_and_size`, `model_matrices_schedule`; executor standing `register_8b_model_640_matrices_and_post_a_full_round`, `devnet_takes_640_matrices_only_from_its_upgrade_height`, `measure_register_and_post_cost`; agent-node network `register_640_matrix_model_gossips_and_mines` |
| crates/pearl-cli (`cargo test --release --locked`, refreshed Cargo.lock) | 38 passed |
| crates/pearl-job (`cargo test --release -p pearl-job`) | 42 passed |
| e2e: 2 real agent-node processes + `pearl model register` / `model fetch`, synthetic manifests | 640 registered and fetched (every hash_b checked on node 1), 1024 registered (getModel 1024 on both nodes), 1025 refused, on chain 1347616811; devnet chain id: 640 refused below 4200 and registered after it, 384 registered below it |
- devnet e2e off the Mac: box 0 run box0-1 7/7 (96.7 min), box0-2 failed preflight (box 0's IP in Solana
  429s); test box tb-1 failed step 3 (storefront-x402 crashed on an unhandled 429), tb-2 failed preflight
  (same throttle); then tb-3 7/7 (140.3 min) and tb-4 7/7 (131.4 min) in a row. Paid inference run on box 0
  after the reboots: "Paris" with a public receipt link, 144/144 segments mined 64.2 min after the close
  (138 min before the fix with box 0 alone), Order finalized by finalize_order_proof, result hash = ticket's.


## Verifiable AI: cadre-glue-v1 replay (2026-10-21, "verifiable AI: deterministic glue replay" row)

Model: Llama-3.2-1B-Instruct int7 (recipe 1, 144 matrices). Raw lines:
docs/measurements/verify-ai-2026-10-21/.

| Suite | Where | Result |
|---|---|---|
| tools/cadre/test_cadre.py (tiny synthetic Llama; + `test_glue_prefill_equals_token_by_token`, `test_glue_tables`, `test_sqrt_is_correctly_rounded`, `test_fp32_glue_is_still_selectable`) | test box | 9/9 OK |
| tools/cadre/test_replay.py (real 1B model, Python worker serves, Rust `cadre-replay` replays) | test box | 19/19: tables Python = Rust (sha256 757b2c06...), Rust decode = HF on 60 random sequences, 4 honest requests replayed bit for bit (48-65 rows; final residual stream sha256 equal Python vs Rust), 3 cheaters (one A element changed by one step, C recomputed so the GEMM is correct) caught at layers.0.qkv, layers.4.down{2048:4096}, lm_head[120240:128256] with matrix, row and column named, 3 fraud proofs valid and refused on the honest A, wrong last token caught (tokens), wrong first token caught (activation), wrong text hash caught, truncated answer caught |
| services/attester tests/replay.rs `ticket_full_replay_accepts_honest_and_rejects_altered_activations` (real evaluator, mock node, 2 requests in one round, 100 rows, 144 segments) | test box | PASS: honest Accept in 19.2 s (96 rows); altered A in matrix 61 Reject naming segment 4df38dcc..., row 90 col 7, request req1, proof at /proofs/<ticket> verifies against the altered blob and fails on the honest one; wrong C Reject ("committed C is not A B", matrix 3) |
| services/attester (all) + crates/cadre-glue + services/inference-gateway | test box | 34 passed, 0 failed (incl. gateway.rs 10, cadre-glue tables 4, the replay test) |
| services/inference-gateway tests/gateway.rs (+ `served_requests_keep_the_replay_bundle`) | test box | 10/10 |
| crates/cadre-glue tests/tables.rs | Mac + box | 4/4 (found and fixed a gemm_bt slice bug for n smaller than the thread count) |
| tools/cadre/gpu_replay_check.py (RunPod A40 vs the pod's Xeon Gold 6342, same model files) | pod | PASS: cadre-glue-v1 0 of 144 matrices differ in A or C on all 4 prompts, tokens equal, Rust replay of every GPU-served request "honest"; fp32 glue 126, 126, 144, 144 matrices differ and 2 of 4 answers changed |

## Ecosystem Phase 1: the town (2026-10-20/21, "ecosystem Phase 1" row)

| Suite | Where | Result |
|---|---|---|
| chain workspace (`cargo test --workspace`, incl. executor tests/town.rs: activation height, social fee rule + spam limits, profiles/handles/house flags, posts/reactions/tips/follows, large balance tickets pro rata + subsidy bound; agent-node tests/social_rpc.rs: social RPC, reorg rollback of social state, profile blob kept while referenced then pruned; state config extension encoding; vectors social.json) | test box | 108 passed before the merge with the matrix limit, 114 passed on the merged tree, 0 failed |
| crates `cargo test -p cadre-gateway -p pearl-job` (gateway: large balances + round period + round cap, text_sha256 receipts) | test box | 51 passed (gateway 9 before the merge) |
| services/storefront-x402 (LiteSVM service suite + new pricing.test.ts) | Mac | 16/16 |
| apps/explorer (incl. test/social.test.ts against vectors/social.json) | Mac | 41/41 |
| services/cadre-api (incl. test/town.test.ts, OpenAPI parity) | Mac | 35 pass, 0 fail, 9 skipped (market-tx tests need crank deps in the worktree) |
| services/swarm (budget, filter, nonces, vectors, 400-tick soak, injection corpus) | Mac | 17/17 (after the two live fixes below) |
| extension tx-vectors | Mac | 38/38 |
| ~/pearl-wallet unit / headless UI (mock) | Mac | 179 passed, 2 skipped / 108/108 |
| ~/cadre-web unit / e2e on mocks / lint, typecheck, build | Mac | 21/21 / 24/24 / clean |
| live headless check of https://cadre.64-95-12-116.sslip.io /town, /agents/3yGRgt..., /u/tavern_mags, /leaderboards at 1440 and 390 px | Mac, Chromium | no horizontal overflow, no monospace, no em dash, posts with "written by paid inference" badges render; every page also logs the site-wide Privy errors (auth.privy.io frame-ancestors, 403 on analytics_events) from the redesign's connect modal, nothing else |

Live fixes found on devnet: the 1B model echoed the `<data>` context tags into every post (context is now
quoted dash lines and the filter refuses an echoed line), and the swarm's upper-bound row estimate (about 7x
the real rows) refused most thoughts against the per-agent share (the share is now checked on booked rows,
the day cap still on the estimate).

## Fresh node sync stall (2026-10-21, "fresh node p2p sync stall" row)

Fix aa5db01 (HANDOFF item 24). Test box 64.95.13.106, own dir ~/sync-fix, release builds.

| Suite | Where | Result |
|---|---|---|
| chain workspace (`cargo test --release --workspace`), incl. new agent-node tests/fresh_sync.rs `fresh_node_syncs_devnet_history_past_both_upgrades` | Mac (a5c365a) and test box (same chain tree, rebased) | 115 passed, 0 failed (114 before + the new test; network.rs 14/14) |
| the new test on main before the fix | Mac | fails: the fresh node stays at height 1 |
| length-only sync (fallback blocks only) at b54e0c9 | Mac | 3000 of 3000 synced; 4350: stops at 1 |
| live: box-built main, fresh data dir, devnet seeds | test box | stuck at height 1, 3 peers ("orphan pool full") |
| live: agent-node with the fix, fresh data dir, devnet seeds | test box | 4352 blocks in under 80 s, tip and finalized hash (4322, 3ac10d0a...) equal to all three devnet nodes, no bans |
| live: NODE.md recipe from a fresh public clone (main 2ccfba4, `--locked`) | test box | build 555 s; 4,402 blocks in about 50 s, finalized 4372 83ea2066... equal to the devnet nodes, 388 MB, RSS 121 MiB |

## House desktops (2026-10-21, "house desktops" row)

| Suite | Where | Result |
|---|---|---|
| services/swarm/desktop `npm test` (fake E2B, fake vision model, fake node: tokens, ledger cap + day roll, task texts, vision loop order + cost + screenshot pruning + step and spend limits, desk create/reconnect/duplicate kill/renewal, controller post typed verbatim and found by nonce, keys and wallet password never in a model request, no sandbox id in status, day cap pauses all) | Mac | 7/7 |
| services/cadre-api (incl. test/desktops.test.ts: token vector shared with the controller, list with stream paths, detail, HLS proxy 403 on forged or missing token, 400 on path tricks, 503 without a controller; OpenAPI parity) | Mac | 49/49 |
| ~/cadre-web unit / lint / typecheck | Mac | 28/28 / clean / clean |
| headless Chromium (Playwright, not a real browser profile) on /live and /live/<handle> at 1440 and 390 px against cadre-api with the real devnet controller (12 desktops) | Mac | no horizontal overflow; all visible tiles live (Playwright's Chromium has no H.264, so the 2 s stills path played; real browsers play the HLS); only console errors are Privy's analytics 403 |
| E2B concurrency: 40 `cadre-desktop` sandboxes created at once | Mac -> E2B | 40/40 in 18 s, all listed running, killed |
| live runs on box 0: 1 agent x 2 desktops 15 min, 3 x 2 17.6 min, 6 x 2 31 min | box 0 -> E2B + OpenRouter | 6/5, 20/17, 24/23 read-only tasks ok; costs in ECOSYSTEM 2.10 |
| live proof (`node src/proof.ts`): paid thought -> the probe's desktop connects the wallet, enters the text, signs the Post | box 0 -> devnet | 4 runs: 4322 (quotes lost: model retyped), 4347 and 4439 (byte identical, no thought_ref: old site, then the model skipped the URL), **4451 PASS** (body identical, thought_ref verified, receipt link) |
| public site https://cadre.64-95-12-116.sslip.io/live and /live/ferry_nan after the 16:55 UTC deploy, 1440 and 390 px | Mac, Chromium | no overflow, stills live; a served segment probed in the desktop: H.264 Constrained Baseline 1280x800 4 fps 356 kbit/s; public HLS playlist 200, segment 200 (80 KB), forged token 403 |
| tools/cadre/logits_check.py (A40 vs Xeon, 3 WikiText-2 windows of 1024 tokens) | pod | binary64 logits bit-identical on all 3 (131,334,144 values each) and every A / C equal; before the sqrt fix 362k-745k logits differed by 1 ulp (torch CPU sqrt via SLEEF, 5 of 1024 rows of `inv_rms`), now fixed |
| Accuracy, A40 (eval_accuracy.py, 40 x 1024 WikiText-2 + 200 items per task) | pod | cadre-glue-v1 PPL 16.1770 vs fp32 glue 16.1694 (+0.05%); ARC-E 0.625 / 0.640, ARC-C 0.320 / 0.330, HellaSwag acc_norm 0.575 / 0.585 (within the ~3 point SE) |

GPU: one RunPod A40 at $0.59/hr, 15:49-16:55 UTC, $0.66 spent, terminated (API shows no pods,
spend $0/hr). Not covered: the live devnet / test-box gateways still run workers started before
this change (fp32 glue); no live paid ticket has been replayed end to end on a chain yet (the
evaluator path is tested on a mock node with real model data).

## Serve the 8B (2026-10-21, "serve the 8B" row)

| suite | where | result |
|---|---|---|
| light client LiteSVM (token/tests/lightclient-litesvm), incl. new `segment_of_a_640_matrix_round_verifies_with_its_job_proven_inline` | Mac, then the test box after the merge with main | 17 + 2 fixture PASS (Mac); box: see below |
| header-relay `cargo test --release` (catchup.rs), incl. new `job_round_is_proven_at_mining_and_its_posting_header_is_held` | test box | 7 PASS before the merge; after: see below |
| cadre-gateway (tests/budget.rs new: 2; tests/gateway.rs 9) | Mac | 11 PASS |
| consensus-pow (new `template_offers_the_oldest_of_700_open_jobs`) | test box | 14 PASS |
| e2e scripts/demo.sh steps 1-6 + 11, synthetic 640-matrix model (scripts/serve8b), DEMO_PORT_OFFSET=1000 | test box ~/serve8b | 7/7 PASS (step 11 2,334.5 s) |

Measured (no estimates): LiteSVM 640-tx posting block: PostSegments 384 bytes, path 264 bytes, begin_verify_job
101,257 CU, segment block 35 txs / 7.34M CU (with a JobCommit: register_job 53,163 CU + 34 txs / 7.24M CU).
Devnet before the change: 434 JobCommit accounts, each 2,240,280 lamports, created by register_job txs of 5,000
lamports fee, 90.9k-103.6k CU, 973-1,006 bytes. E2E: registration 1,280 chunks / 10,536,960 bytes in 102.7 s through
a node at 64 KiB/s and a 20-block grace, 1,280 of 1,280 held 1,427 blocks later; the 640-segment round posted in
blocks 381-426, mined 382-1796, settled 2,208 s after payment; light client: 61,662 txs (WriteProof 41,513), 641
BeginVerifyJob (56,289,474 CU), 0 RegisterJob, 0 JobCommit, 0 failed, 308,350,000 lamports fees, lag max 21.
Found and fixed in the run: block templates scanned the newest 512 jobs only, so 83 of the round's 640 segments
were never offered until the three nodes were restarted with TEMPLATE_JOB_SCAN = 4096 (mid-run, same data).
The test box was resized and rebooted (owner) during the first attempt, which died in step 6; the second
attempt is the one above. A third run on the tree merged with main (security pass L-1/L-2, template scan 4096 from the
start): 7/7 PASS, step 11 1,116.0 s, upload 104.5 s, all 640 segments mined in blocks 340-979 with no stall, settled
1,000 s after payment, 1,280 chunks held 651 blocks after registration, relay 0 failed txs, lag max 20.

## Security pass: services (2026-10-21, "security + robustness pass" row)

| suite | where | result |
|---|---|---|
| pearl-attester (attester 6 incl. `/dev/zero` result_uri, evaluator 8 incl. device result_uri, replay 1) | test box ~/secpass-svc | PASS |
| cadre-gateway (gateway 10, auth_replay 2, auth_audience 2, discover_throttle 1) | test box | PASS |
| pearl-pool (11 incl. pending registrations not persisted, pending cap) | test box | PASS (one old assertion updated: a miner with no accepted share is no longer in the saved state) |
| pearl-job, pearl-miner, pearl-node | test box | PASS |
| pearl-cli `cargo test --locked` with rustls-webpki 0.103.16 / webpki-roots 1.0.10 | test box | 38 PASS |
| storefront-x402 (bn.js 5.2.5) / crank typecheck | Mac | 16 PASS / clean |
| earlier in the pass: explorer 45, cadre-api 56, cadre-proxy 10, pearl-job data_files, gateway auth_replay | Mac | PASS |

Deployed after the tests: test box cadre-api; devnet box 0 cadre-gateway, cadre-attester, cadre-miner, cadre-explorer,
cadre-api (main 154a819); boxes 1-2 cadre-miner (same pearl-miner, sha256 295bfca2...). Public API answered 200 after.

## Ecosystem Phase 2: on-chain DMs (2026-10-21, "ecosystem Phase 2: on-chain DMs" row)

Test box 64.95.13.106 (8 vCPU), own dirs ~/dms-build, ~/dms-merge, ~/dms-api, ~/dms-swarm, ~/dms-replay.

| Suite | Result |
|---|---|
| chain workspace (main + contracts + fix3 + DMs, -j 4) | 184/184 |
| executor tests/dm.rs | 13/13: activation, params bounds, fee rule + size, policy matrix (open, priced, followers, closed) with consent, follows and thread window, consent batches, escrow release by reply, refund timing (deadline inclusive), decline + limits, protocol share, sealed framing + enc_key, channels (paid / invite / open, admins), sealed key epochs + removal forces a rekey, 60-step supply walk |
| agent-node tests/dm_rpc.rs | 2/2: every DM RPC + sendTx admission refusals; reorg rolls back escrow, consent, channel and balances |
| agent-types dm_crypto + vectors | dm.json and dm-crypto.json byte for byte; enc_key never equals the signing key |
| apps/explorer | 60/60 (dm.test.ts reproduces every dm.json tx) |
| services/cadre-api | 59 pass, 0 fail, 9 skipped (route parity included) |
| services/swarm / agents | 41/41 (crypto vectors, HChaCha20 draft vectors, encoders, priority, decline, budgets, DM soak 163 DMs, injection corpus 30) / 20/20 |
| extension tx-vectors | 67/67 (tx, reject, social, contracts, dm) |
| ~/pearl-wallet (bf7f2113e) | unit 280 pass + 6 skipped (2 pre-existing token IDL drift fails), UI 132/132 headless (x3), tsc, eslint, build clean |
| ~/cadre-web (d14cd9d) | unit 56, e2e 63/63 on mocks, 68 screenshots (1440 / 390 px, light / dark): no overflow, no monospace |
| ~/cadre-docs (4ca6501) | check 47 pages x 2 viewports OK |

Live on the test-box DM chain (one agent-node, chain 1347616769, DMs from genesis): cadre-api tools/live-dms.ts
43/43 with real signed txs (heights 1301-1331: plain, priced refused then paid, escrow released, reclaimed after
the window, declined, consent deny / clear, sealed, channel create / invite / joins / key epochs 1 and 2 / rekey
after removal, SSE events); house swarm with a stub gateway: paid plain DM answered and released (1361-1362), sealed
DM answered sealed and opened by the sender (1362-1364), over-budget DM declined with a refund (1407-1408).

Devnet replay: a fresh follower running the new agent-node (5dcab264..., main e7e6cbb) synced the public devnet
from genesis to 4653 with tip 7b8a9c16... and finalized 4623 c9d38411... equal to all three boxes. The devnet
node rollout itself was not run (permission refused in this session; HANDOFF item 29).

## Follower stall + agent-node memory (2026-10-21, "fresh node p2p sync stall" row, round 2)

Cause and fix in HANDOFF item 24. Test box 64.95.13.106, own dir ~/follow-stall.

| Suite / run | Where | Result |
|---|---|---|
| chain workspace (`cargo test --release --locked --workspace`) on branch follow-stall (fix3 source) | test box | 144 passed, 0 failed |
| chain workspace on main with the fix and the contracts merge (0dc64a9), and again with the full-store want check | test box | 164 passed, 0 failed (both) |
| net `normal_lane_is_not_held_behind_bulk_frames` | Mac | passes; with `register_bulk` forced onto the normal lane it fails: "normal lane waited 52.9 s behind bulk frames" |
| net `bulk_queue_bytes_are_bounded_per_peer_and_in_total` (2 GB offered to 3 peers over 1 MB/s links) | Mac | queued bulk bytes stay within 8 MiB per peer / 20 MiB total, links stay up, bytes drain to 0 |
| rpc blobfetch unit tests (window, stale slot, removed while queued) | test box (workspace) | pass |
| live, instrumented pre-fix follower (blobs on) | test box | 77,974 blob "requester timeout" in 2.5 min over exactly 1024 keys; a node stuck on a one-block fork (4515) got gossiped blocks as orphans but no header delivery for minutes |
| live, devnet boxes after the test-box followers stopped (18:35:58 UTC) | devnet | agent-node RSS flat (168 / 76 / 131 MB at 18:39), swap not growing |
| live soak, fix3 followers with blobs on: s1 (from synced data, 18:44-20:15 UTC) and s2 (fresh sync, 19:13-20:15 UTC), watched every 30 s from 19:14 | test box | never more than 3 blocks behind devnet node 0 (last 4663 = 4663), RSS 149-169 MB flat, queue_dropped 0, bulk_dropped 0, bans 0. No fork happened in the window, so the live fork-repair path was not exercised under load (the lane test covers the mechanism) |
| devnet agent-node RSS after the fix (fix3 from about 19:05, then main bc4a6ef from 19:32-19:41, both with the fix), every 5 min to 20:13 | devnet | 178-244 MB on all three boxes, flat over the last 20 min (box 0 208, box 1 235, box 2 239 MB at 20:13) |

## Cadre CLI (2026-10-21, "Cadre CLI" row)

`crates/pearl-cli`: binaries `cadre` and `pearl` (docs/CLI.md "Health and lookups"). Built and tested on the
test box (own dir ~/cadre-cli, `-j 4`), live runs against the public devnet from the test box; one read-only run
on devnet box 0 (binary copied to /tmp, run as lode with `--local`, removed after; no unit touched).

| check | where | result |
|---|---|---|
| `cargo test --release` | test box | 56 pass (13 new in mock.rs / decode.rs / health.rs / inspect.rs, existing tests updated for `overview` and the `Transaction: <kind>` chip) |
| `cadre` (= `status`), public devnet, 20:14 UTC | test box | 27 pass, 4 warn, 0 fail, 3 skip in 1.0 s, exit 0 (full screen below) |
| `cadre status --local` on box 0, 20:00 UTC | devnet box 0 | 25 pass, 6 warn, 0 fail; box-local: gateway (auth v2 + v1), worker glue cadre-glue-v1, attester, swarm rows 15,706 of 16,384, 12 desktops, user-agent runtime; box 0's IP got 429 from the public Solana RPC on programs, mint and the bridge invariant |
| `cadre ping node1 -c 3` | test box | PASS, latency min 10 avg 263 max 531 ms |
| `cadre tx 77e76496...` (a house Post) | test box | Post in words with the thought receipt, FINAL at 4506, explorer link |
| `cadre tx 3XhAW5JY...` (Solana) | test box | `storefront buy_job_amm: bought AI job order #35 for 0.0114 SOL`, 21 inner instructions, OK, slot 513586907 |
| `cadre agent @ferry_nan`, `cadre agents`, `cadre block 4600`, `cadre job ecb7a659`, `cadre model`, `cadre watch --count 2` | test box | all answered from devnet; 32 agent profiles (30 house, 2 user) |
| `cadre verify ecb7a659... --run --models ~/cadre-cli-models` (model copied from ~/cadre-models) | test box | real replay of a devnet ticket: 1 round, 1 request, 48 rows, 144 segments in 12.4 s (GEMM 5.9 s): honest, verdict ACCEPTED |
| `scripts/devnet-health.sh` with `CADRE_CLI` set (scratch LOG_DIR / STATE_DIR) | test box | 37 `cli:` lines in the log; on a box without box 0's services the `--local` checks FAIL as they should |
| `install-miner.sh --cli --no-service` (fresh CADRE_HOME, CADRE_SRC = this tree) | test box | done in 170 s: bin/cadre, bin/pearl, bin/pearl-miner; `cadre ping api` ran; scratch home removed |
| ~/cadre-docs `node scripts/check.mjs` with the new CLI page | Mac | 46 pages x 2 viewports, 1542 links, OK |

`cadre`, public devnet, 2026-10-21 20:14 UTC (non-TTY, so details are not cut):

```
╭────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│                                                                                                                                                                                                                                │
│   [ Chain ]                                                                                                                                                                                                                    │
│                                                                                                                                                                                                                                │
│   CHECK                 STATE   DETAIL                                                                                                                                                                                    MS   │
│   node 64-95-12-116     PASS    height 4669 final 4639 peers 5 mempool 0                                                                                                                                                  78   │
│   node 64-95-13-213     PASS    height 4669 final 4639 peers 5 mempool 0                                                                                                                                                 118   │
│   node 64-95-12-22      PASS    height 4669 final 4639 peers 5 mempool 0                                                                                                                                                  25   │
│   agreement             PASS    finalized block 4639 2d98...312c on 3 of 3 nodes, heights spread 0                                                                                                                             │
│   tip                   PASS    block 4669 64s ago by timestamp, 7a62...a796                                                                                                                                                   │
│                                                                                                                                                                                                                                │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯

╭────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│                                                                                                                                                                                                                                │
│   [ Solana ]                                                                                                                                                                                                                   │
│                                                                                                                                                                                                                                │
│   CHECK                 STATE   DETAIL                                                                                                                                                                                    MS   │
│   solana rpc            PASS    slot 513639795                                                                                                                                                                           127   │
│   program reserve       PASS    8m1R...hVzi deployed, executable                                                                                                                                                         116   │
│   program bridge        PASS    3S22...GFoN deployed, executable                                                                                                                                                         116   │
│   program storefront    PASS    9Xjv...vAKu deployed, executable                                                                                                                                                         116   │
│   program lightclient   PASS    47ma...ie4m deployed, executable                                                                                                                                                         116   │
│   relay payer XQUUCs    PASS    2.6853 SOL (min 1)                                                                                                                                                                             │
│   relay payer E66ekR    PASS    2.7141 SOL (min 1)                                                                                                                                                                             │
│   CADRE mint            PASS    BwJq...Eu5e supply 972,262,263.3533 CADRE (6 decimals, Token-2022)                                                                                                                       116   │
│   light client          PASS    tip 4669 final 4639 chain 4669: 0 behind the tip, final 30 behind (max 120)                                                                                                              326   │
│                                                                                                                                                                                                                                │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯

╭────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│                                                                                                                                                                                                                                │
│   [ Services ]                                                                                                                                                                                                                 │
│                                                                                                                                                                                                                                │
│   CHECK                 STATE   DETAIL                                                                                                                                                                                    MS   │
│   cadre-api             WARN    not ready: solana: https://api.devnet.solana.com: HTTP 429; gateway up, storefront up                                                                                                     81   │
│   explorer              PASS    indexed 4669 of 4669 blocks                                                                                                                                                               30   │
│   storefront-x402       PASS    wallet 9kwg...A5bP on solana-devnet                                                                                                                                                       60   │
│   relayer               PASS    healthy; deposit cursor 37, withdrawal cursor 9                                                                                                                                          249   │
│   gateway               PASS    serves llama-3.2-1b-instruct-w7a7; 79 requests, 16 rounds posted, decode 1.55 tok/s (gateway's own measure)                                                                               88   │
│   site                  PASS    HTTP 200 (54 KB); /install-miner.sh 200                                                                                                                                                   92   │
│   docs site             SKIP    no public docs site for this network (set CADRE_DOCS)                                                                                                                                          │
│   pool                  SKIP    this network runs no mining pool                                                                                                                                                               │
│   settle                PASS    2 pending orders, none waiting on a closed job                                                                                                                                           108   │
│   verifiable AI         PASS    newest completed ticket ecb7...49e0 (closed at 4560) carries the cadre-glue-v1 replay bundle (anyone can replay it: cadre verify)                                                        101   │
│                                                                                                                                                                                                                                │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯

╭───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│                                                                                                                                                                                                                               │
│   [ Agents ]                                                                                                                                                                                                                  │
│                                                                                                                                                                                                                               │
│   CHECK                 STATE   DETAIL                                                                                                                                                                                   MS   │
│   house swarm           PASS    newest house post @chef_ottoline at 4662 (10m ago); 13 of the newest 19 town posts are house posts, 13 with a verified thought; today 13 posts, 4,741 paid rows behind them; 0 pending   84   │
│   house desktops        PASS    12 desktops running for 6 agents; spend today 3.85 of 25 USD cap                                                                                                                         90   │
│   user-agent runtime    PASS    cadre-agents 0.1.0 answers; 1 hosted agents, 1 running                                                                                                                                   89   │
│                                                                                                                                                                                                                               │
╰───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯

╭────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│                                                                                                                                                                                                                                │
│   [ Economy ]                                                                                                                                                                                                                  │
│                                                                                                                                                                                                                                │
│   CHECK                 STATE   DETAIL                                                                                                                                                                                    MS   │
│   buyback               WARN    not in the cadre-api summary now (it reads Solana; status: https://api.devnet.solana.com: HTTP 429)                                                                                            │
│   reserve               WARN    not in the cadre-api summary now (it reads Solana; status: https://api.devnet.solana.com: HTTP 429)                                                                                            │
│   supply                WARN    not in the cadre-api summary now (it reads Solana; status: https://api.devnet.solana.com: HTTP 429)                                                                                            │
│   orders                PASS    22 storefront orders (finalized 14, pending 2, refunded 6), 0.2669 SOL settled, 0.0205 SOL pending                                                                                             │
│   mining                PASS    73.5 blocks/h over the last 60 (60 s target = 60/h); paid-job blocks 60 of 60 (100%); all time 708 of 4669 (15.2%)                                                                       472   │
│   bridge                PASS    OK: vault 38,709,110.122 CADRE = outside escrow 38,709,110.122 CADRE; 31 deposits unlocked on the chain, 9 withdrawals (9 released on Solana)                                            448   │
│                                                                                                                                                                                                                                │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯

╭───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│                                                                                                                                                                                                                               │
│   [ Box ]                                                                                                                                                                                                                     │
│                                                                                                                                                                                                                               │
│   CHECK                 STATE   DETAIL                                                                                                                                                                                   MS   │
│   box-local services    SKIP    gateway, worker, attester, swarm, desktops, user-agent runtime: run on box 0 with --local                                                                                                     │
│                                                                                                                                                                                                                               │
╰───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯

27 pass · 4 warn · 0 fail · 3 skip in 1.0 s · devnet
warn cadre-api, warn buyback, warn reserve, warn supply
result WARN
```

## Smart contracts (2026-10-21, "smart contracts" row; docs/CONTRACTS.md 11)

- Chain workspace on the test box with contracts merged into main: 159 passed, 0 failed (before the last
  main merges: 131, then 134). New suites: contracts/tests/vm.rs (5 + 1 ignored measurement: examples
  validate, deterministic gas, query mode read-only, 11 malicious modules refused at deploy, runtime
  attacks stop deterministically), executor/tests/contracts.rs (10: fees and refunds, failures land with
  state undone, cross-contract calls, reentrancy refused, call depth, storage deposits, escrow/token/bounty
  flows incl. a native job and a town post by a contract, block gas limit and fee floor, migrate/admin,
  devnet activation height, determinism digest, gas table), state/tests/contract_proof.rs (storage proof
  against state_root), bin/agent-node/tests/contracts_rpc.rs (3 node actors agree, reorg undoes a call,
  final storage proof), agent-types vectors contracts.json.
- pearl-verify-sbf: tests/contracts.rs 1 (record, event, receipt under tx_root; tampering fails) + crosscheck 13.
- pearl-cli: 39 passed (incl. the ABI vector). cadre-api: contract + spec + neighbouring suites 40/40 on the
  merged tree; explorer 56/56. Extension 93 passed (contracts 23, tx-vectors 43); Cadre Wallet 229 passed,
  UI checks 50/50 and 110/110 headless (pre-existing IDL drift and token/target failures unrelated).
- Determinism across machines: the executor digest scenario printed the same state digest on the Mac
  (arm64) and the test box (x86_64), 6b70aa8961335901a420fc65ed795062a0fbe2719ab64100d2f725accb31b435.
  The counter example built from source on the test box has the same code hash as the Mac build
  (bb7e5a908dabc16f...).
- Live CLI check on a throwaway 1-node chain on the test box (ports 11750-11752, stopped after):
  build from source, new, call, dry-run, query, show, events, token, ls all OK.
- Replay: a fresh follower with the new agent-node synced the devnet from genesis (block 4587 hash and
  state_root equal to all three boxes) before the rollout.
- Devnet: first contract block 4703; 16 contract txs in 4703-4719 (4 DeployCode, 4 Instantiate, 8 Call), all status ok; the three nodes agreed on
  tip and finalized hash throughout (docs/DEVNET.md "Contracts").
- Not run: header-relay catchup tests (they need the lightclient .so built under token/, absent in the
  build tree); a QMDB storage-proof verifier on Solana does not exist yet.

## Agents trade on chain (2026-10-21, docs/TRADING.md 8)

| Suite | Where | Result |
|---|---|---|
| mandate program LiteSVM (token/tests/mandate, real pump.fun / PumpSwap bytecode, exploit set) | Mac, against the test box build (litesvm aborts on the box) | 11/11 |
| trading contracts in the executor (contracts/tests) | test box | 3/3 |
| trading runtime (services/swarm/trading) | test box | 6/6 |
| cadre-api trading routes (test/trading.test.ts) + suite | Mac | 4/4; suite 56 pass, client/sol fail on the untouched tree too (missing dev dep in a fresh worktree) |
| pearl-cli (with `cadre trades`) | test box | 75 + 4 pass |
| pearl-wallet mandate-decode.spec | Mac | 3/3; pearl unit 209 (2 drift.spec failures predate this) |
| cadre-web lint, typecheck, build, headless screens (desktop + phone, no horizontal scroll, no monospace) | Mac | clean |
| live devnet | test box -> public endpoints | 22 fills on both chains by 3 house traders, receipts on Cadre, 4 unsigned mandate builders simulated |


## Trading for everyone: sovereign agents on Solana (2026-10-21, Phase 4b, TRADING.md 5.6)

| Suite | Where | Result |
|---|---|---|
| services/swarm/agents (spec v2 vector + bounds + agent_self; runtime incl. sovereign refusals of spec, pause, resume, withdraw, export, delete) | test box | 25/25 |
| services/swarm/trading (incl. useragent.test.ts: spec to RiskRules, conviction gates, runner budget, kill file and halt, own-wallet fills, average cost) | test box | 11/11 |
| services/swarm (house swarm, unchanged) | test box | 42/42 |
| services/cadre-api (incl. usertrading.test.ts: walletFill, user-agent trades / agent view / leaderboard / organic volume, useragents sovereignty fields) | test box | 60 pass; client.test.ts and sol.test.ts fail the same way on the untouched tree (missing dev dependency) |
| pearl-cli (Trade view, quote/positions/buy/sell) | test box | 92 unit + 5 entry_cli |
| cadre-web lint, typecheck, unit, build; headless Chromium at 1440 and 390 px, mocks and live devnet via the branch cadre-api | test box | 62 unit; screens 60/60 mock, 60/60 live (screenshots/usertrade/) |
| pearl-wallet trade-decode.spec (real devnet builder txs, receipts, v2 spec message) + unit suite | test box | 10/10; 304 pass, 6 skipped |
| live devnet | test box -> public endpoints | sovereign agent tide_ledger: 4 own-wallet PumpSwap buys (first 66bxetLg...), 4 receipts on Cadre (heights 4869-4873), PnL and leaderboard from chain via the branch cadre-api; a person's `cadre buy --sol 0.002` (5jUAxDes...) |

Not verified: a real wallet signature through the site wizard (the headless stand-in wallet only
connects); the hosted path of a v2 agent on box 0 (cadre-agents there runs the old code until the
deploy); the agent's stance loop stopped placing orders when the devnet gateway answered 502 (the
engine halts while the agent is not running, as designed).


## Ecosystem Phase 3: agent services market (2026-10-22, docs/MARKET.md 10)

| Suite | Where | Result |
|---|---|---|
| market contract in the real executor (contracts/tests/tests/market.rs: accept payout, fee floor incl. rounding and admin edits, reject refund + judge pay, timeout open and delivered, decline, self-dealing and evaluator rules, 2-of-3 split, queries; supply invariant every block, escrow 0 after every close) | test box | 8/8 (trading contracts still 3/3); wasm rebuilt after merging main: same sha256 29958eff... as the deployed code |
| pearl-evaluator LLM-judge mode (services/attester judge.rs) | test box | 5/5 unit; attester crate green |
| cadre-api services routes (test/services.test.ts) + api parity, trading, contracts, town | Mac | 7/7; 38/38 with those suites; client.test.ts and sol.test.ts fail the same way on the untouched tree (missing @solana/web3.js on the Mac) |
| user agents (services/swarm/agents incl. market.test.ts) | Mac | 28/28 after merging main (spec v2) |
| house market desk (services/swarm/market/test) | Mac | 3/3 |
| pearl-wallet market-decode.spec + pearl unit | Mac | 12/12; 231 pass, 6 skipped, 2 drift.spec IDL failures predate this; headless UI check 135/136 (the failure is the pump.fun buy wording from trade-decode, not the market) |
| cadre-web typecheck, lint, unit, build; headless Chromium 1440x900 and 390x844, light and dark, against a cadre-api with the market routes on real devnet state | Mac | 72 unit; no horizontal overflow, no monospace, no em dashes, no page errors (screenshots/market/ in cadre-web) |
| pearl-cli (market.rs, Market view 9 next to main's Trade view 8; node fallback against a mock node; write path on a private one-node chain) | test box | 103 unit + 5 integration after the merge |
| cadre-docs build + link check | Mac | 53 pages x 2 viewports, 1905 links OK |

Live on Cadre devnet (market contract E1kpbaHn14z4Amdqb7SGjLbn576q1KPC3haAG6YQdsym, DeployCode 4845
ed9b6cdd..., Instantiate 4846 fd81934d...; four house offers at 4925-4928):

| Hire | Path | Timeline | Result |
|---|---|---|---|
| #0 scribe_wren hires tally_crow "Five name ideas", 3 + 0.6 fee + 0.5 judge fee | house desk | Hired 4929 (bc567af4...), Delivered 4937 (52ed24e0..., thought req_abd90f1a..., 133 rows), Verdict + Rejected 4940 (8bf92a9c...) | the LLM judge rejected ("generic and vague descriptions"); price + fee refunded, judge paid 0.5 |
| #1 ledger_lark hires scribe_wren "Plain-words explainer", 4 + 0.8 + 0.5 | house desk | Hired 4954 (28911d0d...), Delivered 4962 (78339aa3..., 251 rows), Verdict + Completed 4967 (852828a2...) | accepted by the judge; 4 CADRE to the provider, 0.8 to fee_to, 0.5 to the judge |
| #2 market_ops hires verse_finch "Four-line verse", 2 + 0.4 | cadre-api POST /v1/tx/market (the site's builder), signed with the key file, buyer as evaluator | Hired 4968 (ced9e90e...), Delivered 4971 (992bd860...), manual Verdict + Completed 4972 (6108c071...) | completed; review text checked |

Thought receipts checked against the devnet gateway (`/v1/openai/requests/{id}`): the judge's hire #0
answer hashes to its receipt (text_sha256 6d4adc52..., 336 rows, 4,133,472 lamports, paid by the
judge's own wallet F29KnY...), and tally_crow's delivery hashes to its receipt (ae0ab7bd..., 133
rows, 1,636,166 lamports). Market volume after the run: organic 0, house 6 CADRE (correct: every
buyer is house). Each desk and judge wallet topped up its own prepaid balance over x402: 2,048 rows
for 25,193,988 lamports per top-up.

Found and fixed during the live run: house-to-house hires were labelled self-dealt (the shared house
operator) instead of house; desk blobs lived only on box 0's node (now mirrored to all three); the
judge paid for a second thought on a hire whose verdict tx had not landed yet (now skipped for 30
blocks); review text named the contract in base58 (now Cx).

Not verified: a real Cadre Wallet signature through the site (headless runs used a stand-in
window.pearl; the builder path was proven by signing its unsigned_tx with a key file); an organic
(non-house) hire on devnet (none was made, so organic volume is truthfully 0); the public devnet
cadre-api and site serve the market only after the deploy in MARKET.md 11.

## Ecosystem Phase 5: boards, discovery, trader board, listings, bot feed (2026-10-22, HANDOFF 36)

| Suite | Where | Result |
|---|---|---|
| cadre-api boards (test/boards.test.ts): board math on synthetic facts (self-dealt never counts, house apart, distinct payers, minimum sample and payers gate, windows on block time, house / organic / all sides, rate in ppm, positive boards skip zero rows, realized PnL per sell sums to the trading route's average cost) + every route on an in-memory index with fake market and trading handles (earnings with self-dealt and funded payers excluded, acceptance 2 of 3 with the min-sample cut, tips with the self tip excluded and the 10-day-old tip only in `all`, paid followers vs raw, answered vs unanswered paid DMs, trading 3-sell minimum, self-mined block and own-job verdicts excluded, discovery filters, trader board metrics, event feed JSON / RSS / SSE, no em dashes) | Mac + test box | 20/20 |
| cadre-api full suite incl. town (new leaderboards test: every board live, house rows only on the house side), openapi parity (every new route documented) | test box | 103 tests: 94 pass, 0 fail, 9 skipped (client and sol need storefront-x402 node_modules: installed in the box copy) |
| feed-bot (dry run by default, live only with all three switches, first run takes no backlog, hourly cap, cursor survives a restart, 280-character texts keep the link) | Mac | 4/4 |
| DefiLlama adapter unit (storefront events, window, paging, mandate Settled offsets, mandate fees in the right fields) | test box | 4/4 |
| DefiLlama live, all-time, Cadre devnet (storefront 9XjvGk2x..., mandate BG5WkxZy..., dedicated devnet RPC) | test box | PASS: fees 599,081,615 = total_sol 539,173,466 + shares held 59,908,149; supply side = total_sol; holders revenue 29,657,293 = bb_accrued = cadre-api /v1/buyback accrued; mandate protocol fees 0 = the Mandate accounts' fees_protocol (no settle has run on devnet) |
| cadre-web /leaderboards (+ /[board]), /discover, /board: tsc, lint, unit, next build; e2e/boards.mjs headless Chromium on the test box at 1440 and 390 widths against the branch cadre-api on real devnet data (no horizontal overflow, no monospace, no em dashes, every board shows rows or its empty reason) | test box | 78 unit; 12/12 screens (screenshots/boards/); cadre-web b6b9fc7 |
| cadre-docs guides/boards build + check | test box | 54 pages x 2 viewports, 1,952 links OK |
| pearl-cli Boards view 10 + `cadre top` (formatting per unit, the 9 boards parsed from a live fixture, window / side refusal, view 10 on 0 / alt+0 / F10, canvas at 80 and 140 columns, no em dashes; `top` text and JSON against a fixture server) | test box | 109 unit + 7 integration |

Live on the test box against the public Cadre devnet (branch cadre-api 127.0.0.1:10790, devnet node RPC,
devnet Solana RPC), 2026-10-22 ~03:00 UTC: all nine boards answer; examples (window all): market_earnings
1 Cx4956...6f64 9,148,670.61 CADRE organic from 3 payers (+661,671.35 CADRE house), 2 Cxe21a...FF6C
5,201,854.77 CADRE from 3 payers; trading_pnl 1 @tide_ledger 57,920 lamports over 3 sells; miners 1
Cxe21a...FF6C 470 paid of 1,901 blocks; evaluators 1 Cx7DcD... 13 verdicts (+2 house, 1 own-job excluded);
hardest_thinkers organic empty, house side 19 rows (25 receipts read; every thought was paid by the house
wallet Cu4bS9...); paid_dms, most_tipped, most_followed empty (no tips, follows or paid DMs on devnet yet:
shown as empty, never filled). Trader board: SOL spent on AI all time 272,542,596 lamports organic,
11,442,615 house; paid-block share all time 214,711 ppm. Event feed: 58 events; the feed bot's dry run
printed 6 (new agents, a paid-blocks milestone, two completed house hires, block 5,000) and sent nothing.

## Batched inference worker + gateway backpressure + request data retention (2026-10-21/22, HANDOFF 35)

| Suite | Where | Result |
|---|---|---|
| tools/cadre/test_batch.py, tiny synthetic Llama (7 mixed requests, pool off and on) | test box; A40 pod CPU | 17/17 (console only; testbox-tiny.log is the earlier pre-pool run, 9/9); 17/17 (a40-tb-tiny.log) |
| test_batch.py, Llama-3.2-1B-Instruct w7a7, 9 mixed requests, batched vs alone (ids, text, rows, 288 files, logits rows), pool off and on, + cadre-replay local of every unpadded batched request | test box CPU (4 threads) | 28/28 (testbox-real2.log; replay 7/7 honest, 7.4-18.8 s each) |
| same | A40 (cuda) | 28/28 twice (a40-tb-gpu3.log, a40-tb-gpu4.log; replay 7/7 honest) |
| CPU vs GPU, same pod, same model files: all A/C files of the 9 batched requests | A40 pod | 9/9 identical |
| tools/cadre/test_cadre.py (unchanged serial path) | test box | 9/9 |
| services/inference-gateway (new: backpressure 2, retention 3) | test box | 27/27 |
| same | Mac | 26/27: server_limits a_silent_client_is_cut_off_by_the_read_timeout fails on macOS, also on unmodified main |
| throughput, bench_batch.sh (MAINNET 7) | A40; test box 4 and 2 threads | a40-bench-gpu*.jsonl, testbox-bench-cpu-t4-v2.jsonl, testbox-bench-cpu-t2-v2.jsonl |
| worker peak RSS for box 0 settings | test box, 2 threads | testbox-rss2.txt (3.00 GB at the unit's settings) |

Found and fixed on the way: ThreadingHTTPServer's listen backlog of 5 reset clients at 64 concurrent
(now 256); per-request capture clones and per-request attention made the GPU launch-bound at 45 tok/s
(per-step capture + pooled decode attention: 140); a KV pool of 128 x 2048 positions does not fit an
A40 (51 GB; --pool-positions). Not verified: the batched worker on box 0 itself (rollout handed to the
orchestrator), a paid gateway round end to end with the new retention on a live chain (tested against a
mock chain that includes every tx).


## Blob data lifecycle (2026-10-22, "blob lifecycle" row, HANDOFF 37)

Test box 64.95.13.106, ~/bloblife, `-j 4`, main a72713b.

| Suite | Result |
|---|---|
| chain workspace `cargo test --release` | 196 passed, 0 failed, 2 ignored |
| new: agent-node tests/blob_lifecycle.rs | 3/3: segment A retention by state (mined, final, ticket closed, replay window, mining window) on a replay node and a mining-only node + model blobs pinned + getBlobStats classes; stats breakdown + putBlob "blob store nearly full" backpressure and recovery after the prune; reorg around a mined segment (A kept, mineable again on the new branch), reorg around a pruned one (not wanted back) and a fresh node syncing after pruning (equal finalized hash, pinned manifest fetched, A of a mineable segment fetched, pruned A never wanted) |
| new: executor tests/blobparams.rs | 2/2: key 48 refused below devnet 5800 ("unknown config key") and accepted from it, config record untouched; admin only, range, reserved key 49, default value deletes the record |
| blobs + state units | rpc_reserve_backpressure, blobparams roundtrip/bounds, all green |
| cadre-gateway (crates workspace) | 30/30; new tests/node_blobs.rs 3/3: 503 insufficient_storage before reserving when the node lacks headroom (waiting rows count), a settle refused for space blocks new paid work at once, an older node's quota minus total and no getBlobStats at all |
| pearl-attester | 20/20; tests/replay.rs with the real model (CADRE_REPLAY_MODEL llama-3.2-1b-instruct-w7a7-k2048, runs ~/verify-ai/rt/req0,req1): honest 96-row 144-segment ticket Accept in 12.1 s (commitment-first A check), altered A rejected naming segment 4df38dcc... (matrix 61, row 90 col 7), wrong C rejected (product) |
| live follower | staged agent-node 556739d3... from genesis against the devnet seeds, keep_replay false, 600 MB quota: tip 5077 and finalized hash 4038f7df25c530a1 equal to box 0; fetched only mineable segments' A (316 blobs, 432 MB) + manifest + profiles + ticket blobs; stopped by PID, data removed |
## Cadre open items: fee to the pool, agent launches, halt record (2026-10-22, HANDOFF 38)

All on the test box 64.95.13.106 (~/cadre-launch) unless noted.

| Suite | Result |
|---|---|
| contracts/tests (`cargo test --release`): pool.rs 6 (new), halt.rs 3 (new), market.rs 8, trading.rs 3 | 20/20 pass |
| services/cadre-api (`node --test`) incl. launches.test.ts 8 (new) | 91 tests: 82 pass, 0 fail, 9 skipped (services/crank node_modules and token/target/idl missing in this tree, as before) |
| services/swarm/agents incl. launch.test.ts 6 (new: launch skill, halt watcher, spec launch rules) | 34/34 pass (also on the Mac) |
| services/swarm/trading incl. migrate-vector.test.ts 1 (new), useragent.test.ts +2 (own-token self-trade, curve reserves) | 14/14 pass |

Measured gas (executor tests, gas_price 10,000): pay_mining_pool probe call 328,019 (fee 6,131 base
units); market verdict that completes and pays the fee to the pool 1,005,503 (fee 14,106) against
922,182 (fee 13,272) for an account fee_to; market v2 deploy 30,897 bytes 1,235,880 gas (fee 323,629);
halt deploy 8,101 bytes 324,040 gas; halt set 208,993 (fee 5,320).

Live on devnet: market migrated and fee_to = pool (5040-5042), hire #10 completed with the fee to the
pool (5106); halt contract deployed (5043-5045), set all (5046) and cleared (5047) with a self-hosted
runner reporting `halted` then going on; Solana devnet test launch HzqW62Pm... (1,177 bytes, 178,850 CU)
plus a curve buy and sell by the same key. The first builder attempt with a compute-budget instruction
was 1,254 bytes and refused by the RPC (limit 1,232); the builder now drops that instruction and checks
the size.


## Chain speed: weak-block pre-confirmations + 30 s target (2026-10-21/22, HANDOFF 39, docs/SPEED.md)

All Rust on the test box (-j 4, release), merged tree (branch speed at 49c6700 + test fixes):

| suite | result |
|---|---|
| chain workspace (`cargo test --workspace --no-fail-fast`) | 228 pass, 0 fail, 58 suites |
| crates: pearl-job, pearl-node, pearl-miner | 74 pass |
| pearl-cli | 119 pass |
| token/tests/lightclient-litesvm (incl. new speed.rs 5: realloc/admin, 60 s at S and 30 s at S+1, K floor, reorg across S, CU) | 26 pass |
| services/header-relay | 8 pass |
| apps/explorer | 65 pass |
| services/cadre-api | 98 pass, 2 fail (client.test.ts and sol.test.ts: `@solana/web3.js` not installed in that tree; the same on main) |
| extension | 121 pass (solana.test.ts needs the token IDL build) |
| ~/pearl-wallet | 284 pass (2 drift.spec IDL failures predate this) |
| trading preconf gate | 4 pass |

New tests: consensus-pow/tests/weak.rs (codec, accept/reject, real Pearl weak-only proof is not a block), rpc preconf
units (count, share, confidence, pruning, cap, replacement refusal), agent-node tests/weak_network.rs (3 nodes with
latency: preconfirmed on other nodes, then included, then final; 40-byte tips still work; garbage weak proof gets the
peer blocked), executor speed tests (migration once at S and only on migrating chains, emission per day 1.00294%
before and after, uncle reward math per depth, cap bound, lien split, key 45 activation), chain-level uncle tests
(each rule refused incl. uncle of an uncle and double inclusion, reorg makes an uncle eligible again, 30 s from S+1,
finality pause at S), body codec, a 4-node network on 300 ms links (6 of 6 racing rounds credited the stale block
as an uncle on every node), a fresh node syncing through the speed height.

Live checks: a fresh node with the staged binary 8b4b3f74... synced the public devnet from genesis on the test box (2026-10-22 13:41 UTC: height 5701, finalized 5671 = da8aa41e9ed0c174..., the same hash as node 1 at 5671; 0 invalid or mismatch log lines; stopped by PID, data removed); the Phase 2
fork's earlier build matched node 0 at 1, 3260, 4200, 4700, 4799, 4800, 4801, 5000 and the finalized hash at 5072.
Measurements: docs/SPEED.md 7.

Trading hook (for the trading row; not applied because services/swarm/trading was not on this branch when the gate
was written): in `services/swarm/trading/src/cadre.ts` import `{ waitSettled, policyFromEnv }` from `./preconf.ts`
and add `waitSettled(txHash, notional, p = policyFromEnv()) { return waitSettled(this.rpc, txHash, notional, p); }`
to the Cadre class; in `src/trader.ts`, after `const f = await this.o.venue.trade(...)` on the Cadre venue, `const
settled = await this.o.cadre.waitSettled(f.ref, order.amountIn);`, log `settled.status` / `settled.risk` and append
`settled.risk` to the receipt body. Policy defaults (not measured, a choice): act on preconfirmed only at confidence
>= medium and notional <= TRADE_PRECONF_CAP (10 CADRE), wait for included up to TRADE_INCLUDED_CAP (1,000 CADRE),
final above.

Explorer note: `decodeLcConfig` shows the light client's base `finalityK` (30); after 6000 the schedule at offset 318
holds 60 (display only).
