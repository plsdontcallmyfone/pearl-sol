#!/usr/bin/env bash
# End to end check of the public Cadre devnet from this machine (docs/DEVNET.md).
#   scripts/devnet-e2e.sh        run scripts/devnet-e2e/run.ts against the public endpoints
# Needs keys/devnet/ops-keypair.json (funded) and crates/target/release/pearl-job. Starts nothing.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
set +a
ln -sfn ../../services/relayer/node_modules "$ROOT/scripts/devnet-e2e/node_modules"
exec node "$ROOT/scripts/devnet-e2e/run.ts"
