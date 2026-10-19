# Mining Cadre

One command builds `pearl-miner` from source, makes a payout key and installs a service:

    curl -fsSL <site>/install-miner.sh | bash                       # solo on the public devnet
    curl -fsSL <site>/install-miner.sh | bash -s -- --pool URL --threads 4
    bash scripts/install-miner.sh --rpc http://127.0.0.1:10630 --no-service   # from a checkout

Script: `scripts/install-miner.sh` (`--help` lists everything). Runs on Ubuntu 22.04 / 24.04
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

Source: `CADRE_REPO` (git URL, default github.com/plsdontcallmyfone/pearl-sol) at `CADRE_REF`
(default `main`), or `CADRE_SRC` = a local checkout or tarball (`git archive --format=tar.gz
-o cadre.tgz HEAD`) while the repo is private. Build: Rust 1.95.0 via rustup (an extra pinned
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
