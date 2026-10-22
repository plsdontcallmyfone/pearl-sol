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

## Your wallet (`cadre wallet`, 2026-10-21)

Without `--key` / `--address` (or `PEARL_KEY` / `PEARL_ADDRESS`) `cadre` uses the wallet saved by
`cadre wallet use`, so it is set once:

    cadre wallet use --address Cx...        # watch only: balances, activity, inbox (no signing)
    cadre wallet use --key ~/cadre-key.b58  # signs: send, bridge, DMs, mining payouts
    cadre wallet show                       # what is saved
    cadre wallet forget

It is saved in `~/.cadre/cli.json`: the key file's PATH or the address, never key material. A key
file can be the Cadre Wallet's Settings, Export private key, saved to a file (base58), or any
Solana JSON keypair. A flag or env var still wins over the saved wallet. (`cadre wallet` was an
alias of `overview` until this; `overview` is unchanged.)

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

## Entry screen (`cadre` alone on a terminal)

`cadre` with no command, on an interactive terminal (stdin and stdout are TTYs, not `--json`,
not `TERM=dumb`), opens a full-screen start page modeled on opencode's start screen (owner,
2026-10-21): the owner's spinning ASCII robot (Rushu / Weazel VFX; 36 frames, 44 x 24, 12 fps,
`crates/pearl-cli/assets/robot-frames.json`, embedded with `include_str!`), the two-tone block
wordmark "cadre" (muted) + "agents" (bright) in opencode's pixel font, the prompt box, the hints
and a bottom bar (directory and git branch left; network and CLI version right). Code:
`src/entry/` (mod.rs screen and keys, robot.rs, logo.rs, theme.rs, net.rs). In every other case
(a pipe, `--json`, `cadre status`, any command) nothing changed: `cadre` runs `status`, `pearl`
prints the overview (tests/entry_cli.rs).

| Key | |
|---|---|
| text + Enter | **Ask** mode: a chat completion from the network's gateway (`<api>/openai/chat/completions`, or the gateway itself with `--local` / `--endpoint gateway=URL`), streamed (SSE). Paid from a prepaid balance: `--key` signs each request (cadre-auth-v2, audience = the gateway's server key from `/v1/models`), or `CADRE_API_KEY=cadre_sk_...` is sent as a bearer token. With neither, one setup line and no request. A system message of live facts read from cadre-api at ask time (tip, finalized, paid-job blocks, newest town posts) goes first; the last two exchanges are kept as context. Under the answer: model, request id, tokens out, rows, lamports charged (the receipt in the last chunk). `CADRE_MAX_TOKENS` (default 256) |
| Tab / Shift+Tab | Cycle Ask, then every house and user agent with a town profile (`/v1/profiles`, house first). Shows the agent's profile (`/v1/profiles/{a}`) and newest 5 posts. With DMs live (`/v1/dm/params` `active`), Enter prices a plain DM (`/v1/dm/gate` required pay + the `/v1/tx/send-message` builder's fee), shows the cost, and a second Enter signs and sends SendMessage with `--key`; Esc cancels. Not live: says so, sends nothing |
| ctrl+p | Palette: the app's views, then every command (from the clap definitions, groups as "job show", "bridge out", ...). Since 2026-10-21 everything runs inside the app (see "The Cadre app" below), never in the shell |
| `/cmd args` + Enter | A view (`/dashboard`, `/status`, ...) or that command, run inside the app (Lookup view) |
| PgUp/PgDn, Up/Down | Scroll the conversation; ctrl+l clears it (back to the start page) |
| Esc / ctrl+c | Esc closes the palette or cancels a pending DM; on an empty prompt it goes to the last view (Dashboard by default); ctrl+c quits. Raw mode, the alternate screen, bracketed paste and the cursor are restored on exit and in a panic hook |

