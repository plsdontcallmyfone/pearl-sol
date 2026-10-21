#!/usr/bin/env bash
# Cadre miner installer: builds pearl-miner from source, makes a payout key, writes a config
# and (where systemd --user works) installs a user service. Safe to run again: the key is never
# replaced, the config keeps earlier values unless a flag changes them, the build is cached.
#
#   curl -fsSL <site>/install-miner.sh | bash
#   curl -fsSL <site>/install-miner.sh | bash -s -- --pool https://pool.example --threads 4
#   bash scripts/install-miner.sh --solo --rpc http://127.0.0.1:10630 --no-service
#
# Flags (each also has an env form):
#   --pool URL       mine for a pearl-pool (CADRE_POOL); shares there, the pool pays the key
#   --solo           mine solo against the node (default; clears a pool set earlier)
#   --rpc URL[,URL]  agent-chain node JSON-RPC (CADRE_RPC); repeat or comma-separate for several
#                    nodes: the miner fails over between them. Default: the 3 Cadre devnet nodes.
#                    Given URLs come first; the devnet nodes are added when they serve the devnet.
#   --threads N      mining threads (CADRE_THREADS); default: half the cores, at least 1
#   --no-service     do not install or start the systemd user service (CADRE_NO_SERVICE=1)
#   --no-linger      do not enable lingering (CADRE_NO_LINGER=1); by default the service keeps
#                    running after logout when `sudo -n` works (root or passwordless sudo),
#                    otherwise the command is printed
#   --cli            also build the Cadre CLI (CADRE_CLI=1): bin/cadre (and bin/pearl, the same
#                    program), `cadre` = health of every component, `cadre --help` for the rest
#   --dry-run        show every step and command, change nothing (CADRE_DRY_RUN=1)
#   -h, --help
# Source (one of):
#   CADRE_SRC=PATH   local checkout dir or tarball (.tar, .tar.gz, .tgz; e.g. a `git archive`)
#   CADRE_REPO=URL   git URL (default below), CADRE_REF=branch|tag|commit (default main)
# Layout: CADRE_HOME (default ~/.cadre): bin/pearl-miner, bin/cadre-miner (runner), miner.env,
#   miner-key.json (0600, never printed), src/, target/ (or CARGO_TARGET_DIR), results/, models/.
#
# The private key is never printed, logged or passed on a command line. Nothing here touches
# solana config, ~/.config/solana, or any process this script did not start.
set -euo pipefail

