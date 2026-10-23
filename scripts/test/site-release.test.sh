#!/usr/bin/env bash
# Tests for deploy/site-release.sh with a stub npm (no network, no real build):
#   bash scripts/test/site-release.test.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; SCRIPT="$HERE/../../deploy/site-release.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fails=0
check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }

# a source checkout with an old in-place build (what box 0 had before releases)
SRC="$T/cadre-web"; mkdir -p "$SRC"
git -C "$SRC" init -q && echo '{}' > "$SRC/package.json" && git -C "$SRC" add . && git -C "$SRC" -c user.email=t@t -c user.name=t commit -qm one
mkdir -p "$SRC/.next/static/chunks" && echo old > "$SRC/.next/static/chunks/inplace-old.js"
# stub npm: `ci` makes node_modules, `run build` writes BUILD_ID and one chunk named by the build
cat > "$T/npm" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  ci) mkdir -p node_modules ;;
  run) [[ ${NPM_FAIL:-0} == 1 ]] && { echo "build failed"; exit 1; }
       [[ -n ${NEXT_PUBLIC_CADRE_API:-} && -n ${CADRE_SOL:-} ]] || { echo "env missing"; exit 1; }
       id="b$(date +%s%N)"; mkdir -p .next/static/chunks; echo "$id" > .next/BUILD_ID; echo x > ".next/static/chunks/$id.js"
       echo same-name-new > .next/static/chunks/shared.js ;;
esac
EOF
chmod +x "$T/npm"
export SRC RELEASES="$T/releases" CURRENT="$T/current" NPM="$T/npm" CADRE_SOL="$T/installer" KEEP=2
run() { bash "$SCRIPT" https://api.example https://explorer.example > "$T/out" 2>&1; }

echo same-name-old > "$SRC/.next/static/chunks/shared.js"
run; rc=$?
check "first release builds and switches" "[[ $rc == 0 && -L $CURRENT && -s $CURRENT/.next/BUILD_ID ]]"
check "first release keeps the old in-place chunks" "[[ -f $CURRENT/.next/static/chunks/inplace-old.js ]]"
check "the new build's files win over old ones of the same name" "[[ \$(cat $CURRENT/.next/static/chunks/shared.js) == same-name-new ]]"
r1="$(readlink "$CURRENT")"; c1="$(ls "$CURRENT/.next/static/chunks" | grep '^b')"
sleep 1; run; rc=$?
r2="$(readlink "$CURRENT")"
check "second release switches the symlink" "[[ $rc == 0 && $r2 != $r1 ]]"
check "second release carries the first release's chunk" "[[ -f $r2/.next/static/chunks/$c1 ]]"
check "the first release dir is still there (old server keeps serving it until restart)" "[[ -d $r1 ]]"
sleep 1; NPM_FAIL=1 run; rc=$?
check "a failed build exits non-zero" "[[ $rc != 0 ]]"
check "a failed build leaves the live release as it was" "[[ \$(readlink $CURRENT) == $r2 ]]"
check "a failed build leaves no .tmp dir" "! ls -d $RELEASES/*.tmp >/dev/null 2>&1"
sleep 1; run; rc=$?
check "after three good builds only KEEP=2 releases remain" "[[ $rc == 0 && \$(ls -1d $RELEASES/*/ | wc -l | tr -d ' ') == 2 && ! -d $r1 && -d $r2 ]]"
check "no stray .new symlink" "[[ ! -e $CURRENT.new ]]"
[[ $fails == 0 ]] && echo "ALL PASS" || { echo "FAILED $fails"; cat "$T/out"; }
exit $fails