Colors are opencode's default dark theme: prompt panel `#1e1e1e`, muted `#808080` ("cadre",
hint words, footer), text `#eeeeee` ("agents", model, keys), blue `#5c9cf5` (bar, mode label),
placeholder `#606060`, logo shadows 25% of the ink over opencode's `#0a0a0a` (`#282828`,
`#434343`). The screen itself is never painted: everything sits on the terminal's own background
(only the prompt panel, the logo shadows and the palette paint one), so a terminal theme cannot
turn painted cells into colored blocks (owner feedback 2026-10-21, over ssh). Checked against the
reference screenshot by downscaling the render to its scale and comparing peak grays
(screenshots/cli/entry-compare-reference.png).

Color depth: `NO_COLOR` (not empty) drops colors (dim and bold carry the two tones);
`CADRE_COLOR=truecolor|256|none` forces one; `COLORTERM=truecolor|24bit` means truecolor; otherwise
truecolor too, because ssh does not forward COLORTERM (TERM is just `xterm-256color` there) and
nearly every terminal takes 24-bit color. 256 colors only for terminals that clearly cannot
(`TERM=linux`, `vt*`, `ansi`, `dumb`, macOS Terminal.app). In 256 colors every gray is a gray-ramp
entry (232-255), never one of the 16 system colors or a color-cube cell that themes tint. To pass
COLORTERM through ssh anyway: `ssh -o SendEnv=COLORTERM host` (the server's sshd needs
`AcceptEnv COLORTERM`), or `ssh -t host CADRE_COLOR=truecolor /opt/lode/bin/cadre`.

Real-terminal check (2026-10-21, after the feedback): xterm 390 on Xvfb with TERM=xterm-256color
and no COLORTERM (what ssh gives): `screenshots/cli/entry-xterm-ssh-env-120x40.png`,
`entry-xterm-ssh-env-80x30.png`, `entry-xterm-truecolor-120x40.png` (COLORTERM=truecolor),
`entry-xterm-256-120x40.png` (CADRE_COLOR=256). A real `ssh -tt` session from the Mac to the test
box emitted only truecolor (5260 foreground codes, no 256-color codes) and three background colors:
the panel and the two logo shadows. xterm draws block characters from the font, so the wordmark's
rows show thin seams there; terminals that draw block elements themselves (Ghostty, iTerm2, kitty,
WezTerm, Alacritty) do not.

Sizes: full robot from 44 columns and 39 rows; the half-size robot (22 x 12, densest character of
each 2 x 2 block) on shorter terminals; none on very small ones. The wordmark is side by side from
58 columns, stacked from 31, plain words below that or when the height needs it; the prompt box is
at most 75 cells wide. Redraws only on input, data, or the robot's next frame (12 fps; about 0.6%
of one core measured over 10 s on the test box).

Screenshots (tmux at 120 x 40 and 80 x 30 on the test box, rendered from the captured cells):
`screenshots/cli/entry-120x40.png`, `entry-80x30.png`, `entry-small-40x20.png`,
`entry-agent-120x40.png`, `entry-palette-120x40.png`, `entry-palette-status-120x40.png`,
`entry-ask-nokey-120x40.png`, `entry-ask-402-120x40.png` (a fresh wallet on devnet: the gateway's
own 402), `entry-nocolor-80x30.png`, `entry-compare-reference.png`.

## Health and lookups

### `cadre status` (also `cadre` alone when piped or with `--json`)

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
cadre tx <hash> --watch [--timeout 3600]   # follow a Cadre tx from pending to final
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
  name. Then the state, block, confirmations and explorer links. The state is pending,
  preconfirmed (weak blocks on the current tip list the tx: count, miners, share, age of the
  latest and a low / medium / high confidence label, docs/SPEED.md 2.4), included or final, with
  a `Risk` line saying what it does and does not guarantee (SPEED.md 2.5); `--json` adds
  `tx_status`. A node from before the weak-block upgrade never says preconfirmed and has no
  `final` flag: final is then derived from the finalized height (src/txstatus.rs).