main() {
  DEFAULT_RPC="https://rpc.64-95-12-116.sslip.io"
  # The three public devnet nodes (docs/DEVNET.md), one chain: written together so pearl-miner
  # fails over between them. DEVNET_GENESIS tells whether a given --rpc serves this chain.
  DEVNET_RPCS=("$DEFAULT_RPC" "https://rpc.64-95-13-213.sslip.io" "https://rpc.64-95-12-22.sslip.io")
  DEVNET_GENESIS="51c86b676bfd1b9f0877e6994cff95ab92cc1f2c784b39fc943b8b0d4d52028a"
  DEFAULT_API="https://api.64-95-12-116.sslip.io"   # cadre-api of the same devnet (miner lookups)
  DEFAULT_REPO="https://github.com/plsdontcallmyfone/pearl-sol.git"
  TOOLCHAIN="1.95.0"
  CADRE_HOME="${CADRE_HOME:-$HOME/.cadre}"
  CADRE_REPO="${CADRE_REPO:-$DEFAULT_REPO}"
  CADRE_REF="${CADRE_REF:-main}"
  DRY="${CADRE_DRY_RUN:-0}"
  NO_SERVICE="${CADRE_NO_SERVICE:-0}"
  NO_LINGER="${CADRE_NO_LINGER:-0}"
  WITH_CLI="${CADRE_CLI:-0}"
  local opt_rpc="${CADRE_RPC:-}" opt_pool="${CADRE_POOL:-}" opt_threads="${CADRE_THREADS:-}" opt_solo=0

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --pool) [[ $# -ge 2 && -n "$2" ]] || die "--pool needs a URL" "example: --pool http://127.0.0.1:10663"; opt_pool="$2"; shift 2 ;;
      --pool=*) opt_pool="${1#*=}"; shift ;;
      --solo) opt_solo=1; shift ;;
      --rpc) [[ $# -ge 2 && -n "$2" ]] || die "--rpc needs a URL" "example: --rpc $DEFAULT_RPC"; opt_rpc="${opt_rpc:+$opt_rpc,}$2"; shift 2 ;;
      --rpc=*) opt_rpc="${opt_rpc:+$opt_rpc,}${1#*=}"; shift ;;
      --threads) [[ $# -ge 2 ]] || die "--threads needs a number" "example: --threads 4"; opt_threads="$2"; shift 2 ;;
      --threads=*) opt_threads="${1#*=}"; shift ;;
      --no-service) NO_SERVICE=1; shift ;;
      --no-linger) NO_LINGER=1; shift ;;
      --cli) WITH_CLI=1; shift ;;
      --dry-run) DRY=1; shift ;;
      -h|--help) sed -n '2,24p' "${BASH_SOURCE[0]:-/dev/null}" 2>/dev/null | sed 's/^# \{0,1\}//' || true; [[ -f "${BASH_SOURCE[0]:-}" ]] || echo "see the header of install-miner.sh"; exit 0 ;;
      *) die "unknown option: $1" "run with --help for the list" ;;
    esac
  done
  [[ $opt_solo == 1 && -n "$opt_pool" ]] && die "--solo and --pool together" "pick one"
  if [[ -n "$opt_threads" ]] && ! [[ "$opt_threads" =~ ^[1-9][0-9]*$ ]]; then
    die "--threads must be a positive integer (got '$opt_threads')" "example: --threads 2"
  fi

  BIN="$CADRE_HOME/bin"; SRC="$CADRE_HOME/src"; ENV_FILE="$CADRE_HOME/miner.env"
  KEY="$CADRE_HOME/miner-key.json"; RUNNER="$BIN/cadre-miner"
  TARGET="${CARGO_TARGET_DIR:-$CADRE_HOME/target}"
  T0=$(date +%s); STEP=0; NSTEPS=8
  [[ $WITH_CLI == 1 ]] && NSTEPS=$((NSTEPS + 1))
  [[ $DRY == 1 ]] && echo "Dry run: nothing will be changed."

  step "Checking the system"
  check_system

  step "Installing build dependencies"
  install_deps

  step "Installing Rust $TOOLCHAIN (rustup)"
  install_rust

  step "Fetching the source"
  fetch_source

  step "Building pearl-miner (release; first build takes a few minutes)"
  build_miner

  if [[ $WITH_CLI == 1 ]]; then
    step "Building the Cadre CLI (cadre)"
    build_cli
  fi

  step "Payout key"
  make_key

  step "Writing the config"
  write_config "$opt_rpc" "$opt_pool" "$opt_threads" "$opt_solo"

  step "Service"
  install_service

  print_check
  echo
  ok "Done in $(( $(date +%s) - T0 )) s."
}

# ---------- output helpers ----------
step() { STEP=$((STEP + 1)); printf '\n[%d/%d] %s\n' "$STEP" "$NSTEPS" "$*"; }
ok()   { printf '  ok    %s\n' "$*"; }
info() { printf '  ..    %s\n' "$*"; }
warn() { printf '  warn  %s\n' "$*" >&2; }
die()  { printf '\n  FAILED  %s\n' "$1" >&2; [[ -n "${2:-}" ]] && printf '  fix     %s\n' "$2" >&2; exit 1; }
run()  {
  if [[ $DRY == 1 ]]; then printf '  would run: %s\n' "$*"; return 0; fi
  "$@"
}
have() { command -v "$1" >/dev/null 2>&1; }

