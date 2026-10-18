# `pearl` CLI and terminal dashboard

`crates/pearl-cli`, binary `pearl`. Inline terminal output by default (steps, progress,
info boxes; owner's Daytona-CLI reference, 2026-10-17), `--json` for scripts and agents, and
a full-screen retro blue dashboard (ratatui) as `pearl dashboard`. Every number on screen is an RPC
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
pearl                                       # overview box (no command)
pearl dashboard                             # full-screen dashboard
pearl balance | address | status            # status = everything the dashboard reads
pearl send <to> <amount> [--fee T]
pearl jobs [--status open] [--limit 20]
pearl job show <id>                         # any unique id prefix
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
pearl buyback [--limit 10]                  # storefront buyback: share, vault, totals, latest buys
pearl snapshot out.html [--width 140 --height 46 --tab jobs]   # render the dashboard
```

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

`--json` prints one object per command; errors print `{"ok": false, "error": "..."}` and exit 1.

Nonces: the next nonce is past the account's tip nonce, past any of its txs still in the
node's mempool (`getMempool` + `getTx`, so separate `pearl` processes can send back to back)
and past the last one this process used.

## Tests

`cd crates/pearl-cli && cargo test` (35): box / table / step / progress renderers with and
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