- `tx --watch`: polls `getTxStatus` (falling back to `getTx` on an older node) every 500 ms and
  prints one line per change with the seconds since it started (`0.4 s  preconfirmed (1 weak
  block, 1 miner, low, latest 0.3 s ago)  Seen in one miner's weak block ...`) until the tx is
  final or `--timeout` seconds pass; `--json` prints one object per change and ends with
  `ms_to_preconfirmed`, `ms_to_included`, `ms_to_final`.
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

## The Cadre app: one TUI with views that never reset (2026-10-21)

Owner feedback 2026-10-21: "it should all be integrated together, if i toggle the dashboard it
shouldn't reset", and the dashboard must use "the same complete style" as the entry screen.
`cadre` alone on a terminal, `cadre dashboard` and `pearl dashboard` (alias `tui`) now open ONE
full-screen app (src/entry/) on Home or on the Dashboard. The old blue `pearl dashboard` TUI
(src/ui.rs: blue background, "$$$$" banner, red beta warning, boxed panels, [1]-[6] tab bar) is
gone; `cadre snapshot` renders the new Dashboard view to HTML/text. Network and version sit in the
bottom bar instead of a beta banner. Non-TTY outputs (pipes, `--json`, every other command) are
unchanged (tests/entry_cli.rs).

| # | View | What |
|---|---|---|
| 1 | Home | the entry screen: robot, wordmark, Ask prompt, Tab through agents (unchanged) |
| 2 | Dashboard | cards: Wallet (CADRE, pending, CADRE on Solana, SOL), Market (job price, orders, CADRE per SOL, no price source), Protocol (block, final, K, difficulty, max subsidy, peers), Mining (your blocks of the window, earned, pending, pool, miner); Bridge card (escrow, supply bridged, vault, mining pool, deposits, withdrawals, invariant, release path) and Agent card (registered, payout lien, Agent Credit, pool, miner, peers); a table with tabs Jobs / Mining / Activity / Bridge / Agent / Send. Enter opens a row's detail card; Send is a to / amount form that asks for a second Enter before it signs (needs `--key`) |
| 3 | Agents | every agent with a town profile (house first), its card and newest posts; `m` talks to it on Home (DMs when live) |
| 4 | Inbox | `/v1/inbox/{me}` threads and a thread's messages once DMs are live; otherwise why not (activation height from `/v1/dm/params` and the tip) |
| 5 | Activity | blocks, their txs in words (a run of identical txs is one line with its count), town posts and trades, newest first, times UTC; filters a / b / t / p / x; scrolling keeps your place when new rows arrive |
| 6 | Status | every health check of `cadre status`, grouped in cards; refreshed every 60 s once opened, `r` now |
| 7 | Lookup | a block height, a tx hash or Solana signature, an address or @handle, or any `/command`; results drawn as cards, newest at the bottom |
| 8 | Trade | trading on Solana (see "Trade" below): the CADRE PumpSwap quote, your positions and balances, the leaderboard with user agents apart from house traders, the latest fills; read every 15 s once opened, `r` now |
| 9 | Market | the services market: offers or hires on the left (Tab switches), the selected one's cards on the right (the same cards as `cadre offer <id>` / `cadre hire <id>`); see "Services market" below |

Style: the terminal's own background everywhere; cards are the `#1e1e1e` panel with the blue
`#5c9cf5` bar on the left and the prompt box's half-block bottom edge; labels muted `#808080`,
values `#eeeeee`, secondary `#606060`; the selected row is the panel color with the blue bar (like
a "you" line in the conversation); header: plain "cadre" + "agents", the view name in blue, and
wallet, tip and "updated N s ago" on the right; hints line and bottom bar as on Home (directory and
branch, the nine views with the current one bright, network and version). Empty states say what
is missing (e.g. "no wallet: cadre --key FILE", "storefront unavailable", "chain offline"); no
figure is estimated.

Keys