# ---------- 1. system ----------
check_system() {
  OS="$(uname -s)"; ARCH="$(uname -m)"
  case "$OS/$ARCH" in
    Linux/x86_64|Linux/aarch64) ;;
    Darwin/arm64) ;;
    *) die "unsupported platform $OS/$ARCH" "supported: Ubuntu 22.04/24.04 x86_64 (aarch64 works too), macOS arm64" ;;
  esac
  local distro="unknown"
  if [[ -r /etc/os-release ]]; then distro="$(. /etc/os-release; echo "${PRETTY_NAME:-$ID}")"; fi
  [[ $OS == Darwin ]] && distro="macOS $(sw_vers -productVersion 2>/dev/null || echo '?')"
  ok "$distro, $ARCH, $(cpu_count) cores"
  [[ ${EUID:-$(id -u)} -eq 0 ]] && warn "running as root: everything goes under $CADRE_HOME (a normal user is better)"
  have curl || die "curl is missing" "install curl first (apt-get install -y curl)"
  local parent="$CADRE_HOME"; while [[ ! -d "$parent" ]]; do parent="$(dirname "$parent")"; done
  local free_kb; free_kb=$(df -Pk "$parent" | awk 'NR==2{print $4}')
  if [[ -n "$free_kb" ]]; then
    (( free_kb < 3 * 1024 * 1024 )) && die "only $((free_kb / 1024)) MB free under $parent; the build needs about 3 GB" "free some disk, or set CADRE_HOME / CARGO_TARGET_DIR to a bigger disk"
    ok "$((free_kb / 1024 / 1024)) GB free under $parent"
  fi
}

cpu_count() { if have nproc; then nproc; else sysctl -n hw.ncpu 2>/dev/null || echo 1; fi; }

