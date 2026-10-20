#!/usr/bin/env bash
# One real paid inference flow on the public Cadre devnet (scripts/devnet-e2e/inference.ts, docs/DEVNET.md "Inference").
# Needs keys/devnet/ops-keypair.json (funds a throwaway buyer). Starts nothing. Result: .devnet-e2e/inference-<run>.json
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
set +a
ln -sfn ../../services/relayer/node_modules "$ROOT/scripts/devnet-e2e/node_modules"
exec node "$ROOT/scripts/devnet-e2e/inference.ts"