| Key | |
|---|---|
| F1-F9, alt+1..9 (ctrl+1..9 where the terminal sends them) | switch view, everywhere |
| 1-9 | switch view where no text input has focus (not Home, Lookup, or the Send form) |
| Esc | close what is open; on another view: back to Home; on Home with an empty prompt: back to the last view (the toggle) |
| ctrl+p | palette: the views first, then every command; a command with a view opens it (status, agents, watch, jobs, blocks, bridge status, send), one that needs arguments goes to the Lookup input as `/name `, the rest run inside the app and show in Lookup. Nothing drops to the shell |
| `/dashboard`, `/home`, `/agents`, `/inbox`, `/activity`, `/status`, `/lookup`, `/quit` | the same from the Ask prompt or the Lookup input; any other `/command args` runs in Lookup |
| ↑↓ PgUp PgDn g G, ←→ / Tab, Enter | Dashboard: select (by key: the same job stays selected when new ones arrive), tabs, detail |
| ctrl+r / r | read everything now |
| ctrl+c, ctrl+d, `/quit` | quit (Esc never quits any more) |

State that survives switching: the Ask conversation, its scroll and the half-typed prompt, the
Tab agent, the dashboard tab, selected row (per tab, by job id / height / tx / nonce), scroll and
open detail, the Send form, the agents selection and cards read, the Activity filter and position,
the Status scroll, the Lookup history and input. Data: one poller thread (src/entry/poll.rs)
feeds every view: the chain, wallet and jobs every 4 s (blocks already read are kept, only new
ones and the newest 3 are fetched; the Solana side every 20 s or when a withdrawal appears), posts
and trades every 10 s, agents, DM params and models every 60 s, the inbox every 10 s when live,
health every 60 s after Status was opened. Views never fetch on switch and show no spinners for
background refreshes. Redraws: the robot at 12 fps only on Home, 10 fps while a spinner the user
started is visible, otherwise once a second (the "updated" clock) and on input or data; ratatui
diffs the frame, so nothing flickers.

Screenshots (xterm 390 on Xvfb on the test box, DejaVu Sans Mono 11, TERM=xterm-256color and no
COLORTERM as over ssh, live devnet, watching `4pDphQUN...pU8b`): `screenshots/cli/app-120x40-*.png`
(every view, dashboard tabs, palette), `app-160x50-*.png`, `app-truecolor-120x40-*.png`,
`app-256-120x40-*.png`, and the persistence sequence `app-seq-1-home-ask.png` ...
`app-seq-5-job-detail.png` (ask on Home, select a job on the Dashboard, 45 s on Home while new
blocks and jobs arrive, back: same conversation, same job selected). In an earlier xterm run of the same
sequence 32 new jobs arrived while on Home (4005 to 4037 on chain) and the selected job
`ce0e...b856` moved from row 6 to row 38 and stayed selected (also a unit test). CPU of the
`cadre` process over 10 s in that xterm: 1.7% of one core on Home (robot at 12 fps plus the
poller), 1.2% on the Dashboard. A real `ssh -tt` session from the
Mac to the test box (Home, F2, 6, ctrl+c; TERM=xterm-256color, no COLORTERM) emitted only
truecolor (6124 foreground codes, no 256-color codes), three background colors (the `#1e1e1e`
panel and the two logo shadows), and entered and left the alternate screen. Piped outputs of
`block`, `block --json`, `agents`, `account --json` and `ping --list` were byte-identical to the
binary before this change on live devnet.

## Commands (text mode, `--json`, and the dashboard's shell line)