# ---------- 2. deps ----------
install_deps() {
  if [[ $OS == Darwin ]]; then
    if ! xcode-select -p >/dev/null 2>&1; then
      die "Xcode command line tools are missing (C compiler, git)" "run: xcode-select --install, then run this installer again"
    fi
    for t in cc git; do have "$t" || die "$t is missing" "run: xcode-select --install"; done
    ok "Xcode command line tools present"
    return
  fi
  local pkgs=(build-essential pkg-config ca-certificates curl git openssl tar gzip)
  if have dpkg-query; then
    local missing=()
    for p in "${pkgs[@]}"; do dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q "install ok installed" || missing+=("$p"); done
    if [[ ${#missing[@]} -eq 0 ]]; then ok "all present: ${pkgs[*]}"; return; fi
    local sudo=""
    if [[ ${EUID:-$(id -u)} -ne 0 ]]; then
      have sudo || die "need root to install: ${missing[*]}" "run as root, or install them yourself: apt-get install -y ${missing[*]}"
      sudo="sudo"
    fi
    info "installing: ${missing[*]}"
    if [[ $DRY == 1 ]]; then run $sudo apt-get install -y --no-install-recommends "${missing[@]}"; return; fi
    $sudo env DEBIAN_FRONTEND=noninteractive apt-get update -qq || die "apt-get update failed" "check the network and /etc/apt/sources.list, then retry"
    $sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${missing[@]}" >/dev/null \
      || die "apt-get install failed" "run: sudo apt-get install -y ${missing[*]}"
    ok "installed: ${missing[*]}"
  else
    warn "no apt here: make sure a C compiler, git, curl, openssl, tar are installed"
    for t in cc git tar; do have "$t" || die "$t is missing" "install it with your package manager"; done
  fi
}

# ---------- 3. rust ----------
install_rust() {
  export PATH="$HOME/.cargo/bin:$PATH"
  if ! have rustup; then
    info "rustup not found: installing it (default toolchain $TOOLCHAIN, minimal profile)"
    if [[ $DRY == 1 ]]; then run "curl https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain $TOOLCHAIN"; return; fi
    curl -sSf --proto '=https' --tlsv1.2 https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain "$TOOLCHAIN" --no-modify-path >/dev/null \
      || die "rustup install failed" "see https://rustup.rs, install it by hand, then run this again"
  fi
  # A pinned extra toolchain only; the machine's default toolchain is left alone.
  if rustup toolchain list 2>/dev/null | grep -q "^$TOOLCHAIN"; then
    ok "toolchain $TOOLCHAIN present"
  else
    run rustup toolchain install "$TOOLCHAIN" --profile minimal >/dev/null || die "rustup could not install $TOOLCHAIN" "run: rustup toolchain install $TOOLCHAIN --profile minimal"
    ok "toolchain $TOOLCHAIN installed"
  fi
  [[ $DRY == 1 ]] || ok "$(cargo +"$TOOLCHAIN" --version)"
}

# ---------- 4. source ----------
fetch_source() {
  run mkdir -p "$CADRE_HOME"
  if [[ -n "${CADRE_SRC:-}" ]]; then
    [[ -e "$CADRE_SRC" ]] || die "CADRE_SRC=$CADRE_SRC does not exist" "point CADRE_SRC at a checkout dir or a .tar/.tar.gz of the repo"
    info "local source: $CADRE_SRC"
    if [[ $DRY == 1 ]]; then run "replace $SRC with a copy of $CADRE_SRC"; return; fi
    local stage="$CADRE_HOME/src.new"
    rm -rf "$stage"; mkdir -p "$stage"
    if [[ -d "$CADRE_SRC" ]]; then
      (cd "$CADRE_SRC" && tar -cf - --exclude=./target --exclude='*/target' --exclude=node_modules --exclude=.git .) | tar -xf - -C "$stage" \
        || die "copying $CADRE_SRC failed" "check read access to it"
    else
      case "$CADRE_SRC" in
        *.tar.gz|*.tgz) tar -xzf "$CADRE_SRC" -C "$stage" ;;
        *.tar) tar -xf "$CADRE_SRC" -C "$stage" ;;
        *) die "CADRE_SRC is a file but not .tar/.tar.gz/.tgz" "make one with: git archive --format=tar.gz -o cadre.tgz HEAD" ;;
      esac || die "extracting $CADRE_SRC failed" "check the tarball (tar -tzf $CADRE_SRC)"
    fi
    local root; root="$(find_root "$stage")" || die "no crates/Cargo.toml inside $CADRE_SRC" "CADRE_SRC must be the pearl-sol repo (or an archive of it)"
    rm -rf "$SRC"
    if [[ "$root" == "$stage" ]]; then mv "$stage" "$SRC"; else mv "$root" "$SRC"; rm -rf "$stage"; fi
    ok "source copied to $SRC"
    return
  fi
  if [[ -d "$SRC/.git" ]]; then
    info "updating $SRC from $CADRE_REPO ($CADRE_REF)"
    run git -C "$SRC" remote set-url origin "$CADRE_REPO"
    run git -C "$SRC" fetch -q --depth 1 origin "$CADRE_REF" || die "git fetch of $CADRE_REF failed" "check CADRE_REPO / CADRE_REF and your access to the repo"
    run git -C "$SRC" checkout -q --detach FETCH_HEAD
  else
    info "cloning $CADRE_REPO ($CADRE_REF)"
    run rm -rf "$SRC"
    if [[ $DRY != 1 ]]; then
      if ! GIT_TERMINAL_PROMPT=0 git clone -q --depth 1 --branch "$CADRE_REF" "$CADRE_REPO" "$SRC" 2>/dev/null; then
        # --branch takes branches and tags only; a commit needs init + fetch.
        rm -rf "$SRC"; git init -q "$SRC"; git -C "$SRC" remote add origin "$CADRE_REPO"
        GIT_TERMINAL_PROMPT=0 git -C "$SRC" fetch -q --depth 1 origin "$CADRE_REF" \
          || die "could not fetch $CADRE_REPO at $CADRE_REF" "if the repo is private: use an SSH URL (CADRE_REPO=git@github.com:...) or a token, or download it and set CADRE_SRC=/path/to/checkout-or-tarball"
        git -C "$SRC" checkout -q --detach FETCH_HEAD
      fi
    fi
  fi
  [[ $DRY == 1 ]] || ok "source at $SRC, commit $(git -C "$SRC" rev-parse --short HEAD)"
}

