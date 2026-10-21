# Security review before mainnet (2026-10-21)

Owner request 2026-10-21: "strengthen everything that already exists". This is the
adversarial review of what is built today, with the fixes that landed and the items
still open. Each fix has a regression test that fails on the old code unless noted.
Exploit details of anything still open live only in the gitignored
`docs/security-private/`; this file names the area and the fix, not the attack.

Areas and who reviewed them (all forks of the security row in CLAUDE.md):

| area | scope | status |
|---|---|---|
| 1. Programs: reserve, storefront | token/programs/{reserve,storefront}, LiteSVM exploit tests | fixed, see below |
| 2. Programs: bridge, lightclient | token/programs/{bridge,lightclient}, relayer callers | fixed, see below |
| 3. Services | relayer, attester, header-relay, storefront-x402, cadre-gateway, cadre-api, explorer, crank, pool, miner, cadre-proxy | this file, section 3 |
| 4. Cadre Wallet | ~/pearl-wallet dev | fixed in pearl-wallet (d3b3f05d0) |
| 5. Ops | devnet boxes 0-2 | d8bf302 |
| 6. Dependencies | cargo audit, npm audit | section 6 |
| 7. Chain hardening | chain/ outside sync, net and the contracts VM | fixed, 0ecf5bf + 9277119 (merge 060dfd5) |

## 1. Reserve and storefront programs (d60c9d4)

- **R-1** reserve: `update_config` can no longer swap the mint after launch (`MintFrozen`).
- **R-2 / S-3** reserve, storefront: an open buyback crank must spend the full
  min(available, per-call cap), so third parties cannot throttle buybacks with
  minimum-size cranks (`PartialSpend`). An allowlisted cranker may still pass less.
- **S-1** storefront: a share record PDA pre-funded by anyone no longer blocks `buy_job`.
- **S-2** storefront: mint and bridge program freeze at the first order, the reserve
  program at the first buyback routed to it (`DestinationFrozen`).

Error codes appended, no account layout change. Reserve 20/20, storefront 26/26.

## 2. Bridge and light client (b4ce4db, 22c739f)

- **B-1** bridge: `refund_deposit` now needs the deposit's Order and refunds only an
  order deposit, only to that order's buyer (`NotAnOrder`).
- **B-2** bridge: the refund message is v2 and names the bridge program id; v1
  signatures are refused. The bridge upgrade and the relayer must roll out together.
- **L-1** lightclient: `replace_job` re-registers a job from a canonical block once a
  witness header shows that the block which registered it first is dead.
- **L-2** lightclient: `mark_final` accepts a final child as the link after a tip jump
  past the 256-entry ring; non-final headers outside the ring cannot be closed early.

Relay side (5deedec): header-relay `fill_final_gap` and `replace_dead_job`.
Bridge 15/15, storefront 26/26, reserve 20/20, relayer 33/33, lightclient LiteSVM 20/20,
header-relay catch-up 7/7, pearl-verify-sbf 13/13.

Open (low, details private): the signer-path withdraw message names no deployment (only
matters if the signer path is turned on; proof-only mode refuses `release`); bridge and
lightclient `initialize` can be front-run between deploy and init (mainnet: pin the init
key and check right after init); relay reward and header rent go to the submitter
(`relay_reward` is 0 today).

## 3. Services