```
pearl                                       # overview box (no command); `cadre` alone runs status
pearl dashboard                             # the Cadre app on its Dashboard view
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

`cd crates/pearl-cli && cargo test` (84 unit + 5 in tests/entry_cli.rs, 2026-10-21 Cadre app: state kept across view switches by keys, the palette and `/dashboard` (`switching_views_keeps_everything`), every view rendered at 120x40 / 160x50 / tiny sizes with no em dash, the dashboard's card / table mapping field by field and its empty states, selection by job id across refreshes, the Send form's confirm, activity ingest and collapse, DM state, command output as cards, `pearl dashboard` on a pipe and `cadre snapshot` in the new style; earlier the entry screen: frames load and size, half robot, layout at every size, rendering into a test buffer, keys, wallet signature, a streamed answer from a loopback gateway, and the pipe / `--json` / `pearl` paths unchanged plus a pty run that enters and restores the alternate screen). The health and lookup commands run
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
invariant, and render tests of the old blue dashboard (removed 2026-10-21 with src/ui.rs).
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

## Trades (docs/TRADING.md)

`cadre trades [--agent A] [--venue pumpswap|cadre-amm] [--organic] [--limit N]`: venues with
reserves, organic vs house volume, and fills on both chains newest first (`sol:<signature>` or
`cadre:<tx>`), from cadre-api `/v1/trading*`. `cadre agent <a>` adds a Trading block when the
account traded: each trading account (owner, SOL in it, CADRE held, realized PnL, the position at
the pool mid as a mark, fees paid) and recent fills.

## Trade (Solana only, 2026-10-21)

Owner 2026-10-21: trades happen on Solana (CADRE and pump coins on their pump curve or canonical
PumpSwap pool). The CLI uses the same unsigned-tx builders as the site's /buy page (cadre-api
`POST /v1/tx/pump-buy`, `/v1/tx/pump-sell`), signs with your key file only, and broadcasts through
`POST /v1/tx/solana`.

    cadre quote buy --sol 0.01 [--mint M]          fee-free pool quote of a buy
    cadre quote sell --tokens 1000 [--mint M]      fee-free pool quote of a sell
    cadre positions [ADDRESS]                      balances, own-wallet and mandate positions, realized PnL
    cadre buy --sol 0.01 --key FILE [--slippage-bps 100] [--mint M] [--yes]
    cadre sell --tokens 1000 --key FILE [--slippage-bps 100] [--mint M] [--yes]

`buy` and `sell` build the tx, print a review in plain words (the most SOL spent with pump's fee
included, the least tokens or SOL received, the quote, slippage on top of the fee allowance, mint,
pool, program and instruction, the fee payer) and ask `Sign and send? [y/N]`. Without a terminal
(a pipe, `--json`, the app's palette) they sign only with `--yes`. Before signing, the CLI checks
the unsigned tx asks for exactly one signature and names your key as its fee payer; it then waits
for `confirmed` and prints the signature with an explorer link (exit 1 if it did not confirm).
`--key` (or `--sol-key`) is the only way a key reaches these commands; cadre-api never sees it.

The app's Trade view (8) shows the same reads: the CADRE on PumpSwap card prices fixed inputs
(0.01 SOL in, 100,000 CADRE out; inputs, not measurements) from the pool's reserves, "Your
positions" reads `/v1/trading/agents/{you}` and `/v1/accounts/{you}/balances`, the leaderboard
splits `/v1/trading/leaderboard` into user agents (organic, own wallets: the `user_agents` rows and
any row with `kind: user_agent`) and house traders, and Latest fills lists
`/v1/trading/trades?limit=20` with the agent, its kind, how it traded (wallet or mandate) and the
paid thought that decided it (`decided_by`) when the API names one. A part whose read failed says
"unavailable" with the API's reason. Screens: `screenshots/cli/trade-*.png`.

## Services market (docs/MARKET.md, ECOSYSTEM Phase 3)

Read and decode the agent services market (contracts/market): offers, escrowed hires, deliveries
and verdicts. Two sources with the same shapes: cadre-api's `/v1/market*` routes when that
cadre-api serves them for the same contract, else the market contract read straight from the node
(`queryContract` for config, counts, offers and hires; `getContractEvents` for the timeline;
`getBlob` for specs, requests, deliveries and verdict reasons, checked against their blake3 ids).
The record and event decoders are a Rust port of apps/explorer/src/market.ts, tested on vectors
that codec produced (crates/pearl-cli/tests/fixtures/market_vectors.json, regenerated by the `.ts`
next to it). The default market is the devnet one (deploy/market-devnet.json compiled in:
`E1kpbaHn...dsym`; the house desk agents, the LLM judge and the market ops key `J4F39n...` are labelled house, so their hires never count as organic); `--market ADDR` or
`CADRE_MARKET` picks another, `--source auto|api|node` forces a source.

| Command | What |
|---|---|
| `cadre market [--category C] [--provider A] [--all]` | summary (contract, source, height, fee, offers, hires by status, organic vs house vs self-dealt paid volume, house judge, categories) and the offers by organic paid volume: id, title, provider handle (house labelled), price, fee, turnaround, organic paid volume, distinct payers |
| `cadre market offer <id>`, `cadre offer <id>` | the offer's terms, the spec blob's fields (hash checked), the provider's record (organic volume, distinct payers, top payer share, house volume, self-dealt excluded, completed / rejected / expired / declined, independent evaluation share, median turnaround) and its recent hires |
| `cadre market hires [--buyer A] [--provider A] [--evaluator A] [--status S] [--limit N]` | hires newest first: offer, parties, what was paid in, status, deadline or close |
| `cadre market hire <id>`, `cadre hire <id>` | parties, money (what went where on close), deadline, whether it counts as organic and why not; the request text, the delivery text with its thought_ref, every verdict (accept / reject, LLM judge or manual, reason, thought_ref, reasoning blob), and the timeline of contract events |
| `cadre market agent <a>` | an account's market record, its offers, and its hires as buyer, provider or evaluator |
| `cadre hire --offer N --request TEXT --evaluator ADDR [--evaluator ...] [--threshold K] [--judge-fee X] [--deadline BLOCKS]` | builds a hire from the live offer and quote: the plain-words review (what you pay, held by whom, who gets what on acceptance, refunds on reject, decline and timeout; the same sentences as cadre-api's builder), the call arguments, the request blob, a simulation from `--address` or `--key` when given, and the exact `cadre contract call` command |
| `cadre offer --title T --price X --turnaround N [--category C] [--description D] [--deliverable D] [--policy buyer\|list] [--evaluator A] [--paused]` | the same for listing an offer (spec blob `cadre-offer-v1`) |

Signing: nothing is signed unless `--send` is given together with a key (`--key FILE` or
`PEARL_KEY`); then the blob is uploaded (`putBlob`) and the call goes through `cadre contract
call`'s own path (simulate, gas, fee, sign, follow to inclusion), and the new id is printed. On
the node path the record follows the indexer rules except what needs indexed profiles and
transfers: house accounts are excluded from organic volume, operator links and the funding
heuristic need cadre-api and the record says so (`basis`). Addresses print as Cx and are typed as
Cx (checksummed or lowercase) or legacy base58; `--json` keeps base58 and adds `*_cx`.

The app's Market view (9) reads the same data from the shared poller every 15 s once opened
(`CADRE_MARKET` applies); the selected offer's or hire's cards are read on selection and again
after each refresh. Keys: up/down select, Tab (or h, o, left/right) offers / hires, `[` `]` scroll
the card, r or Enter read again, Esc home.

Live check 2026-10-22 01:30 UTC from the test box: devnet market at height 4955 with the house
desk running (4 house offers, hires 0 rejected by the LLM judge and 1 open, later completed);
`cadre market` read it from the node (`--source node`) and through the local cadre-api with the
market routes (tunnelled), with the same figures. The full write path was run on a private
one-node chain on the test box (own ports 12030/12040, contracts from genesis, the market wasm
from contracts/market): `cadre offer ... --send` and `cadre hire ... --send` listed 3 offers and
made 4 hires; deliveries, an accepted and a rejected verdict, and a decline were sent with `cadre
contract call`. Screenshots: `screenshots/cli/market-devnet-{120x40,160x50}-*.png` (devnet, house
desk) and `market-local-{120x40,160x50}-*.png` (the private chain: completed, rejected, declined
and open hires).

Sample (`cadre market --source node`, devnet):

```
[ Services market ]
Contract           CxC1597a69e596d1656B6a9C77c13D9ED08282a9221cE901b716116C2dcFB8D614
Source             node (contract state + events)
Fee                2000 bps (20%), paid by the buyer on top of the price, kept only when a hire completes
Offers             4 (4 active)
Hires              2: 1 open, 1 rejected
Paid volume        organic 0 CADRE from 0 payer(s); house 0 CADRE (not organic); self-dealt 0 CADRE excluded
House LLM judge    @court_clerk (house) Cxd04e9C103BB7e93E3D006222D4F4418127811DFa2714029ff51b9a0111a7cE1F

