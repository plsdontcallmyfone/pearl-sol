# `pearl` CLI and terminal dashboard

`crates/pearl-cli`, binary `pearl`. A retro blue terminal dashboard (ratatui) and the same
commands in plain mode for scripts and agents (`--json`). Every number on screen is an RPC
read from the agent-chain node, the Solana validator or the pool, or arithmetic over such
reads (named in `pearl status --json` under `derived`). There is no price source, so no
price is shown.

**Naming.** The product name is a placeholder. Everything user-visible (name, ticker, the
block wordmark, the `<name>` corner mark) comes from `crates/pearl-cli/src/brand.rs`
(`NAME`, `TICKER`); the wordmark is generated from `NAME` by a built-in block font, so a
rename is a one-line change. The crate (`pearl-cli`) and binary (`pearl`) names will change
with the rename.

## Build

```
cd crates/pearl-cli && cargo build --release      # Rust 1.95.0 (rust-toolchain.toml here)
crates/pearl-cli/target/release/pearl --help
```

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
| `--json` | | | plain mode: one JSON object (`"ok": true/false`) |
| `--wait` | | | wait for the effect: tx included; `bridge in` also waits for the unlock on the chain |

Localnet throwaway keys: `keys/localnet/` (e.g. `miner-keypair.json`, which the demo funds).
Nothing touches `~/.config/solana` or `solana config`.

## Dashboard

`pearl --key keys/localnet/miner-keypair.json` (no command) opens it. Screenshot of the live
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

## Commands (plain mode and the shell line)

```
pearl balance | address | status            # status = everything the dashboard reads
pearl send <to> <amount> [--fee T]
pearl jobs [--status open] [--limit 20]
pearl job show <id>
pearl job post --pay W --fee P [--m 64 --n 64 --k 2048 --deadline 500 --max-blocks 10
               --evaluator ADDR --threshold 1 --seed S | --data job.blob]
pearl blocks [n] | activity [--limit 20] | tx <hash>
pearl bridge in <amount>                    # Solana deposit for your chain address
pearl bridge out <amount> [--to SOLANA]     # BridgeLock; released when K deep
pearl bridge status
pearl lien set <solana-recipient> <bps> [--until H --cap T --release-key ADDR]
pearl lien clear --release-key-file F [--account A] | lien show
pearl register <name> [uri] | agent [address]
pearl mine start [--threads 2] | mine stop | mine status
pearl snapshot out.html [--width 140 --height 46 --tab jobs]   # render the dashboard
```

Amounts are decimal tokens (`1.5`); chain units are 9 dp, SPL 6 dp (bridge amounts take at
most 6 decimals). `job post` generates random int7 matrices (or reads a PSD1 blob), puts the
data blob on the node, computes `hash_a` / `hash_b` against the job key of the very tx it then
signs, checks P against `min_protocol_fee_effective`, and sends PostJob; the default evaluator
is the chain's first attester. `mine start` in the dashboard runs `pearl-miner` with your key
as payout in the background (log in `$TMPDIR/pearl-cli-miner-<pid>.log`), stopped by
`mine stop` or on quit; only that child is ever stopped. In plain mode it runs in the
foreground.

`--json` prints one object per command; errors print `{"ok": false, "error": "..."}` and exit 1.

Nonces: the next nonce is past the account's tip nonce, past any of its txs still in the
node's mempool (`getMempool` + `getTx`, so separate `pearl` processes can send back to back)
and past the last one this process used.

## Tests

`cd crates/pearl-cli && cargo test`: formatting, amount parsing, shell-line parsing, PDA / ATA
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
`python3 crates/pearl-cli/pty-smoke.py BIN KEY OUT` drives the real dashboard in a pty
(tabs, detail popup, `mine start` / `mine status` / `mine stop`, `quit`): all checks found,
exit 0, the spawned miner mined 3 blocks and was stopped by PID.

Screenshots: `crates/pearl-cli/screenshot.sh KEY` renders each tab from the live stack
(TestBackend buffer -> HTML -> headless Chromium) into `screenshots/cli/live-<tab>.png`.