| id | severity | where | issue | fix | commit |
|---|---|---|---|---|---|
| V-1 | medium | apps/explorer agentcard.ts | SSRF guard in the agent card probe missed IPv6 forms of private IPv4 addresses (mapped, compatible, NAT64, 6to4) and re-resolved the host at fetch time | IPv6 parsed to bytes and checked by any embedded IPv4, more reserved ranges, no URI credentials, private answers refused at connect time | e23ca64 |
| V-2 | high | crates/pearl-job data.rs (miner, pool, attester) | a chain-sourced `data_uri` / `result_uri` naming a device, FIFO or huge file made miners read until killed | regular file, exact size (manifest at most 64 KiB), bounded read | 9cd45aa |
| V-2b | high | services/attester evaluator.rs `fetch_result`, lib.rs `read_result` | the same unbounded read in the attester | metadata check, `read_exact_file`; a wrong kind or size is the deliverer's fault (reject), a missing file holds | see section 7 |
| V-3 | medium | inference-gateway auth.rs | the replay guard lived in memory, so a request signed just before a gateway restart could be replayed after it | signatures older than the process are refused; set-based lookups | e976e3f |
| V-4 | low/medium | cadre-api routes/market.ts | tx builders accepted 50% slippage and any min-out override | slippage capped at `CADRE_MAX_SLIPPAGE_BPS` (default 1000), looser min-out answers 422 | 88ad2de |
| V-5 | medium | apps/explorer profile-view.ts | `RegisterAgent.uri` rendered as a link with any scheme (stored XSS on click) | links only http(s) | 52247dd |
| V-6 | high (proxy users) | services/cadre-proxy | on loopback without `--token`, any web page could spend the proxy wallet | foreign Origin and non-loopback Host refused (`CADRE_PROXY_ALLOW_ORIGINS`) | ec2c80f |
| V-7 | medium (DoS) | inference-gateway main.rs | one thread per connection, no cap, no read timeout; bodies over 4 MiB silently truncated | **open**: the serve-8B row (owner of main.rs) finished without it. Fix: connection cap (semaphore ~64, 503 above), per-connection read timeout, 413 above 4 MiB. Today the gateway binds 127.0.0.1 behind cadre-api's rate limit | |
| V-8 | low | inference-gateway lib.rs | anonymous `GET /v1/prepaid/{w}` and `/v1/balance?wallet=` each fanned out into node calls | anonymous reads rescan a wallet at most once per 15 s (cache capped at 10,000 wallets); signed reads always rescan | see section 7 |
| V-9 | low | inference-gateway auth.rs | cadre-auth-v1 names no gateway, so a request signed for one gateway could be replayed to another inside the 120 s window | **cadre-auth-v2**: `"cadre-auth-v2\n" + AUDIENCE + "\n" + METHOD + "\n" + PATH + "\n" + TS + "\n" + hex(sha256(body))`, AUDIENCE = the gateway's server key in base58 (`GET /health` -> `server`, `auth` lists the accepted versions). v1 stays accepted until `CADRE_AUTH_REQUIRE_AUDIENCE=1` | see section 7 |
| V-10 | low (DoS) | services/pool lib.rs | unauthenticated registration rewrote the whole state file each time and consumed nonce ranges; every rejected share also rewrote it | unproven registrations stay in memory (at most 4096, dropped after 30 min idle, ids reused), saved with the first accepted share; rejected shares no longer write | see section 7 |
| V-11 | info | services/relayer config.ts | `COMMITMENT` defaults to `confirmed` | not changed; mainnet units must set `finalized` (the devnet unit does) | |

Checked and found OK: key files on box 0 (0600 in 0700 dirs), key writers in the code
(0600), RPC URLs redacted in logs, strict x402 verification (service key only as fee
payer, one transfer, no lookup tables), cadre-api rate limiting and 1 MiB body cap,
storefront-x402 64 KiB body cap, gateway API keys stored as sha256 of 24 random bytes.

Not reviewed in depth: relayer internals beyond config and health, crank flows, miner
HTTP, the storefront-x402 paid-job path beyond x402 verification.

### Clients still signing cadre-auth-v1

V-9 protects only once clients sign v2 and gateways run with
`CADRE_AUTH_REQUIRE_AUDIENCE=1`. Clients to move, each by its owning row: cadre-proxy,
services/swarm, services/swarm/agents, ~/cadre-web lib/gateway.ts, ~/pearl-wallet
(background/pearl/ai.ts and the W-2 `/v1/keys` origin check), scripts/devnet-e2e,
scripts/demo. Each fetches `server` from the gateway's `/health` once and signs v2.

## 4. Cadre Wallet (pearl-wallet dev, d3b3f05d0, deps 448ae16ce)

W-1 pump buy destination, source and mint checked by account position; W-2 a foreign
site can no longer get a `/v1/keys` request signed; W-3 sign-in domain mismatch flagged;
W-4 dangerous Solana instructions named; W-5 simulated SOL balance change shown;
W-6 content script in the top frame only; W-7 auto-lock (default 15 min and on screen
lock); W-8 at most 3 pending approvals per origin; W-9 bidi and control characters
stripped; W-10 punycode hosts warned. 204 unit + 27 new tests, UI check 108/108.
Details: ~/pearl-wallet docs/PEARL-PORT.md section 9.

## 5. Ops (d8bf302)

Key-only sshd, unattended upgrades without reboot, caddy security headers, 0600 env
files, systemd sandbox drop-ins (written, not active), a health check timer on box 0,
and a backup script (docs/DEVNET.md). A gateway request-dir prune timer was written and
then removed: pruning box 0 is left to the owner.

## Agent chain (chain/, 0ecf5bf property tests, 9277119 fixes)

Property tests: every wire decoder never panics on random or mutated bytes and is
canonical (re-encodes byte-identical); the PoW verifier never panics and never accepts a
proof it did not mine; random tx sequences keep the supply invariant and are
deterministic. Fixes (none change consensus; they need a cadre-node rolling restart):

