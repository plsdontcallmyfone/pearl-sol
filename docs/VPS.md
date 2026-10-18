# Test VPS (pearl-sol-test)

Owner-approved test box (2026-10-17) that runs the whole local stack (`scripts/demo.sh`) off the
Mac. Everything on it is a local stack: no devnet or mainnet, no public-chain sends.

## Box

| | |
|---|---|
| Provider | BitLaunch, name `pearl-sol-test` |
| Address | 64.95.13.106 (the other box, 64.94.84.126 "instance-backend", is NOT ours: never touch it) |
| OS | Ubuntu 24.04.4 LTS, x86_64, kernel 6.8 |
| Size | 4 vCPU, 8 GB RAM + 8 GB swapfile (`/swapfile`, in fstab), 150 GB disk |
| Cost | $0.156/hr (about $3.74/day, $114/month) |
| SSH | `ssh -i ~/.ssh/pearl_vps -o IdentitiesOnly=yes pearl@64.95.13.106` (root works with the same key; use `pearl` for everything) |
| Firewall | ufw, default deny incoming, only 22/tcp open |
| Repo | `/home/pearl/pearl-sol` (clone of a git bundle of `main`; committed files only) |

All agent-chain nodes and services bind 127.0.0.1. One exception: `solana-test-validator` has no
RPC bind flag, so its RPC/WS/faucet (10600-10602) listen on 0.0.0.0; ufw drops them (checked from
outside: 10600, 10601, 10602, 10630 all filtered). Do not open ufw ports; tunnel instead.

## Installed (user `pearl`, matching the Mac)

| tool | version | how |
|---|---|---|
| Rust default | 1.94.1 (= Mac stable) | rustup, `rustup default 1.94.1` |
| Rust pinned | 1.95.0 | chain/ and crates/pearl-cli rust-toolchain.toml |
| Agave / solana CLI | 3.1.12, cargo-build-sbf 3.1.12, platform-tools v1.52 | release.anza.xyz/v3.1.12/install |
| Anchor | anchor-cli 0.31.1 | avm (0.31.1), `avm use 0.31.1` |
| Node / npm | v22.22.1 / 10.9.4 | nodejs.org tarball in `~/.local/node` |
| apt | build-essential, pkg-config, libssl-dev, libudev-dev, clang 18, cmake, git, protobuf-compiler 3.21, python3, lsof, jq | |

PATH lines sit at the top of `~/.bashrc` (marked `# pearl-sol-path`) so non-interactive
`ssh pearl@box 'cmd'` sees cargo, solana, anchor and node.

`research/pearlrepo` is not needed for any build. `token/target/deploy/*-keypair.json` were copied
from `keys/localnet/` (program ids unchanged); never generate new ones.

## Build times (first build on the box, one at a time)

| build | time |
|---|---|
| chain/ agent-node (1.95.0) | 196 s |
| crates/ pearl-job, pearl-miner, pearl-attester | 34 s |
| crates/pearl-cli | 84 s |
| services/header-relay | 186 s |
| token/ `anchor build` (4 programs) | 352 s |
| npm ci relayer, crank, storefront-x402 | 13 s |

Peak memory stayed well under 8 GB; swap untouched. The build script used is `~/build.sh` on the
box (the same order as above). After a sync, `scripts/demo.sh` rebuilds the Rust parts if
stale; rebuild the token programs by hand (`cd token && anchor build`) when token/programs change.

## Demo result (2026-10-18)

`scripts/demo.sh` on the box: 10/10 PASS, 468 s wall clock (builds were fresh). A second run with
`DEMO_KEEP_UP=1`: 9/9 PASS, 419 s, stack left up. No Linux fixes were needed in the repo.

## Sync updates (Mac -> box)

Commit on the Mac first (only committed history travels), then:

    scripts/vps.sh sync

which is

    git bundle create /tmp/pearl-sol.bundle main
    scp -i ~/.ssh/pearl_vps -o IdentitiesOnly=yes /tmp/pearl-sol.bundle pearl@64.95.13.106:pearl-sol.bundle
    ssh -i ~/.ssh/pearl_vps -o IdentitiesOnly=yes pearl@64.95.13.106 'cd ~/pearl-sol && git pull --ff-only ~/pearl-sol.bundle main'

Nothing from `~/.config`, `~/.ssh` or other repos is copied. Do not edit files on the box: fix on
the Mac, commit, sync.

## Start / stop the stack

    scripts/vps.sh demo    # DEMO_KEEP_UP=1 scripts/demo.sh on the box, detached; follows ~/demo.log to the end
    scripts/vps.sh down    # scripts/demo.sh down on the box (kills by recorded PID)
    scripts/vps.sh ssh     # shell as pearl

A plain pass/fail run that tears down after itself: `scripts/vps.sh ssh 'cd ~/pearl-sol && bash scripts/demo.sh'`.
Stop the stack when idle (`down`); the box itself bills while it exists.

## Tunnel in from the Mac

The Mac's own demo stack uses 10600-10669, so the helper maps remote 106xx to local 206xx:

    scripts/vps.sh tunnel  # remote 10600 10601 10630-10634 10660-10663 -> local 20600 ... 20663

or by hand:

    ssh -i ~/.ssh/pearl_vps -o IdentitiesOnly=yes -N \
      -L 127.0.0.1:20600:127.0.0.1:10600 -L 127.0.0.1:20601:127.0.0.1:10601 \
      -L 127.0.0.1:20630:127.0.0.1:10630 -L 127.0.0.1:20631:127.0.0.1:10631 \
      -L 127.0.0.1:20660:127.0.0.1:10660 -L 127.0.0.1:20662:127.0.0.1:10662 \
      pearl@64.95.13.106

(`VPS_PORT_OFFSET=0 scripts/vps.sh tunnel` maps 1:1 when no local stack is up.) Then the local
CLI against the box, read-only:

    crates/pearl-cli/target/release/pearl --rpc http://127.0.0.1:20630 --sol-rpc http://127.0.0.1:20600 \
      --pool http://127.0.0.1:20663 --address <ADDR> balance

Writes need the box's demo keys (`.demo/keys/*.json`, regenerated every run, they stay on the
box); run those on the box instead:

    scripts/vps.sh ssh 'cd ~/pearl-sol && crates/pearl-cli/target/release/pearl --key .demo/keys/user.json balance'

Checked 2026-10-18 on the kept-up stack: `balance`, `send <lender> 1.5` (FINAL at block 486),
`bridge status` (invariant OK, delta 0), and the local CLI through the tunnel.
