# cadre-proxy

A local OpenAI-compatible endpoint for agent frameworks. It holds its own wallet, enforces
spend caps, and forwards requests to the Cadre inference gateway (`cadre-gateway`, API v1 in
docs/SPEC.md 5.4). Clients only change their base URL to `http://127.0.0.1:10695/v1`.

Plain Node (>= 20), no dependencies, no build step.

## Run

    node src/cli.mjs wallet                       # create / show the proxy wallet (address only)
    node src/cli.mjs topup                        # what a balance top-up needs (the gateway's 402)
    node src/cli.mjs serve --max-day-lamports 50000000 --max-request-rows 2048
    node src/cli.mjs config hermes                # or openclaw | kilo | cline
    node src/cli.mjs balance                      # GET /v1/balance, wallet-signed
    node src/cli.mjs spend                        # local ledger and caps

Options: `--gateway URL` (env `CADRE_GATEWAY`, default `http://127.0.0.1:10664`), `--port`
(default 10695), `--host` (default 127.0.0.1; any other host needs `--token`), `--token T`
(env `CADRE_PROXY_TOKEN`: local clients must send `Authorization: Bearer T`; without it any
local key is accepted), `--wallet`, `--ledger`, `--default-max-tokens` (default 256, injected
when a request sets neither `max_tokens` nor `max_completion_tokens`, so every request has a
bound). Files live in `~/.config/cadre-proxy` (env `CADRE_PROXY_HOME`).

Web pages cannot use the proxy: a request with an `Origin` header (every cross-origin browser
request has one) is refused with 403 unless the origin is listed in `CADRE_PROXY_ALLOW_ORIGINS`
(comma separated), and on loopback a `Host` other than 127.0.0.1, localhost or [::1] is refused
(DNS rebinding). Local tools (OpenAI SDKs, curl, editors) send no Origin and are unaffected.

## Wallet

One ed25519 key, the same on Solana (x402 payer of top-ups) and on the Cadre chain (balance
owner, ticket poster). Generated on first run as a Solana-style 64-byte JSON array in a 0600
file under a 0700 directory; the proxy refuses a key file other users can read. The secret
is never printed, logged or written anywhere else; only the address is shown. Fund it like
any Solana wallet (it pays the top-up in SOL).

## Gateway auth

Default: wallet-signature headers on every forwarded request (`X-Cadre-Wallet`,
`X-Cadre-Timestamp`, `X-Cadre-Signature` = raw ed25519 over `"cadre-auth-v2\n" + AUDIENCE +
"\n" + METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + hex(sha256(body))`, PATH without the
query, AUDIENCE = the gateway's server key, read once from its `/v1/models`
(`data[0].cadre.server`), so a signature only works at that gateway). Or
`node src/cli.mjs key` makes a gateway API key (`POST /v1/keys`), saved to
`api-key.json` (0600, never printed), and `serve --use-api-key` sends it as a bearer token.

## Spend caps

`--max-request-lamports`, `--max-day-lamports`, `--max-total-lamports`, `--max-request-rows`,
`--max-day-rows` (none set = no cap). Before forwarding, the proxy estimates rows as prompt
tokens (1 per 3 characters + 8 per message, an over-estimate) plus the max new tokens, and
lamports as rows x the model's `lamports_per_row` from the gateway's `/v1/models`. In-flight
estimates count against the caps. After the response it books the gateway's actual charge
from the receipt (`X-Cadre-Rows` / `X-Cadre-Charged-Lamports`, or the `cadre` object of the
body or of the last SSE chunk). A refused request gets HTTP 429
`{"error": {"type": "insufficient_quota", "code": "cadre_spend_cap", "message"}}` and never
reaches the gateway. If a lamport cap is set but the gateway publishes no price, requests are
refused (fail closed). The ledger (`ledger.json`) keeps day totals, the overall total and the
last 200 receipts; it is rewritten atomically.

## Routes

`POST /v1/chat/completions` and `/v1/completions` (capped; streams pass through byte for
byte), `GET /v1/models`, `GET /v1/balance`, `POST /v1/balance/close`, `GET /v1/requests/<id>`,
`GET /v1/receipts[/<id>]`, `GET /v1/stats` (forwarded, signed), plus local `GET /health` and
`GET /cadre/spend`.

## Top-up

`topup [--rows N]` prints the gateway's 402 requirements (pay to the storefront, native SOL).
`topup --pay` builds the x402 "exact" native-SOL payment itself (one System transfer of
exactly the amount, the storefront as fee payer) and retries with `X-PAYMENT`. It only runs
against a loopback `--sol-rpc` (default `http://127.0.0.1:10600`, the localnet); public
chains are refused. On devnet or mainnet, pay with a wallet you control instead.

## Client configs

`config <client>` prints a one-command snippet with the proxy base URL, the gateway's model id
(from `/v1/models`, or `llama-3.2-1b-w7a7` when the gateway is unreachable) and a dummy key
(or your `--token`). Sources, checked 2026-10-19:

| Client | Form | Source |
|---|---|---|
| Hermes Agent | `hermes config set model.provider custom ...` + `~/.hermes/config.yaml` `model:` block | keys: NousResearch/hermes-agent `cli-config.yaml.example`; the `config set` form: NVIDIA Hermes playbook |
| OpenClaw | `openclaw onboard --custom-base-url ... --custom-model-id ... --custom-no-auth` + `models.providers.cadre` block | docs.openclaw.ai custom providers (flags named there; the combined command is not shown, unverified) |
| Kilo Code | `~/.config/kilo/kilo.json` with provider `openai-compatible` (`options.baseURL`, `options.apiKey`) | kilo.ai/docs custom models (file location from the same page's note on global config) |
| Cline | settings panel values only (OpenAI Compatible, Base URL, API Key, Model ID) | docs.cline.bot; no file or CLI form is documented, so none is printed |

Cadre serves short contexts (4096 tokens at the time of writing); agents that need long
contexts will not fit.

## Tests

    node --test test/*.test.mjs

A mock gateway on 127.0.0.1:10696, the proxy on 10697 and a CLI-spawned proxy on 10698
(ports per CLAUDE.md). Covers the auth bytes (rebuilt independently), API-key mode, SSE pass
through, caps, local token, wallet perms, that the secret never shows in any output or the
ledger, the x402 payment (cross-checked with services/storefront-x402 `verifyExact` when its
dependencies are installed) and the config snippets.
