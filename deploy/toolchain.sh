#!/usr/bin/env bash
# Build toolchains for the devnet build box, as user lode (docs/DEVNET.md, same versions as docs/VPS.md):
#   ssh lode@BOX 'bash -s' < deploy/toolchain.sh
set -euo pipefail
cd ~
if ! command -v ~/.cargo/bin/rustup >/dev/null; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y -q --default-toolchain 1.94.1 --profile minimal
fi
source ~/.cargo/env
rustup toolchain install 1.95.0 --profile minimal -q
if [[ ! -x ~/.local/share/solana/install/active_release/bin/solana ]]; then
  sh -c "$(curl -sSfL https://release.anza.xyz/v3.1.12/install)" >/dev/null
fi
export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"
if ! command -v ~/.cargo/bin/avm >/dev/null; then
  cargo install --git https://github.com/coral-xyz/anchor --tag v0.31.1 avm --locked -q
fi
~/.cargo/bin/avm install 0.31.1 >/dev/null 2>&1 || true
~/.cargo/bin/avm use 0.31.1
if [[ ! -x ~/.local/node/bin/node ]]; then
  curl -sSfL https://nodejs.org/dist/v22.22.1/node-v22.22.1-linux-x64.tar.xz | tar -xJ -C ~/.local 2>/dev/null || { mkdir -p ~/.local && curl -sSfL https://nodejs.org/dist/v22.22.1/node-v22.22.1-linux-x64.tar.xz | tar -xJ -C ~/.local; }
  mv ~/.local/node-v22.22.1-linux-x64 ~/.local/node
fi
grep -q 'cadre-path' ~/.bashrc || sed -i '1i # cadre-path\nexport PATH="$HOME/.cargo/bin:$HOME/.local/share/solana/install/active_release/bin:$HOME/.local/node/bin:$PATH"' ~/.bashrc
export PATH="$HOME/.local/node/bin:$PATH"
npm i -g pnpm@9 >/dev/null 2>&1 || true
echo "rust $(rustc --version) | solana $(solana --version | cut -d' ' -f2) | anchor $(anchor --version) | node $(node --version)"
