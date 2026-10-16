#!/usr/bin/env bash
# Dumps the pump.fun programs and the mainnet accounts they need (curve flow,
# graduation, PumpSwap) into fixtures/pump/, and writes validator-args.txt for
# `scripts/localnet.sh up token`. Account list: research/NOTES-pump.md section 2.
#
# RPC: $PUMP_FIXTURE_RPC, else Helius mainnet with the key read from
# ~/.config/bordr-watch/helius.key (never printed; errors are redacted).
# Programs are pinned bytes: re-run this script to refresh after a pump deploy.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/fixtures/pump"
mkdir -p "$OUT/programs" "$OUT/accounts"

if [[ -z "${PUMP_FIXTURE_RPC:-}" ]]; then
  KEYF="$HOME/.config/bordr-watch/helius.key"
  [[ -f $KEYF ]] || { echo "no PUMP_FIXTURE_RPC and no $KEYF" >&2; exit 1; }
  PUMP_FIXTURE_RPC="https://mainnet.helius-rpc.com/?api-key=$(tr -d '[:space:]' <"$KEYF")"
fi
RPC="$PUMP_FIXTURE_RPC"
redact() { sed -E 's/api-key=[^&[:space:]"]+/api-key=REDACTED/g'; }

PUMP=6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P
AMM=pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA
FEES=pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ
WSOL=So11111111111111111111111111111111111111112
TOKEN=TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
ATAP=ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL

# name:address of the fixed accounts
FIXED="
pump_global:4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf
pump_fee_config:8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt
pump_global_volume_accumulator:Hq2wp8uJ9jCPsYgNHex8RtqdvMPfVGoYwjvF1ATiwn2Y
pump_quote_control:6z6GDdfb2AjR9ZhJmAUQ5cipJCVxQvLJhB2H8mCwTFBP
amm_global_config:ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw
amm_fee_config:5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx
amm_global_volume_accumulator:C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw
fees_program_global:CHqnuTkj6sXDFknM652aEFPECZh9qVsBXWkhPohmV9dA
withdraw_authority:39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg
mayhem_global_params:13ec7XdrjF3h3YcqBTFDSReRcUFwbCnJaAQspM4j6DDJ
mayhem_sol_vault:BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s
wsol_mint:$WSOL
"
BUYBACK="5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD 9M4giFFMxmFGXtc3feFzRai56WbBqehoSeRE5GK7gf7 GXPFM2caqTtQYC2cJ5yJRi9VDkpsYZXzYdwYpGnLmtDL 3BpXnfJaUTiwXnJNe7Ej1rcbzqTTQUvLShZaWazebsVR 5cjcW9wExnJJiqgLjq7DEG75Pm6JBgE1hNv4B2vHXUW6 EHAAiTxcdDwQ3U4bU6YcMsQGaekdzLS3B5SmYo46kJtL 5eHhjP8JaYkz83CWwvGU2uMUXefd3AazWGx4gpcuEEYD A7hAgCzFw14fejgCp387JUJRMNyz4j89JKnhtKU8piqW"
FEEREC="62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV 7VtfL8fvgNfhz17qKRMjzQEXgbdpnHHHQRh54R9jP2RJ 7hTckgnGnLQR6sdH7YkqFTAA7VwTfYFaZ6EhEsU3saCX 9rPYyANsfQZw3DnDmKE3YCQF5E8oD89UXoHn9JFEhJUz AVmoTthdrX6tKt4nDjco2D775W2YK3sDhxPcMmzUAmTY CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM FWsW1xNtWscwNmKv6wVsU1iTzRN6wmmk3MjxRP5tT7hz G5UZAVbAf46s7cKWoyKu8kYTip9DGTpbLZ2qa9Aq69dP JCRGumoE9Qi5BBgULTgdgTLjSgkCMSbF62ZZfGs84JeU"

wsol_ata() { solana find-program-derived-address "$ATAP" "pubkey:$1" "pubkey:$TOKEN" "pubkey:$WSOL" | awk '{print $1}'; }

LIST="$FIXED"
i=0; for a in $BUYBACK; do LIST+="buyback_$i:$a"$'\n'"buyback_${i}_wsol:$(wsol_ata "$a")"$'\n'; i=$((i+1)); done
i=0; for a in $FEEREC;  do LIST+="fee_recipient_$i:$a"$'\n'"fee_recipient_${i}_wsol:$(wsol_ata "$a")"$'\n'; i=$((i+1)); done

rm -f "$OUT"/accounts/*.json
ARGS="$OUT/validator-args.txt"
: >"$ARGS.tmp"

for e in pump:$PUMP pump_amm:$AMM pump_fees:$FEES; do
  n=${e%%:*}; id=${e#*:}; f="$OUT/programs/$n.so"
  solana program dump -u "$RPC" "$id" "$f.raw" 2>&1 | redact >/dev/null || { echo "dump $n failed" >&2; exit 1; }
  # programdata is padded with zeros up to max_len; drop the padding (ELF ends at its section table)
  python3 - "$f.raw" "$f" <<'PY'
import sys, struct
b = open(sys.argv[1], 'rb').read()
assert b[:4] == b'\x7fELF' and b[4] == 2, 'not a 64-bit ELF'
phoff, shoff = struct.unpack_from('<QQ', b, 0x20)
phentsize, phnum, shentsize, shnum = struct.unpack_from('<HHHH', b, 0x36)
end = max(shoff + shnum * shentsize, phoff + phnum * phentsize)
for i in range(shnum):
    o = shoff + i * shentsize
    typ = struct.unpack_from('<I', b, o + 4)[0]
    off, size = struct.unpack_from('<QQ', b, o + 0x18)
    if typ != 8:  # SHT_NOBITS has no file bytes
        end = max(end, off + size)
for i in range(phnum):
    o = phoff + i * phentsize
    off, _, _, filesz = struct.unpack_from('<QQQQ', b, o + 8)
    end = max(end, off + filesz)
assert not any(b[end:]), 'non-zero bytes after the ELF end'
open(sys.argv[2], 'wb').write(b[:end])
PY
  rm -f "$f.raw"
  echo "--upgradeable-program $id fixtures/pump/programs/$n.so none" >>"$ARGS.tmp"
  echo "program $n $(wc -c <"$f" | tr -d ' ') bytes"
done

missing=0
while IFS=: read -r name addr; do
  [[ -n $name ]] || continue
  f="$OUT/accounts/$name.json"
  if solana account -u "$RPC" "$addr" --output json-compact -o "$f" >/dev/null 2>"$OUT/.err"; then
    echo "--account $addr fixtures/pump/accounts/$name.json" >>"$ARGS.tmp"
  else
    echo "skip $name $addr: $(redact <"$OUT/.err" | head -1)"; rm -f "$f"; missing=$((missing+1))
  fi
done <<<"$LIST"
rm -f "$OUT/.err"
mv "$ARGS.tmp" "$ARGS"
SLOT=$(solana slot -u "$RPC" 2>/dev/null || echo unknown)
cat >"$OUT/SOURCE.txt" <<META
Dumped from Solana mainnet at slot $SLOT on $(date -u +%Y-%m-%dT%H:%M:%SZ) by scripts/pump-fixtures.sh.
Programs have their programdata zero padding stripped. Accounts skipped (absent on mainnet): $missing.
META
echo "wrote $(grep -c . "$ARGS") validator args, $(du -sh "$OUT" | awk '{print $1}') total, $missing skipped"
