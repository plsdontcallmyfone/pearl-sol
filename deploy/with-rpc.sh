#!/bin/sh
# Devnet service wrapper (installed as /opt/lode/bin/with-rpc): when /etc/lode/solana-rpc-url holds a
# dedicated Solana RPC URL (copied from the Mac's keys/devnet/solana-rpc-url, 0600, never logged), the
# service talks to it; otherwise TOKEN_RPC from the env files (the public devnet RPC) stays.
f=/etc/lode/solana-rpc-url
if [ -s "$f" ] && [ -r "$f" ]; then
  u=$(head -n 1 "$f" | tr -d '\r\n ')
  if [ -n "$u" ]; then
    TOKEN_RPC=$u; SOLANA_RPC=$u; RPC_URL=$u
    export TOKEN_RPC SOLANA_RPC RPC_URL
  fi
fi
exec "$@"
