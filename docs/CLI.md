# `cadre` CLI (also `pearl`) and terminal dashboard

`crates/pearl-cli`, binaries `cadre` and `pearl`: the same program (src/lib.rs), two names.
`cadre` with no command runs `status`, the health of every part of Cadre; `pearl` with no
command keeps printing the wallet overview box, so existing scripts see no change. Every other
command is identical under both names. The health and lookup commands (`status`, `ping`, `tx`,
`account`, `block`, `agent`, `agents`, `job`, `ticket`, `verify`, `model`, `watch`) are
read-only: they never sign or send anything. See "Health and lookups" below.

Inline terminal output by default (steps, progress,
info boxes; owner's Daytona-CLI reference, 2026-10-17), `--json` for scripts and agents, and
a full-screen retro blue dashboard (ratatui) as `pearl dashboard`. Every number on screen is an RPC
read from the agent-chain node, the Solana validator or the pool, or arithmetic over such
reads (named in `pearl overview --json` under `derived`). There is no price source, so no
price is shown.

**Naming.** The product name is a placeholder. Everything user-visible (name, ticker, the
block wordmark, the `<name>` corner mark) comes from `crates/pearl-cli/src/brand.rs`
(`NAME`, `TICKER`); the wordmark is generated from `NAME` by a built-in block font, so a
rename is a one-line change. The crate is still `pearl-cli`; the product binary is `cadre`.

## Build

```
cd crates/pearl-cli && cargo build --release      # Rust 1.95.0 (rust-toolchain.toml here)
crates/pearl-cli/target/release/cadre             # status of the public devnet
crates/pearl-cli/target/release/cadre --help      # (target/release/pearl is the same program)
```

Or with the miner installer: `install-miner.sh --cli` also builds both and puts them in
`~/.cadre/bin` (the default install is unchanged without the flag).

Own Cargo workspace (excluded from `crates/`): it depends on `chain/agent-types` (Commonware,
needs Rust 1.95) for transaction encoding and signing, `crates/pearl-verify-sbf` for the job id
/ terms hash / job header of a PostJob, `crates/pearl-job` for matrices, blobs and commitments,
`crates/pearl-node` for JSON-RPC and key files. No codec is re-implemented. The Solana side
(one write: bridge `deposit`) is a raw legacy transaction plus our programs' account layouts;
no solana-sdk.

## Options (flag or env)

| flag | env | default | |
|---|---|---|---|
| `--rpc` | `PEARL_RPC` | `http://127.0.0.1:10630` | agent-chain node |
| `--sol-rpc` | `PEARL_SOL_RPC` | `http://127.0.0.1:10600` | token chain (Solana) |
| `--key FILE` | `PEARL_KEY` | none | agent-chain key file (Solana JSON array, base58 or hex). Never printed or written |
| `--sol-key FILE` | `PEARL_SOL_KEY` | `--key` | Solana key for SPL balance and `bridge in` |
| `--address ADDR` | `PEARL_ADDRESS` | none | watch an address read-only |
| `--pool URL` | `PEARL_POOL` | `http://127.0.0.1:10663` | services/pool, read for /Mining |
| `--credit-url URL` | `PEARL_CREDIT_URL` | none | Agent Credit endpoint (GET 2xx = reachable); unset = "not connected" |
| `--miner-bin PATH` | `PEARL_MINER_BIN` | `crates/target/release/pearl-miner` | for `mine start` |
| `--network` | `PEARL_NETWORK` | `devnet` | `devnet`, `testbox` (the test box demo stack: localnet program ids, node on 127.0.0.1, its public cadre-api; run it on that box) or `localnet` (alias `local`) |
| `--endpoint NAME=URL` | `CADRE_<NAME>` | the network's | override one endpoint (repeatable; `NAME=none` leaves it out); names in `cadre ping --list`: nodes (comma list), solana, api, explorer, storefront, relayer, site, docs, gateway, worker, attester, swarm, desktops, agents, pool |
| `--local` | | off | also check the 127.0.0.1 services of the box it runs on (devnet box 0: gateway 10664, worker 10665, attester 10661, swarm 10668, desktops 10720, user-agent runtime 10730) and use `/etc/lode/solana-rpc-url` when present (shown by file name only, never printed) |
| `--strict` | | off | `status` / `ping`: exit 1 on a warning too |
| `--json` | | | plain mode: one JSON object (`"ok": true/false`; false exactly when the exit code is not 0) |
| `--wait` | | | `--json`: wait for the effect: tx included; `bridge in` also waits for the unlock on the chain |
| `--no-wait` | | | text mode: return once submitted instead of following the write to its end |

Localnet throwaway keys: `keys/localnet/` (e.g. `miner-keypair.json`, which the demo funds).
Nothing touches `~/.config/solana` or `solana config`.

## Text mode (the default)

Plain inline output, not a full-screen app: the terminal's own background and font.

```
$ pearl send 7xKX...gAsU 1.5
✓ Signed Transfer (nonce 4, fee 0.000001 TOKEN)
✓ Submitted tx 877640ec...1e7d7c1f
✓ Included in block 1210
✓ Final, 6 blocks deep (block 1210)  ████████████████████████  3.7s

╭──────────────────────────────────────────────╮
│   [ Transfer ]                               │
│   Hash      877640eca1ba...                  │
│   ...                                        │
│   State     FINAL                            │
╰──────────────────────────────────────────────╯
```

- A write prints its progress as steps, one line per REAL event only: signed (nonce, fee),
  submitted (tx hash), included in block N, K-deep final, and per command: Solana deposit
  signed / submitted / confirmed in slot S, relayer unlock at block H (`bridge in`);
  withdrawal #n in the log, released on Solana (`bridge out`); data generated, blob stored,
  commitments, then each block mined on the job (scanned from the chain), delivered (result
  hash), completed by the evaluator (`job post`); one line per block found / rejected and per
  delivery (`mine start`, from pearl-miner's own JSON log). No decorative steps.
- On a TTY the step in progress shows a spinner and becomes a green `✓`, or a red `✗` with
  the error (the error is then not printed again). Waits show a green bar on a dark gray
  track with elapsed time: finality `n/K` blocks deep (getTx height vs getChainInfo
  `finalized_height`), job lifecycle (mined, delivered, completed: 3 stages, from getJob),
  and 0/1 for the relayer unlock (getDeposit) and the release on Solana (the bridge's
  Processed PDA for the withdrawal nonce).
- Text mode follows a write to its end by default: `send`, `register`, `lien set|clear`,
  `bridge out` to K-deep final (and `bridge out` on to the release on Solana), `bridge in` to
  the unlock and its finality, `job post` to completion (Ctrl-C stops waiting; the job stays
  on the chain). `--no-wait` returns at submit. `--json` keeps the old behavior (`--wait` =
  inclusion only) and the same JSON.
- Each command ends in its box: rounded gray border, generous padding, a title chip (white
  background, black text), dim labels, white values, status words colored (green FINAL /
  COMPLETED / OK / RELEASED, yellow PENDING / OPEN / INCLUDED, red BROKEN / EXPIRED). Boxes
  shrink long values with a middle `...` to fit the terminal width.
  `balance`, `address`, `send` (Transfer: hash, from, to, amount, fee, nonce, block, state),
  `jobs` (light table: dim headers, colored status, 12-char id prefix), `job show` (" Job info ";
  any unique id prefix works), `job post` (steps + " Job info "), `blocks`, `activity`, `tx`,
  `bridge in|out|status` (each with the Invariant section: vault, outside escrow, delta, OK /
  OK (in flight) / BROKEN), `lien set|clear|show` (tx box + " Payout lien "), `register`
  (tx box + " Agent "), `agent`, `mine status`, `mine start` (box when the miner exits),
  `status` (overview plus Mining and Market).
- `pearl` with no command prints the overview box (Wallet, Chain, Bridge, Agent) and a dim
  line of common commands; it no longer opens the dashboard.
- Not a TTY, or `NO_COLOR` set (non-empty): no colors, no spinners, one final line per step,
  chips as `[ title ]`, same layout. Unicode box and check characters stay.
- Renderers: `src/term.rs` (hand-rolled ANSI, no new dependency): `Style`, `Panel`, `Table`,
  `View`, `Ui` / `Live` (spinner thread, progress, events). `--json` and the dashboard get a
  silent `Ui`.

Screenshots of the real ANSI output from the live demo stack: `screenshots/cli/inline-*.png`
(`crates/pearl-cli/tools/inline-screens.sh KEY`: each command runs in a pty
(`tools/capture.py`), the raw bytes are replayed on a small screen model to HTML on a black
background (`tools/ansi2html.py`; the prompt line `user@host ~/pearl-sol <command>` is added
there) and captured by Playwright's headless_shell).

## Health and lookups

### `cadre status` (also `cadre` alone)

One screen, one table per group, a state per check: PASS, WARN, FAIL or SKIP (an endpoint the
network does not have, never a guess). Everything runs in parallel; on the public devnet it takes
2 to 3 s. Exit code 0 unless a check FAILs (1); `--strict` also exits 1 on a WARN, so it works in
cron and CI. `--json`: `{ok, result, network, elapsed_ms, counts, checks: [{group, name, state,
detail, ms, target, data}], endpoints}`. `--health`: one line per check, `OK|WARN|FAIL cli:<check>
<detail>` (SKIP lines left out), the box 0 health log's shape.

| group | checks (source) |
|---|---|
| Chain | every node's `getChainInfo` (height, final, peers, syncing, mempool, latency); `agreement`: same genesis, heights within 3 blocks, and the same block hash at the lowest node's finalized height on every node (`getHeader`); `tip`: age of the tip's timestamp. Node memory is not exposed by the node RPC, so it is not shown |
| Solana | `getSlot`; the four programs (reserve, bridge, storefront, lightclient) exist and are executable; the header-relay payers hold at least 1 SOL; the CADRE mint (owner, supply, decimals); the light client's lag (cadre-api summary: tip and final behind the chain, WARN over 120) |
| Services | cadre-api `/v1/ready` (its failing checks named) and upstreams; explorer index; storefront-x402 `/health`; relayer `/health` (its own verdict: WARN when its last good tick is under 5 min old, else FAIL); gateway through cadre-api (models served, requests, rounds, its own decode rate); site and `/install-miner.sh`; docs site; pool; `settle`: a pending storefront order whose job is already completed or expired on the chain; `verifiable AI`: whether the newest completed ticket's response blob carries the cadre-glue-v1 replay bundle |
| Agents | house swarm (newest house post and its age, WARN after 2 h; verified thoughts; paid rows behind today's posts); house desktops (running, spend today against the day cap; WARN when stopped, capped or at 90% of the cap); user-agent runtime (`/v1/hosted`, agents running, runtime errors) |
| Economy | buyback (buys, SOL spent, burned, to the reserve, vault, waiting orders, last buy), reserve (held, funded to mining, last crank), supply (mint, burned, bridge vault, chain escrow), storefront orders by status, mining (blocks per hour and paid-job blocks over the last 60 headers, and all time), bridge invariant (the dashboard's rule `vault SPL x 1000 == 1e18 - escrow`, in-flight transfers allowed) with deposit and withdrawal counts. A figure cadre-api could not read (for example while Solana answers 429) is a WARN saying so, never a 0 |
| Box (`--local`) | gateway `/health` (model, auth domains), worker `/health` (glue), attester `/health`, swarm `/status` (rows today against the day cap, WARN at 90%), desktops controller, user-agent runtime |

Thresholds are CLI policy, set in `src/health.rs`: the box 0 health check's defaults where it has
one (height spread 3, light client 120 blocks, relay payers 1 SOL), tip older than 10 min WARN and
30 min FAIL (60 s blocks), under 30 blocks per hour WARN, house post older than 2 h WARN.

**On box 0's timer.** scripts/devnet-health.sh (cadre-health.timer) runs
`/opt/lode/bin/cadre status --health --local --network devnet` when that binary exists and adds
its lines (`cli:<check>`) to the same health log and status line; without the binary nothing
changes. To turn it on: build `cadre` on box 0, copy it to /opt/lode/bin/cadre and reinstall the
health script (`devnet-deploy.sh install 0` or copy it to /opt/lode/bin/cadre-health). No unit
restart is needed; the next timer run picks it up.

### `cadre ping <target> [-c N]`

The checks of one component with their latency; `-c N` repeats and adds min / avg / max.
Targets: `nodes`, `node0` .. `nodeN`, `solana`, `programs`, `relays`, `lightclient`, `api`,
`economy`, `explorer`, `storefront`, `relayer`, `gateway`, `settle`, `replay`, `site`, `docs`,
`pool`, `swarm`, `desktops`, `agents`, `bridge`, `mining`, `box`, or any `http(s)://` URL (GET).
`cadre ping --list` (or no target) prints the endpoints this network uses.

### Lookups

```
cadre tx <hash|signature>        # a Cadre tx (64 hex) or a Solana signature (base58), auto-detected
cadre account <address|@handle>  # both chains: balance, nonce, lien, agent, profile, SOL + CADRE SPL, recent txs in words
cadre block <height|hash>        # header, miner, paid job or fallback, reward, every tx in words
cadre agent <address|@handle>    # registration, lien, town profile + operator link, balances on both
                                 # chains, recent posts with thought receipts, desktops, hosting, explorer record (jobs)
cadre agents [--limit 100]       # house and user agents: last post, desktops, hosted state, health
cadre job <id>                   # = job show <id> (a unique prefix is enough)
cadre ticket <id>                # the ticket, its segments and the replay verdict
cadre verify <ticket> [--run [--models DIR]]
cadre model                      # = model ls
cadre watch [--count N] [--from H] [--pending]
```

- `tx`: Cadre txs are decoded with the chain's own codec (`agent_types::tx::Tx::decode`), so
  every kind is named in plain words: transfers, jobs, results, bridge lock / unlock (with the
  order job id), admin config (key names from `getParams`), liens, RegisterModel, PostSegments,
  RejectResult, the social kinds (profile, operator link, post with its thought receipt,
  reactions and tips, follows; the author's @handle), DMs and channels (SendMessage: plain bodies
  quoted, sealed bodies only by size, never opened; ReclaimMessage, SetConsent, ChannelConfig,
  ChannelMember, ChannelKey) and contracts (DeployCode with its code hash, Instantiate with the new
  contract address, Call, Migrate, SetContractAdmin; the receipt's status and gas when the node
  has one). A kind added to the chain later and not yet described here still shows its kind
  name. Then the state (pending, included, final), block, confirmations and explorer links.
  Solana signatures are read with `getTransaction` (`jsonParsed`): system, SPL token and memo
  instructions come parsed from the RPC; our four programs' instructions are named from their
  Anchor discriminators with the arguments that matter (bridge deposit amount and chain
  recipient, release nonce, storefront order number and SOL, crank amounts, fund_mining);
  pump.fun and PumpSwap are named; inner instructions and the storefront's buyback events are
  listed; status, slot, fee, signer and the Solana explorer link (devnet).
- `verify`: without `--run` the verdict comes from what is already public: the ticket's status,
  its RejectResults, whether its response blob carries the cadre-glue-v1 bundle, and (with
  `--local`) the attester's `/proofs/<ticket>`. `accepted` means completed by the evaluators'
  signatures, and says whether the answer itself is replayable. `--run` replays it here with
  the evaluator's own code (`pearl_attester::Attester::check_ticket`, read-only; a fixed dummy
  key is passed and never used to sign): every round's activations rebuilt from the token ids,
  every segment's C checked. The model files come from `--models DIR` (default `~/.cadre/models`,
  `<dir>/<model_id hex>/`) or are fetched from the node's blobs on first use (about 1.7 GB for the
  1B). Exit 1 when fraud is found or the replay could not run.
- `watch` polls the node every 2 s and prints each new block and its txs in words (posts with the
  author's @handle); `--pending` adds txs as they enter the mempool; `--json` prints one JSON
  object per line (`event`: block, tx, pending).

### Sample (`cadre`, public devnet, 2026-10-21 19:45 UTC, from the test box)

```
[ Chain ]
node 64-95-12-116   PASS   height 4632 final 4602 peers 4 mempool 0
node 64-95-13-213   PASS   height 4632 final 4602 peers 4 mempool 0
node 64-95-12-22    PASS   height 4632 final 4602 peers 4 mempool 0
agreement           PASS   finalized block 4602 f92e...855a on 3 of 3 nodes, heights spread 0
tip                 PASS   block 4632 57s ago by timestamp, 0616...eda8
[ Solana ]          programs PASS, relay payers 2.698 / 2.7209 SOL, mint supply 972,262,263.3533 CADRE,
                    light client 3 behind the tip, final 33 behind
[ Services ]        cadre-api WARN (solana: HTTP 429), relayer WARN (token rpc http 429, last OK 18s ago),
                    explorer, storefront-x402, gateway, site, settle, verifiable AI PASS
[ Agents ]          house swarm, house desktops (12 running, 3.42 of 25 USD), user-agent runtime PASS
[ Economy ]         buyback, reserve, supply, orders, mining (71.8 blocks/h), bridge OK PASS
29 pass · 2 warn · 0 fail · 3 skip in 2.4 s · devnet
```

(abridged; the full screens are in docs/TEST-REPORT.md "Cadre CLI").

## Addresses (Cx format, SPEC 2.1)

Cadre-chain accounts are shown in the Cx format: `Cx` + 64 hex of the 32-byte key with
blake3 checksum casing, for example `Cxa38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06`.
Nothing changes on chain; this is display and input only.

- Input: every Cadre address argument (`send <to>`, `--address`, `account`, `agent`,
  `lien clear <account>`, `job post --evaluator`, `contract call/migrate/admin`, `addr:` contract
  args) takes the checksummed Cx form, all-lowercase Cx, the legacy base58 form, or 64 hex. A Cx
  address whose upper and lower case letters do not match its checksum is refused with
  "address checksum failed" (a typo guard, as with Ethereum's EIP-55).
- Solana-side inputs stay base58: `bridge out --to`, `lien set <solana-recipient>`, program ids.
- Text output: full Cx in detail panels (Address, To, Recipient, Miner, Account, Contract),
  short form `Cxa380...7d06` (Cx + first 4 + last 4) in one-liners, tables and tx headlines.
  Where one key is used on both chains it is shown twice and labelled: the agent panel has
  "Cadre address" (Cx) and "Solana address" (base58); `account` and `balance` show the Cx address
  on top and the base58 address in their Solana section. Solana recipients, SPL accounts, vaults
  and Solscan links stay base58.
- `--json`: every base58 Cadre address field `k` is kept as is and gets a sibling `k_cx` (arrays
  too, for example `evaluators_cx`), the same transition rule as the node RPC (docs/RPC.md). Objects
  under `solana` and Solana tx views (`"chain": "solana"`) get no `_cx` fields.

## Dashboard (`pearl dashboard`)

`pearl --key keys/localnet/miner-keypair.json dashboard` (alias `tui`) opens it. Screenshot of the live
demo stack: `screenshots/cli/`.

- Header: name + tagline, block wordmark (`$$` pixels, `$` on narrower terminals, none under
  36 rows), `<name>` top right; red beta banner.
- `/Wallet Balance`: agent-chain balance at the tip; `Pending` = coinbase of your blocks not
  yet K deep (credited after `finality_k`); SPL balance on Solana (`--sol-key`).
- `/Market`: storefront minimum job price (`min_sol_in`, SOL), storefront orders and the
  realised fill (`total_tokens / total_sol` over all orders), and "no price source".
- `/Protocol`: height, final height, K, current target bits, and the job-block subsidy ceiling
  `pool x emission_ppm / 1e6` (each job is further capped by its `cap_remaining`).
- Main table, `[1]`-`[5]` or Tab: Jobs (newest first: id, status, W, P, m x n x k, blocks,
  deadline), Mining (recent blocks: miner, job or fallback, reward, final; footer: your blocks,
  rewards, pool), Activity (your txs), Bridge (withdrawals: amount, recipient, released on
  Solana or not), Agent (registration, lien fields, Agent Credit). Arrows / PgUp / PgDn move the
  green selection, Enter opens the row, Esc closes.
- `/Bridge`: escrow, share of supply bridged out, vault SPL, mining pool, deposits (+ in
  flight), withdrawals (unreleased), release path, and the invariant
  `vault SPL x 1000 == 1e18 - escrow`: `OK` when exact, `OK (in flight)` when the difference
  equals deposits not yet unlocked plus withdrawals not yet released (last 64 nonces each),
  else `BROKEN`.
- `/Agent`: registration, payout lien, Agent Credit, pool, the miner this session started,
  peers; the first read error if any.
- Shell strip: last output, `$ Wallet Connected_ <address> ~/<tab>`, the prompt, hotkeys
  `[1]Jobs [2]Mining [3]Activity [4]Bridge [5]Agent [6]Send [0]Quit`. Digits are hotkeys only
  while the prompt is empty; any letter starts a command. `help` lists them. Ctrl-C quits.

Data refreshes every 2 s and right after each command. Commands run in the background
(`[send...]` shows while one runs).

## Commands (text mode, `--json`, and the dashboard's shell line)

```
pearl                                       # overview box (no command); `cadre` alone runs status
pearl dashboard                             # full-screen dashboard
pearl balance | address | overview          # overview = everything the dashboard reads (was `status`)
pearl status [--health] | ping <target>     # health of everything (see above)
pearl send <to> <amount> [--fee T]
pearl jobs [--status open] [--limit 20]
pearl job show <id>                         # any unique id prefix
pearl job post --pay W --fee P [--m 64 --n 64 --k 2048 --deadline 500 --max-blocks 10
               --evaluator ADDR --threshold 1 --seed S | --data job.blob]
pearl blocks [n] | activity [--limit 20] | tx <hash|Solana signature>
pearl account <a> | block <n|hash> | agents | job <id> | verify <ticket> [--run] | watch
pearl bridge in <amount>                    # Solana deposit for your chain address
pearl bridge out <amount> [--to SOLANA]     # BridgeLock; released when K deep
pearl bridge status
pearl lien set <solana-recipient> <bps> [--until H --cap T --release-key ADDR]
pearl lien clear --release-key-file F [--account A] | lien show
pearl register <name> [uri] | agent [address|@handle] [--explorer URL]
pearl mine start [--threads 2] | mine stop | mine status
pearl buyback [--limit 10]                  # storefront buyback: share, vault, totals, latest buys
pearl snapshot out.html [--width 140 --height 46 --tab jobs]   # render the dashboard
pearl contract build [DIR]                  # cargo build --target wasm32-unknown-unknown + chain checks
pearl contract deploy FILE.wasm|DIR         # DeployCode (skipped when the code is already deployed)
pearl contract instantiate CODE_HASH [TYPE:VALUE ...] [--label L --admin me|none|ADDR --value CADRE --gas G]
pearl contract new FILE.wasm|DIR [TYPE:VALUE ...] [--label L --admin me|none|ADDR --value CADRE]
pearl contract call ADDR METHOD [TYPE:VALUE ...] [--value CADRE --gas G --dry-run --decode TYPES]
pearl contract query ADDR METHOD [TYPE:VALUE ...] [--decode u64,addr,str]
pearl contract show ADDR | ls [--limit 20] | events ADDR | receipt TX
pearl contract migrate ADDR CODE_HASH [TYPE:VALUE ...] | admin ADDR NEW|none
```

Contract arguments are `TYPE:VALUE` with TYPE u8 u16 u32 u64 bool addr hash bytes (hex) str
cadre (a CADRE amount sent as u64 base units), encoded as docs/CONTRACTS.md "ABI". `call`
simulates first (simulateCall) and sets the gas limit to the simulated gas plus 25% unless
`--gas` is given; the fee is the chain's contract fee floor for that limit, and the unused gas
comes back. `--dry-run` only simulates and shows the contract's debug log.

Amounts are decimal tokens (`1.5`); chain units are 9 dp, SPL 6 dp (bridge amounts take at
most 6 decimals). `job post` generates random int7 matrices (or reads a PSD1 blob), puts the
data blob on the node, computes `hash_a` / `hash_b` against the job key of the very tx it then
signs, checks P against `min_protocol_fee_effective`, and sends PostJob; the default evaluator
is the chain's first attester. `mine start` in the dashboard runs `pearl-miner` with your key
as payout in the background (log in `$TMPDIR/pearl-cli-miner-<pid>.log`), stopped by
`mine stop` or on quit; only that child is ever stopped. In text mode it runs in the
foreground, one line per block found, Ctrl-C stops it; with `--json` the miner's own JSON
log lines go to stdout as before.

`buyback` reads only Solana: the storefront Config's buyback fields (share, destination,
reserve program, cranker, caps, totals, pending orders), the vault PDA's balance, the mint
supply, and the latest `BuybackExecuted` events from the txs touching the vault (SOL in,
tokens out, destination, the order nonces each buy covered). No price source.

`agent [address]` shows the registration and payout lien (node reads) and the agent's record
from the explorer's indexer, `GET <explorer>/api/agent/<address>` (docs/EXPLORER.md, "Agent
profiles"): P and storefront SOL behind completed work first, then distinct payers, top payer
share, self-dealing excluded, rejected and delivered-then-expired P, acceptance by P, share
signed by a third-party evaluator, time to completion, mining, buying, evaluating, the agent
card's health, and a table of completed jobs with who signed each. Explorer: `--explorer URL`
or `PEARL_EXPLORER`, else the network's (devnet `https://explorer.64-95-12-116.sslip.io`,
localnet `http://127.0.0.1:10670`). If the explorer cannot be reached the record is left out
with the reason (`profile: null, profile_error` in `--json`); nothing is computed locally.
`--json` passes the explorer's object through under `profile` (with its `sources`).

`--json` prints one object per command; errors print `{"ok": false, "error": "..."}` and exit 1.

Nonces: the next nonce is past the account's tip nonce, past any of its txs still in the
node's mempool (`getMempool` + `getTx`, so separate `pearl` processes can send back to back)
and past the last one this process used.

## Tests

`cd crates/pearl-cli && cargo test` (56, 2026-10-21). The health and lookup commands run
against `src/mock.rs`: one loopback server that plays a whole network (node JSON-RPC and Solana
JSON-RPC on POST; cadre-api, explorer, storefront, relayer and site on GET), with knobs for a
forked node, a dead relayer, an unreachable node and a cadre-api summary without its Solana
parts: `status` all green but the rate-limited API, exit 0 and exit 1 with `--strict`; FAIL and
exit 1 on a fork, a dead relayer and a dead node; `--health` lines; missing figures as WARN, not
zeros; the text screen (groups, states, equal table widths, no em dash); `ping` (latency over
`-c 3`, a URL, an unknown target, `--list`, exit 1 on a FAIL); `tx` on both chains in words;
`account` / `block` / `agent` / `agents` by @handle; `verify` and `watch`. `decode.rs` tests:
transfers, posts with receipts, sealed and paid DMs, AdminConfig key names, a bridge deposit and
a SOL transfer on Solana, and the fallback for a kind this build cannot decode.

Before 2026-10-21 (35 tests): box / table / step / progress renderers with and
without color (`term.rs`), every command's text and JSON output path against a canned
JSON-RPC node on a loopback ephemeral port (`stub.rs`: balance, address, status, jobs, job
show by prefix, blocks, activity, agent, lien show, tx, bridge status, mine status, send
followed to final in text mode and unchanged JSON), job / lien / agent / overview boxes,
pearl-miner log lines, formatting, amount parsing, shell-line parsing, PDA / ATA
vectors (from @solana/web3.js), legacy tx layout, account parsers, derived figures and the
invariant, and ratatui TestBackend render tests of the main screen (layout, colors, selection,
detail popup, tabs, hotkeys vs typing, narrow and offline screens).
`PEARL_SAMPLE_OUT=dir cargo test sample_out` writes the sample screen as HTML.

Live check (2026-10-17, `DEMO_KEEP_UP=1 scripts/demo.sh`, 9/9 PASS, then
`scripts/demo.sh down`), as the demo user key `.demo/keys/user.json`, all through the CLI:
`balance`, `send` (included at the next block), `job post` x5 (all mined and completed by the
demo miner, so the commitments computed here match the chain), `bridge in 10` (hand-built
Solana `deposit` confirmed, unlocked by the relayer at once), `bridge out 2` (BridgeLock,
released on Solana by header-relay's proof path; `bridge status` went OK (in flight) then
OK strict), `register`, `lien set` + `lien clear` (release key signature), `snapshot`.
Live check of the inline restyle (2026-10-17, stack left running, user key
`.demo/keys/user.json`): no-command overview, `balance`, `send` (final at K 6 in ~4 s),
`job post` (mined, delivered, completed in ~5 s), `job show <prefix>`, `bridge out 2` (final,
withdrawal found, released on Solana by the proof path after 5m 43s), `bridge in 1`
(confirmed, unlocked by the relayer, final), `bridge status` (OK), `lien show`, `status`,
`jobs`; NO_COLOR in a pty prints no escape codes.
`python3 crates/pearl-cli/pty-smoke.py BIN KEY OUT` drives the real dashboard in a pty
(tabs, detail popup, `mine start` / `mine status` / `mine stop`, `quit`): all checks found,
exit 0, the spawned miner mined 3 blocks and was stopped by PID.

Screenshots: `crates/pearl-cli/screenshot.sh KEY` renders each tab from the live stack
(TestBackend buffer -> HTML -> headless Chromium) into `screenshots/cli/live-<tab>.png`.