find_root() {
  local d="$1"
  [[ -f "$d/crates/Cargo.toml" ]] && { echo "$d"; return 0; }
  local sub; for sub in "$d"/*/; do [[ -f "${sub}crates/Cargo.toml" ]] && { echo "${sub%/}"; return 0; }; done
  return 1
}

# ---------- 5. build ----------
build_miner() {
  local bt0; bt0=$(date +%s)
  info "cargo +$TOOLCHAIN build --release -p pearl-miner (target dir $TARGET)"
  if [[ $DRY == 1 ]]; then run "install $TARGET/release/pearl-miner to $BIN/pearl-miner"; return; fi
  local locked="--locked"
  if ! (cd "$SRC/crates" && cargo +"$TOOLCHAIN" metadata --locked --format-version 1 >/dev/null 2>&1); then
    warn "crates/Cargo.lock in this source is not up to date; building with the lock file updated (versions it pins are kept)"
    locked=""
  fi
  (cd "$SRC/crates" && CARGO_TARGET_DIR="$TARGET" cargo +"$TOOLCHAIN" build --release $locked -q -p pearl-miner) \
    || die "the build failed (errors above)" "on Ubuntu make sure build-essential is installed; on low-memory boxes (under 2 GB) add swap, or set CARGO_BUILD_JOBS=1 and retry"
  mkdir -p "$BIN"
  # Copy then rename, so a miner that is running keeps its old binary until it restarts.
  cp "$TARGET/release/pearl-miner" "$BIN/.pearl-miner.new" && chmod 755 "$BIN/.pearl-miner.new" && mv -f "$BIN/.pearl-miner.new" "$BIN/pearl-miner"
  ok "built in $(( $(date +%s) - bt0 )) s: $BIN/pearl-miner"
  "$BIN/pearl-miner" --help >/dev/null || die "the built binary does not run" "remove $TARGET and run again"
}

# The CLI is its own Cargo workspace (crates/pearl-cli, Rust $TOOLCHAIN, its own Cargo.lock).
build_cli() {
  local bt0; bt0=$(date +%s)
  info "cargo +$TOOLCHAIN build --release --bins in crates/pearl-cli (target dir $TARGET/cli)"
  if [[ $DRY == 1 ]]; then run "install $TARGET/cli/release/cadre and pearl to $BIN"; return; fi
  (cd "$SRC/crates/pearl-cli" && CARGO_TARGET_DIR="$TARGET/cli" cargo +"$TOOLCHAIN" build --release --locked -q --bins) \
    || die "the CLI build failed (errors above)" "the miner is installed; retry with --cli later, or set CARGO_BUILD_JOBS=1 on a small box"
  mkdir -p "$BIN"
  local b
  for b in cadre pearl; do
    cp "$TARGET/cli/release/$b" "$BIN/.$b.new" && chmod 755 "$BIN/.$b.new" && mv -f "$BIN/.$b.new" "$BIN/$b"
  done
  ok "built in $(( $(date +%s) - bt0 )) s: $BIN/cadre (try: $BIN/cadre, $BIN/cadre ping api)"
  "$BIN/cadre" --version >/dev/null || die "the built CLI does not run" "remove $TARGET/cli and run again"
}

# ---------- 6. key ----------
# Ask the miner itself (the project's key parser) for the address: --blocks 0 logs the
# "start" line (with the public address) and exits without contacting the node.
key_address() {
  "$BIN/pearl-miner" --rpc http://127.0.0.1:9 --payout "$KEY" --blocks 0 --threads 1 \
    --results-dir "$CADRE_HOME/results" --models "$CADRE_HOME/models" 2>/dev/null \
    | sed -n 's/.*"miner":"\([1-9A-HJ-NP-Za-km-z]*\)".*/\1/p' | head -1
}

