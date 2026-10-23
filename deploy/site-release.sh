#!/usr/bin/env bash
# Build a new release of the Cadre website and switch to it (scripts/devnet-deploy.sh site runs this
# as lode on box 0, after pulling ~/cadre-web to the bundled main). Each deploy builds in its own
# release dir ($RELEASES/<commit>-<time>) while the running server keeps serving the previous
# release untouched; the previous release's .next/static is copied in (no overwrite), so a page
# still open from the old build finds its script chunks (before 2026-10-23 a rebuild under the live
# server gave 500s for a few minutes); then $CURRENT is switched atomically (symlink + rename) and
# the caller restarts cadre-web. The $KEEP newest releases are kept. A failed build changes nothing.
#   deploy/site-release.sh <api-url> <explorer-url>
# Env (tests): SRC (~/cadre-web), RELEASES (~/cadre-web-releases), CURRENT (~/cadre-web-current),
# KEEP (3), CADRE_SOL (~/site-installer), NPM (npm).
set -euo pipefail
API_URL="$1"; EXPLORER_URL="$2"
SRC="${SRC:-$HOME/cadre-web}"; RELEASES="${RELEASES:-$HOME/cadre-web-releases}"; CURRENT="${CURRENT:-$HOME/cadre-web-current}"
KEEP="${KEEP:-3}"; NPM="${NPM:-npm}"
export CADRE_SOL="${CADRE_SOL:-$HOME/site-installer}" NEXT_TELEMETRY_DISABLED=1 PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
sha="$(git -C "$SRC" rev-parse --short=9 HEAD)"
rel="$RELEASES/$sha-$(date -u +%Y%m%d%H%M%S)"
mkdir -p "$RELEASES"; rm -rf "$rel.tmp"; mkdir "$rel.tmp"
trap 'rm -rf "$rel.tmp"' EXIT
git -C "$SRC" archive HEAD | tar -x -C "$rel.tmp"
echo "release $(basename "$rel") from $(git -C "$SRC" log --oneline -1)"
cd "$rel.tmp"
"$NPM" ci --no-audit --no-fund >/dev/null
NEXT_PUBLIC_CADRE_API="$API_URL" NEXT_PUBLIC_CADRE_NETWORK=devnet NEXT_PUBLIC_CADRE_EXPLORER="$EXPLORER_URL" "$NPM" run build 2>&1 | tail -25
test -s .next/BUILD_ID || { echo "build produced no .next/BUILD_ID; live site unchanged" >&2; exit 1; }
# the live release (or, the first time, the old in-place build in $SRC)
if [[ -L $CURRENT ]]; then prev="$(readlink -f "$CURRENT")"; else prev="$SRC"; fi
# (only chunks built in the last 7 days, so carried files do not pile up forever; file by file: `cp -n` exits non-zero on a skipped file in coreutils 9.2+ and on macOS)
if [[ -d $prev/.next/static ]]; then
  (cd "$prev/.next/static" && find . -type f -mtime -7) | while IFS= read -r f; do
    [[ -e .next/static/$f ]] || { mkdir -p ".next/static/$(dirname "$f")" && cp -p "$prev/.next/static/$f" ".next/static/$f"; }
  done
fi
cd /
mv "$rel.tmp" "$rel"
ln -sfn "$rel" "$CURRENT.new"
mv -fT "$CURRENT.new" "$CURRENT" 2>/dev/null || { python3 -c 'import os,sys; os.replace(sys.argv[1], sys.argv[2])' "$CURRENT.new" "$CURRENT"; }
# keep the newest KEEP releases (the new one always among them)
ls -1dt "$RELEASES"/*/ 2>/dev/null | sed 's:/$::' | grep -v '\.tmp$' | tail -n +"$((KEEP + 1))" | while read -r d; do [[ $d != "$rel" ]] && rm -rf "$d"; done
echo "live: $(readlink "$CURRENT")"