| id | severity | fix |
|---|---|---|
| C-1 | high | mempool admission checks the sender balance against all its pooled fees; 64 MiB cap; cheapest-first eviction; replacement needs +10% fee |
| C-2 | high | BridgeUnlock signatures checked before pooling, processed deposits refused |
| C-3 | high | orphan pool has a 256 MiB budget, oldest dropped first |
| C-4 | medium | header-only buffer drops its oldest entry when full |
| C-5 | medium | job headers checked against the live parent's state |
| C-6 | high | "timestamp too far in the future" no longer cached as invalid, and no peer ban for it |
| C-7 | high | getBlob unknown-id scan cached per tip |
| C-8 | medium | unsolicited peer blobs fill at most half the blob store |
| C-9 | medium | RPC batch cap 256, bounded WebSocket queues and sessions, 1 MiB messages, 60 s timeout |
| C-10 | medium | getBlockTemplate reused within 1 s; getTicket lists at most 4096 segments |
| C-11 | low | sendTx refuses an unpayable min_fee or a zero admin key |
| C-12 | low | the mock verifier is refused for the devnet chain id |

Chain workspace 138 tests pass. Open, by area (details private): two sync-path resource
issues handed to the sync-fix row; attester and evaluator signed messages carry no
chain_id (mainnet attesters and evaluators must be fresh keys until a coordinated
chain + relayer + attester change); putBlob rate limits are per node, not per client;
RegisterModel pins its blobs for min_fee (8B row); listJobs and history index bounds.

**Admin powers.** AdminConfig can name the attesters, so the admin can unlock the whole
escrow balance (a mint-equivalent power), and can change every consensus setting
(difficulty limits, block time, finality K, emission, alpha, fallback parameters). It
cannot move an account balance directly. On Solana, bridge `set_signers` /
`set_release_modes` and lightclient `set_params` are full custody too. Before mainnet:
multisig plus timelock on all of them.

## 6. Dependencies

Rust (cargo-audit 0.22.2):

- chain, token, crates/pearl-verify-sbf: no advisories.
- crates and crates/pearl-cli: rustls-webpki 0.103.16 (RUSTSEC-2026-0382) and
  webpki-roots 1.0.10 (RUSTSEC-2026-0381), in-semver. pearl-cli is bumped (section 7);
  crates/Cargo.lock waits for its other uncommitted edits:
  `cargo update -p rustls-webpki -p webpki-roots@1.0.9`.
- services/header-relay (14) and token/tests/{bridge,lightclient}-litesvm (10 each):
  the Solana 1.x/2.x client tree (ed25519-dalek 1, curve25519-dalek 3, rustls 0.21,
  h2 0.3). Only a major Solana SDK upgrade clears them. The ed25519 issue needs
  mismatched key pairs, which solana `Keypair` never produces.

npm (high / critical before the pass):

| package | high | critical | notes |
|---|---:|---:|---|
| services/crank | 9 | 0 | bn.js 5.2.5 (6ed714e) |
| services/storefront-x402 | 8 | 0 | bn.js 5.2.5 (6ed714e) |
| services/relayer | 0 | 0 | 5 moderate |
| token/tests/reserve, token/tests/storefront | 9 each | 0 | test harnesses |
| services/agent-cli | 4 | 0 | superseded code |
| extension | 6 | 2 | dev tooling only, superseded by ~/pearl-wallet |
| explorer, cadre-api, cadre-proxy, swarm | 0 | 0 | no dependencies |

The remaining highs all come from @solana/web3.js 1.x, bigint-buffer (no patched
release) and Anchor's toml parsing; npm's suggested fixes are downgrades or major
versions.

## 7. Services follow-ups (V-2b, V-8, V-9, V-10, pearl-cli lockfile)

Commits 9bae779 (V-2b, V-8, V-9, V-10) and a69323a (pearl-cli lockfile). Tests on the
test box (8 vCPU): attester 6 + evaluator 8, cadre-gateway 10 + auth_replay 2 +
auth_audience 2 + discover_throttle 1, pool 11, pearl-job, pearl-miner and pearl-node
green, pearl-cli 38 with `--locked`.
Redeployed 2026-10-21 (owner "Yes, allow deploy restarts"; run by the orchestrator
session): test box cadre-api restarted (health and summary 200); devnet box 0 synced to
154a819, cadre-gateway and pearl-attester installed and restarted, pearl-miner rebuilt
(sha256 295bfca2...) and restarted, cadre-explorer and cadre-api restarted (public API
200); boxes 1-2 the same pearl-miner (previous binary kept as .prev), cadre-miner
restarted, nodes active. Miners installed with install-miner.sh pick up V-2 on their
next install. The cadre-proxy fix (V-6) reaches users through the repo.

Still open from the services area: V-7, the V-9 client migration and strict mode, and
the crates/Cargo.lock webpki bumps (another row's uncommitted lockfile).