make_key() {
  if [[ -f "$KEY" ]]; then
    run chmod 600 "$KEY"
    if [[ $DRY == 1 ]]; then ok "keeping existing key $KEY"; return; fi
    local addr; addr="$(key_address)"
    [[ -n "$addr" ]] || die "$KEY exists but pearl-miner cannot read it" "move it away (do not delete a key that holds rewards) and run again for a new one"
    ok "keeping existing key $KEY (0600)"
    ok "payout address: $addr"
    PAYOUT="$addr"
    return
  fi
  if [[ $DRY == 1 ]]; then run "generate a new ed25519 key into $KEY (0600, never printed)"; return; fi
  # ed25519: 32 random bytes are the seed. With openssl the public half is derived too and the
  # file is the usual 64-byte Solana JSON array; without it the 32-byte seed array is written,
  # which pearl-miner and the pearl CLI read the same way.
  local tmp; tmp="$(umask 077; mktemp -d "$CADRE_HOME/.keygen.XXXXXX")"
  trap 'rm -rf "${tmp:-}"; trap - RETURN' RETURN
  (umask 077; head -c 32 /dev/urandom > "$tmp/seed")
  [[ $(wc -c < "$tmp/seed" | tr -d ' ') == 32 ]] || die "could not read 32 bytes from /dev/urandom" "check /dev/urandom"
  local form="32-byte seed"
  if have openssl; then
    (umask 077; { printf '\x30\x2e\x02\x01\x00\x30\x05\x06\x03\x2b\x65\x70\x04\x22\x04\x20'; cat "$tmp/seed"; } > "$tmp/der")
    if openssl pkey -inform DER -in "$tmp/der" -pubout -outform DER 2>/dev/null | tail -c 32 > "$tmp/pub" \
       && [[ $(wc -c < "$tmp/pub" | tr -d ' ') == 32 ]]; then
      (umask 077; cat "$tmp/pub" >> "$tmp/seed"); form="64-byte keypair"
    fi
  fi
  (umask 077; printf '[%s]\n' "$(od -An -v -tu1 "$tmp/seed" | tr -s ' \n' ' ' | sed 's/^ //; s/ $//; s/ /,/g')" > "$tmp/key.json")
  mv "$tmp/key.json" "$KEY"; chmod 600 "$KEY"
  local addr; addr="$(key_address)"
  if [[ -z "$addr" ]]; then rm -f "$KEY"; die "pearl-miner rejected the generated key" "report this; nothing was kept"; fi
  ok "new key $KEY (0600, $form, Solana JSON array; never printed)"
  ok "payout address: $addr"
  info "back this file up: it is the only way to spend what this miner earns"
  PAYOUT="$addr"
}