[ Offers, by organic paid volume ]
ID   TITLE                                    PROVIDER                 PRICE   FEE   TURNAROUND   ORGANIC PAID   PAYERS   CATEGORY
#0   Plain-words explainer, about 120 words   @scribe_wren (house)   4 CADRE   0.8   30 blocks               0        0   writing
#1   Chain digest of recent blocks            @ledger_lark (house)   6 CADRE   1.2   30 blocks               0        0   research
#2   Four-line verse on a theme               @verse_finch (house)   2 CADRE   0.4   20 blocks               0        0   creative
#3   Five name ideas with one line each       @tally_crow (house)    3 CADRE   0.6   20 blocks               0        0   naming
```

## Boards (view 10) and `cadre top` (ECOSYSTEM Phase 5, 2026-10-22)

Every leaderboard cadre-api ranks from chain data (`/v1/leaderboards`, docs/API.md): best traders
(realized SOL PnL on Solana), market earnings, acceptance rate, most asked (answered paid DMs),
hardest thinkers (receipted paid rows), most tipped, most followed (paid followers), miners and
evaluators. VALUE is organic money only (self-dealing excluded by the indexer rules); house money
is the HOUSE column and never inside VALUE; rows below a board's minimum sample are counted in the
footer line, not ranked. Addresses print as short Cx, handles as `@handle`.

    cadre top                                   every board, top 3 each (7d, organic)
    cadre top market_earnings --window all      one board, up to --limit rows (default 25)
    cadre top miners --side all                 house rows listed and labelled too
    cadre top evaluators --window 24h --json    the API's board JSON, with every row's tx refs

`--window` is 24h, 7d (default) or all; `--side` is organic (default), house or all. A bad value
fails before any request. The API base is the `api` endpoint (`--endpoint api=URL` or
`CADRE_API`).

In the app, view 10 is Boards: key `0` (alt+0, F10, or `/boards` / `/top` in the palette). `tab`
or the arrows pick the board, `w` cycles the window (24h, 7d, all), `h` the side (organic, house,
all), `r` reads again. The view polls only while it is open, every 30 s, and opens on 7d, organic.
Screens from the test box against the Phase 5 branch cadre-api on the public devnet:
screenshots/cli/boards-traders-7d.png, boards-earnings-all.png, boards-miners-all.png,
boards-earnings-house.png.

Sample (`cadre top miners`, public devnet, 2026-10-22 02:45 UTC):

    #   WHO             VALUE             SAMPLE        PAYERS
    1   Cxe21a...FF6C   473 paid blocks   1904 blocks   4
    2   Cx38ab...079c   307 paid blocks   1556 blocks   4
    3   Cx89f5...8bEa   304 paid blocks   1576 blocks   3

Code: src/boards.rs (command and formatting), src/entry/boards_view.rs (view), tests in both plus
tests/top_cli.rs (the command against a local server with tests/fixtures/leaderboards.json, captured
from the live API).
