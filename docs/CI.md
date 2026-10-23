# CI

`.github/workflows/ci.yml` runs on every push to main and every pull request. It is offline by
design: no secrets, no devnet or test box, no deploys, nothing signed with a real key. It uses
`pull_request`, never `pull_request_target`, so a fork's PR runs with a read-only token and no
secrets, and checkout does not persist the token.

| Job | What it runs |
|---|---|
| lint | `node scripts/ci/lint.mjs`: no em dash (U+2014) in any tracked text file; packages/cadre-py unit tests (`python3 -m unittest`, stdlib only) |
| programs | Agave 3.1.12 + Anchor 0.31.1 release binaries, `anchor build` in token/ (program ids from keys/localnet), then `cargo test` in token/tests/bridge-litesvm, token/tests/lightclient-litesvm and services/header-relay (its catch-up tests load lightclient.so). Uploads token/target/{deploy,idl} for the node job |
| node | builds the `pearl-job` CLI, then `scripts/ci/node.sh`: `npm ci` per package, every `npm test`, the `typecheck` scripts (cadre-api, crank, relayer, storefront-x402, extension) and the extension build |
| rust (matrix) | `cargo test` in chain/ (Rust 1.95), crates/ (pearl-job, pearl-chain, pearl-node, cadre-glue, miner, attester, pool, inference-gateway), crates/cadre-cli, crates/pearl-verify-sbf, contracts/ (wasm build, then contracts/tests in the real executor, plus crates/cadre-contract-sdk) |

To run the same thing locally: `cd token && anchor build` (copy keys/localnet/<p>-keypair.json
into token/target/deploy first), build `pearl-job` (`cargo build --release -p pearl-job` in
crates/), then `scripts/ci/node.sh` and the cargo commands above.

Packages are not an npm workspace: each package with a package-lock.json is installed on its own.
Tests that compare against a sibling package (cadre-api against services/crank and
services/storefront-x402; the extension's job-parity tests against the `pearl-job` binary) skip
with a reason when that sibling is missing locally, and fail under `CI=true` so a broken
pipeline cannot pass by skipping.

## Not in CI, and why

- **Superseded SVM agent chain**: agent/ (Anchor programs), agent/tests/escrow-litesvm,
  agent/tests/mining-litesvm, crates/chain-litesvm, services/agent-cli. Kept only as reference
  (CLAUDE.md); they still build locally.
- **Live and devnet runs**: scripts/demo.sh, scripts/devnet-*.sh, scripts/devnet-e2e,
  services/cadre-api `npm run live`, tools/defillama `live`, extension `test:ui` (headless
  Chromium against a running stack), apps/explorer `screens`. They need validators, the test box
  or devnet.
- **Benchmarks marked `#[ignore]`**: miner `bench_rates`, chain `measure` and
  `measure_merkleize_writes`. No correctness assertion; run them with `-- --ignored`.
- **GPU tooling**: tools/cadre (test_cadre.py, test_batch.py, test_replay.py need torch and numpy, and the real paths a GPU) and scripts/gpu.
- **Release SBF builds for deploy and the light client's `cargo build-sbf` size checks**: the
  programs job builds what the tests load; deploy builds stay manual.
- **research/**: notes and a gitignored upstream clone.
- On macOS (not CI) two inference-gateway read-timeout tests are ignored: macOS does not copy the
  listener's SO_RCVTIMEO to accepted sockets. They run on Linux in CI.