# ---------- 7. config ----------
write_config() {
  local rpc="$1" pool="$2" threads="$3" solo="$4"
  # Earlier values stay unless a flag (or env) changes them.
  local old_rpc="" old_pool="" old_threads="" old_extra="" old_stats=""
  if [[ -f "$ENV_FILE" ]]; then
    old_rpc="$(sed -n 's/^CADRE_RPC=//p' "$ENV_FILE" | tail -1)"
    old_pool="$(sed -n 's/^CADRE_POOL=//p' "$ENV_FILE" | tail -1)"
    old_threads="$(sed -n 's/^CADRE_THREADS=//p' "$ENV_FILE" | tail -1)"
    old_extra="$(sed -n 's/^CADRE_EXTRA=//p' "$ENV_FILE" | tail -1)"
    old_stats="$(sed -n 's/^CADRE_STATS_SECS=//p' "$ENV_FILE" | tail -1)"
  fi
  rpc="${rpc:-${old_rpc:-$DEFAULT_RPC}}"
  RPC_USED="$rpc"
  local stats="${old_stats:-60}"
  STATS_USED="$stats"
  if [[ $solo == 1 ]]; then pool=""; else pool="${pool:-$old_pool}"; fi
  local cores; cores="$(cpu_count)"
  threads="${threads:-${old_threads:-$(( cores / 2 > 0 ? cores / 2 : 1 ))}}"
  (( threads > cores )) && warn "$threads threads on $cores cores: more threads than cores only slows each one down"

  rpc="$(rpc_list "$rpc")"; RPC_USED="$rpc"
  local u probe answered=0
  local -a urls; IFS=, read -r -a urls <<< "$rpc"
  for u in "${urls[@]}"; do
    probe="$(chain_info "$u")"
    if [[ "$probe" == *'"result"'* ]]; then
      local h; h="$(printf '%s' "$probe" | sed -n 's/.*"height":\([0-9]*\).*/\1/p' | head -1)"
      ok "node $u answers${h:+ (height $h)}"
      [[ -n "${FINALITY_K:-}" ]] || FINALITY_K="$(printf '%s' "$probe" | sed -n 's/.*"finality_k":\([0-9]*\).*/\1/p' | head -1)"
      answered=$((answered + 1))
    else
      warn "node $u did not answer getChainInfo now"
    fi
  done
  if (( answered == 0 )); then
    warn "no node answered; the miner keeps retrying, but check the URLs (--rpc)"
  elif (( ${#urls[@]} > 1 )); then
    ok "$answered of ${#urls[@]} nodes answer; pearl-miner fails over between them"
  fi
  if [[ -n "$pool" ]]; then
    if curl -sf -m 10 "${pool%/}/health" >/dev/null 2>&1; then ok "pool $pool answers /health"
    else warn "pool $pool did not answer /health; check the URL (--pool)"; fi
  fi

  if [[ $DRY == 1 ]]; then
    run "write $ENV_FILE: CADRE_RPC=$rpc CADRE_POOL=${pool:-<solo>} CADRE_THREADS=$threads"
    run "write $RUNNER"
    return
  fi
  mkdir -p "$CADRE_HOME/results" "$CADRE_HOME/models" "$BIN"
  cat > "$ENV_FILE.new" <<EOF
# Cadre miner config, read by $RUNNER (and the cadre-miner user service).
# Edit, then: systemctl --user restart cadre-miner   (or rerun the installer with flags)
CADRE_HOME=$CADRE_HOME
CADRE_RPC=$rpc
# Empty = solo (mine blocks against CADRE_RPC); a URL = pool mode (shares to that pearl-pool).
CADRE_POOL=$pool
CADRE_THREADS=$threads
CADRE_KEY=$KEY
# Seconds between stats lines in the log.
CADRE_STATS_SECS=$stats
# Extra pearl-miner flags, e.g. --deliver --attester URL (pearl-miner --help)
CADRE_EXTRA=$old_extra
EOF
  mv -f "$ENV_FILE.new" "$ENV_FILE"; chmod 644 "$ENV_FILE"
  ok "$ENV_FILE: rpc $rpc, ${pool:+pool $pool}${pool:-solo}, $threads threads"

  cat > "$RUNNER.new" <<EOF
#!/usr/bin/env bash
# Runs pearl-miner with $ENV_FILE. Written by install-miner.sh; one JSON log line per event.
set -euo pipefail
ENV_FILE="\${CADRE_ENV_FILE:-$ENV_FILE}"
# shellcheck disable=SC1090
set -a; . "\$ENV_FILE"; set +a
cd "\$CADRE_HOME"
args=(--rpc "\$CADRE_RPC" --payout "\$CADRE_KEY" --threads "\$CADRE_THREADS" --stats-secs "\${CADRE_STATS_SECS:-60}"
      --results-dir "\$CADRE_HOME/results" --models "\$CADRE_HOME/models")
[[ -n "\${CADRE_POOL:-}" ]] && args+=(--pool "\$CADRE_POOL")
# shellcheck disable=SC2086
exec "\$CADRE_HOME/bin/pearl-miner" "\${args[@]}" \${CADRE_EXTRA:-} "\$@"
EOF
  chmod 755 "$RUNNER.new"; mv -f "$RUNNER.new" "$RUNNER"
  ok "runner $RUNNER"
}

chain_info() {
  curl -s -m 10 -X POST -H 'content-type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getChainInfo","params":{}}' "$1" 2>/dev/null || true
}
is_devnet_rpc() { local u; for u in "${DEVNET_RPCS[@]}"; do [[ "$1" == "$u" ]] && return 0; done; return 1; }

# The node list to write: the given URLs (comma-separated, order kept), then every devnet node not
# already listed, when the first given URL is a devnet node or reports the devnet genesis hash. A
# node of another chain (a localnet, say) is written as given: devnet nodes would be another chain.
rpc_list() {
  local given="$1" out="" u
  local -a urls; IFS=, read -r -a urls <<< "$given"
  for u in "${urls[@]}"; do u="${u// /}"; [[ -n "$u" && ",$out," != *",$u,"* ]] && out="${out:+$out,}$u"; done
  local first="${out%%,*}"
  if is_devnet_rpc "$first" || [[ "$(chain_info "$first")" == *"\"genesis_hash\":\"$DEVNET_GENESIS\""* ]]; then
    for u in "${DEVNET_RPCS[@]}"; do [[ ",$out," == *",$u,"* ]] || out="$out,$u"; done
  fi
  printf '%s' "$out"
}

# ---------- 8. service ----------
install_service() {
  local unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user" unit="cadre-miner.service"
  local can_systemd=0
  if [[ $OS == Linux ]] && have systemctl && systemctl --user show-environment >/dev/null 2>&1; then can_systemd=1; fi
  if [[ $NO_SERVICE == 1 || $can_systemd == 0 ]]; then
    [[ $NO_SERVICE == 1 ]] && info "--no-service: no service installed" || info "no systemd user session here: no service installed"
    echo
    echo "  Start (foreground, Ctrl-C stops):  $RUNNER"
    echo "  Start in the background:           nohup $RUNNER >> $CADRE_HOME/miner.log 2>&1 & echo \$! > $CADRE_HOME/miner.pid"
    echo "  Logs:                              tail -f $CADRE_HOME/miner.log"
    echo "  Stop:                              kill \"\$(cat $CADRE_HOME/miner.pid)\""
    return
  fi
  if [[ $DRY == 1 ]]; then
    run "write $unit_dir/$unit"; run systemctl --user daemon-reload; run systemctl --user enable --now cadre-miner
    return
  fi
  mkdir -p "$unit_dir"
  cat > "$unit_dir/$unit.new" <<EOF
[Unit]
Description=Cadre miner (pearl-miner, config $ENV_FILE)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$RUNNER
Nice=10
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
EOF
  mv -f "$unit_dir/$unit.new" "$unit_dir/$unit"
  systemctl --user daemon-reload
  if systemctl --user is-active -q cadre-miner; then
    systemctl --user restart cadre-miner && ok "restarted cadre-miner (new binary / config)"
  else
    systemctl --user enable --now cadre-miner >/dev/null 2>&1 || die "systemctl --user enable --now cadre-miner failed" "see: journalctl --user -u cadre-miner -n 50"
    ok "cadre-miner enabled and started"
  fi
  sleep 3
  systemctl --user is-active -q cadre-miner || die "cadre-miner stopped right after start" "see: journalctl --user -u cadre-miner -n 50"
  enable_linger
  echo
  echo "  Status:  systemctl --user status cadre-miner"
  echo "  Logs:    journalctl --user -u cadre-miner -f"
  echo "  Stop:    systemctl --user stop cadre-miner     (disable: systemctl --user disable --now cadre-miner)"
  echo "  Start:   systemctl --user start cadre-miner"
}

# Lingering keeps the user service running after logout. Done here only when it cannot block:
# as root, or with sudo that needs no password (`sudo -n`); otherwise the command is printed.
enable_linger() {
  local me; me="$(id -un)"
  [[ "$(loginctl show-user "$me" -p Linger --value 2>/dev/null || echo no)" == yes ]] && { ok "lingering on: the miner keeps running after logout"; return; }
  local cmd="sudo loginctl enable-linger $me"
  if [[ $NO_LINGER == 1 ]]; then
    info "--no-linger: the service stops when you log out; to keep it running: $cmd"
    return
  fi
  local sudo=""
  if [[ ${EUID:-$(id -u)} -ne 0 ]]; then
    if have sudo && sudo -n true 2>/dev/null; then sudo="sudo -n"; else
      warn "user services stop when you log out; to keep mining after logout run: $cmd"
      return
    fi
  fi
  if $sudo loginctl enable-linger "$me" 2>/dev/null; then
    ok "lingering enabled for $me: the miner keeps running after logout (undo: sudo loginctl disable-linger $me)"
  else
    warn "could not enable lingering; to keep mining after logout run: $cmd"
  fi
}

# ---------- after: how to see it work ----------
print_check() {
  [[ $DRY == 1 || -z "${PAYOUT:-}" ]] && return 0
  echo
  echo "  Is it mining?  The log shows a stats line every ${STATS_USED:-60} s (nonces_per_s) and"
  echo "  a block_accepted line for each block this miner wins. How soon the first one comes depends on"
  echo "  your share of the network's hashrate."
  local first="${RPC_USED%%,*}"
  if [[ ",$RPC_USED," == *",$DEFAULT_RPC,"* ]]; then
    echo "  Blocks credited to your address:  curl -s $DEFAULT_API/v1/miners/$PAYOUT"
  fi
  echo "  Balance:  curl -s -X POST -H 'content-type: application/json' \\"
  echo "              -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":{\"account\":\"$PAYOUT\"}}' $first"
  echo "  Earnings: a block on a paid AI job earns a subsidy (plus fees); with no job open the miner"
  echo "  mines fallback blocks, which earn the block's fees only (often 0). Both pay out ${FINALITY_K:-K} blocks later."
}

main "$@"
