# Mining Cadre

One command builds `pearl-miner` from source, makes a payout key and installs a service:

    curl -fsSL <site>/install-miner.sh | bash                       # solo on the public devnet
    curl -fsSL <site>/install-miner.sh | bash -s -- --pool URL --threads 4
    bash scripts/install-miner.sh --rpc http://127.0.0.1:10630 --no-service   # from a checkout

Script: `scripts/install-miner.sh` (`--help` lists everything; the site serves a copy at
https://cadre.64-95-12-116.sslip.io/install-miner.sh, and `| bash -s -- --dry-run` shows every
step without changing anything). Runs on Ubuntu 22.04 / 24.04
(x86_64, aarch64) and macOS arm64 (no service there: it prints the run command). Safe to run
again: it never replaces the key, keeps earlier config values unless a flag changes them, and
reuses the build cache.

| flag | env | default |
|---|---|---|
| `--pool URL` | `CADRE_POOL` | none (solo) |
| `--solo` | | solo; clears a pool set earlier |
| `--rpc URL` | `CADRE_RPC` | `https://rpc.64-95-12-116.sslip.io` (devnet node 0, docs/DEVNET.md) |
| `--threads N` | `CADRE_THREADS` | half the cores, at least 1 |
| `--no-service` | `CADRE_NO_SERVICE=1` | service installed when `systemctl --user` works |
| `--dry-run` | `CADRE_DRY_RUN=1` | prints each step, changes nothing |

Source: `CADRE_REPO` (git URL, default github.com/plsdontcallmyfone/pearl-sol, public since
2026-10-20, so no account or token is needed) at `CADRE_REF` (default `main`), or `CADRE_SRC` = a
local checkout or tarball (`git archive --format=tar.gz -o cadre.tgz HEAD`) for offline boxes. Build: Rust 1.95.0 via rustup (an extra pinned
toolchain; the default one is not changed), `cargo +1.95.0 build --release --locked -p
pearl-miner` in `crates/`.

What goes where (`CADRE_HOME`, default `~/.cadre`):

| path | |
|---|---|
| `bin/pearl-miner` | the miner (services/miner) |
| `bin/cadre-miner` | runner: reads `miner.env`, execs pearl-miner |
| `miner.env` | `CADRE_RPC`, `CADRE_POOL` (empty = solo), `CADRE_THREADS`, `CADRE_KEY`, `CADRE_STATS_SECS`, `CADRE_EXTRA` (extra pearl-miner flags) |
| `miner-key.json` | payout key, 0600, Solana JSON array (64-byte keypair when `openssl` has ed25519, else the 32-byte seed); never printed. Only the address is shown. Back it up |
| `src/`, `target/` | source and build cache (`CARGO_TARGET_DIR` overrides target) |
| `results/`, `models/` | miner working dirs |
| `~/.config/systemd/user/cadre-miner.service` | user service (Nice 10, restart always) |

Run it:

    systemctl --user status cadre-miner
    journalctl --user -u cadre-miner -f          # one JSON line per event: start, stats, block_accepted
    systemctl --user stop cadre-miner            # disable: systemctl --user disable --now cadre-miner
    sudo loginctl enable-linger $USER            # keep mining after logout

Without systemd: `~/.cadre/bin/cadre-miner` in the foreground (Ctrl-C stops), or
`nohup ~/.cadre/bin/cadre-miner >> ~/.cadre/miner.log 2>&1 & echo $! > ~/.cadre/miner.pid`,
stop with `kill "$(cat ~/.cadre/miner.pid)"`.

Solo mode mines blocks straight against the node (`getBlockTemplate` / `submitBlock`, docs/RPC.md);
pool mode sends shares to a `pearl-pool` (services/pool) that pays the key's address, and uses
the node only to fetch `blob:` job data.

## What a fresh install costs (measured 2026-10-20, public repo at main)

| box | wall time | build step | disk | build peak memory |
|---|---|---|---|---|
| test box, Ubuntu 24.04 x86_64, 4 vCPU / 8 GB, load about 17 from another stack, no Rust before | 128 s | 114 s | 1.1 GB | 776 MiB PSS summed over the install tree (largest single process 562 MiB RSS) |
| fresh `ubuntu:24.04` aarch64 container (Docker Desktop on Apple silicon, 10 cores), apt deps and Rust installed by the script | 40 s | 16 s | 1.0 GB | not measured |
| macOS 26 arm64, Rust 1.95.0 already present | 21 s | 16 s | | |

Disk splits as Rust toolchain about 585 MB (`~/.rustup`), cargo registry 213 MB (`~/.cargo`), build
cache 252 MB (`~/.cadre/target`), source 44 MB, binary 5 MB. A running miner is small: 6.5 MB peak
RSS with 2 threads; each thread uses one core. Measured throughput: 5,500 nonces/s on 2 threads on
the loaded x86 box, 23,660 nonces/s on 5 threads on Apple silicon. The script refuses to start with
under 3 GB free.

## Is it working?

The log has a `start` line with the payout address, a `stats` line every `CADRE_STATS_SECS`
(`nonces_per_s`, `found`, `accepted`) and a `block_accepted` line (height, block hash) per block won.
The installer prints the two checks at the end; on the devnet:

    curl -s https://api.64-95-12-116.sslip.io/v1/miners/<address>     # blocks: paid_job, fallback; subsidy, matured, immature
    https://explorer.64-95-12-116.sslip.io/block/<height>              # "miner" is your address

What a block earns: a block on a paid AI job earns a subsidy plus fees; with no job open the miner
mines fallback blocks, which earn only the fees of the transactions in them (often 0). Either way the
payout matures `finality_k` (30) blocks later. To spend it, import `miner-key.json` into the Cadre
Wallet (it takes the 32- and 64-byte forms) or use it as `pearl --key` (docs/CLI.md).

## Stop and uninstall

    systemctl --user disable --now cadre-miner               # or: kill "$(cat ~/.cadre/miner.pid)"
    rm ~/.config/systemd/user/cadre-miner.service && systemctl --user daemon-reload
    mv ~/.cadre/miner-key.json ~/cadre-miner-key.backup.json # keep the key if it holds rewards
    rm -rf ~/.cadre
    rustup toolchain uninstall 1.95.0                         # only if nothing else uses it
