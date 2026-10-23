#!/usr/bin/env bash
# Every Node suite in the repo, from a clean install (CI runs exactly this; so can you).
# Preconditions, both built by CI's earlier jobs:
#   token/target/deploy/*.so + token/target/idl/*.json   (cd token && anchor build, keys from keys/localnet)
#   the pearl-job CLI: crates/target/release/pearl-job, or PEARL_JOB_BIN=<path>
# There is no npm workspace: each package with a package-lock.json is installed on its own.
# SKIP_INSTALL=1 reuses existing node_modules. Exit status is non-zero if any suite failed.
set -uo pipefail
cd "$(dirname "$0")/../.."
INSTALL=(extension services/cadre-api services/crank services/relayer services/storefront-x402
  services/swarm/desktop services/swarm/trading token/tests/mandate token/tests/reserve
  token/tests/storefront token/tests/town tools/defillama)
if [ "${SKIP_INSTALL:-}" != 1 ]; then
  for d in "${INSTALL[@]}"; do
    echo "npm ci: $d"
    (cd "$d" && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund --loglevel=error) || exit 1
  done
fi
SUMMARY=$(mktemp)
fail=0
run() {
  local d=$1; shift
  echo "::group::$d: $*"
  local t0=$SECONDS r
  if (cd "$d" && "$@"); then r=ok; else r=FAIL; fail=1; fi
  echo "::endgroup::"
  printf '%-26s %-24s %-4s %4ss\n' "$d" "$*" "$r" $((SECONDS - t0)) >> "$SUMMARY"
}
run apps/explorer            npm test
run packages/cadre-mcp       npm test
run packages/cadre-sdk       npm test
run packages/cx-address      npm test
run services/agent-dump      npm test
run services/cadre-api       npm run typecheck
run services/cadre-api       npm test
run services/cadre-proxy     npm test
run services/crank           npm run typecheck
run services/feed-bot        npm test
run services/relayer         npm run typecheck
run services/relayer         npm test
run services/storefront-x402 npm run typecheck
run services/storefront-x402 npm test
run services/swarm           npm test
run services/swarm/agents    npm test
run services/swarm/desktop   npm test
run services/swarm/market    npm test
run services/swarm/strategies npm test
run services/swarm/trading   npm test
run token/tests/mandate      npm test
run token/tests/reserve      npm test
run token/tests/storefront   npm test
run token/tests/town         npm test
run tools/defillama          npm test
run extension                npm run typecheck
run extension                npm test
run extension                node scripts/build.mjs
echo; cat "$SUMMARY"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then { echo '```'; cat "$SUMMARY"; echo '```'; } >> "$GITHUB_STEP_SUMMARY"; fi
exit $fail
