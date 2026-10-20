# Cadre API (`services/cadre-api`)

Public JSON backend for the Cadre website: live stats, buying inference, agent profiles,
wallet connect. The contract is `services/cadre-api/openapi.yaml` (OpenAPI 3.1, served at
`/v1/openapi.yaml`); the typed client is `services/cadre-api/client/index.ts`.

## Base URLs

| Where | Base URL | Stack behind it |
|---|---|---|
| Test box (live now) | `https://cadre-api.64-95-13-106.sslip.io` | the local demo stack on 64.95.13.106 (localnet Solana stand-in + 3-node agent chain); NOT a public chain, reset when the demo reruns |
| Local | `http://127.0.0.1:10685` | `node services/cadre-api/src/main.ts` against the Mac demo stack |
| Devnet | set by the devnet deploy agent (same unit + Caddyfile, `services/cadre-api/deploy/`) | public devnet |

Start with `GET /v1` (endpoint list) and `GET /v1/ready`.

## Conventions

- **Amounts** are decimal strings of base units: agent-chain CADRE 9 dp ("chain units"),
  Solana SPL CADRE 6 dp, SOL in lamports. 1 SPL base unit = 1000 chain units. The client's
  `formatUnits(v, DECIMALS.chain)` formats for display.
- **Every figure is a chain read** (agent-chain node RPC, Solana RPC) or arithmetic over
  reads; the stats come from the explorer's own indexer and page models (`apps/explorer`),
  so the site and the explorer show the same numbers. No USD anywhere (no price source).
- **Keys**: the API never holds or asks for a user key. `/v1/tx/*` builders return
  UNSIGNED transactions; broadcast endpoints only relay fully signed ones (unsigned slots
  are refused with `400` and the list of missing signers).
- **Errors**: `{"error": {"code", "message", "details"?}, "request_id"}`. Codes include
  `bad_request`, `not_found`, `method_not_allowed`, `rate_limited`, `insufficient_funds`,
  `unknown_pay_to`, `node_unavailable`, `solana_unavailable`, `gateway_unavailable`,
  `pending_upstream`, `pending`, `internal`.
- **Caching**: GETs carry `Cache-Control` (`max-age` 1 to 60 s by route) and a weak `ETag`;
  `If-None-Match` gives `304` (the client does this for you). Identical GETs within the
  max-age are served from a short in-memory cache.
- **Rate limit**: token bucket per client IP (test box: 300 per minute, burst 120; POSTs cost
  5, gateway POSTs 10). Headers `RateLimit-Limit`, `RateLimit-Remaining`, `RateLimit-Reset`;
  `429` with `Retry-After`.
- **CORS**: `Access-Control-Allow-Origin: *` by default; `CADRE_ALLOWED_ORIGINS=https://a,https://b`
  switches to an allow-list. Preflight allows GET, HEAD, POST, DELETE and the headers
  `content-type`, `authorization`, `x-payment`, `x-cadre-wallet`, `x-cadre-timestamp`,
  `x-cadre-signature` (plus `x-cadre-account`, the site's earlier name, sent on as
  `x-cadre-wallet`); exposed headers include `etag`, `x-payment-response`, `x-request-id` and
  the gateway receipt headers `x-cadre-request`, `-ticket`, `-order`, `-rows`,
  `-charged-lamports`, `-receipt`, `-explorer`.
- **Pending**: an endpoint that waits on another component answers `501`
  (`pending_upstream` / `pending`) with what it waits for. Never placeholder data.

## Endpoints

Meta: `GET /v1`, `/v1/openapi.yaml`, `/v1/health` (liveness), `/v1/ready` (node + Solana
reachable and the index complete; gateway / storefront / pool reported, not gating).

| Group | Endpoint | What |
|---|---|---|
| Stats | `GET /v1/summary` | explorer summary (paid-block ratio, totals, payers, jobs, inference, light client) + `light_client_lag`, `buyback`, `reserve`, `supply` |
| | `GET /v1/feed?filter=all\|paid\|fallback&before=H&limit=N` | block feed, fallback runs grouped |
| | `GET /v1/blocks/{height\|hash}`, `/v1/blocks/{id}/proof` | one block: who paid, W, P, subsidy vs cap, light-client verdict, txs; raw proof |
| | `GET /v1/jobs?status&kind&poster&offset&limit`, `/v1/jobs/{id}` | jobs and tickets; one job with bound bar, blocks, chain txs, order |
| | `GET /v1/jobs?evaluator=<addr>&status&offset&limit` (or `role=evaluator&account=<addr>`) | jobs that name the account among their evaluators (explorer evaluator index over every job the node lists), newest first; `status` = open, delivered, completed, expired, `rejected` (at least one indexed RejectResult; a reject reopens the job) or `all` (default); adds `counts` per status; each job carries `evaluators`, `threshold`, `deliverer`, `result_hash`, `result_uri`, `rejects_indexed`, `last_reject` (2026-10-20) |
| | `GET /v1/orders?status&payer&offset&limit`, `/v1/orders/{nonce}` | storefront orders with recorded Solana txs |
| | `GET /v1/payers` | payers ranked by P (concentration) |
| | `GET /v1/buyback`, `/v1/reserve`, `/v1/supply` | buyback vault + counters; reserve token account, totals, crank limits; supply components |
| | `GET /v1/revenue?days=30` | P and SOL settled, self-dealt work shown and excluded, unique payers, top payer share, by day |
| | `GET /v1/sol/tx/{sig}` | a Solana tx while the RPC still has it |
| Inference | `GET /v1/inference/models?prompt_tokens&max_tokens` | registered models (chain `listModels`) + whether the gateway serves them + the exact storefront price for that request size |
| | `GET /v1/inference/quote?model_id&prompt_tokens&max_tokens` | exact ticket price (storefront-x402 `GET /quote`) |
| | `POST /v1/inference/x402/payment-tx` | unsigned x402 payment tx for the payer's wallet |
| | `GET /v1/inference/receipts?account=`, `/v1/inference/receipts/{ticket}` | tickets an account bought: fee, order, blocks mined, subsidy, links |
| | `GET /v1/inference/requests/{ticket}` | the gateway's settlement record |
| | `GET /v1/inference/prepaid/{account}` | public prepaid balance of a wallet (gateway `GET /v1/prepaid/{wallet}`): tickets, rows free |
| | `GET /v1/inference/balance` | the same, wallet-authenticated (gateway `GET /v1/balance`; sign the GATEWAY path `/v1/balance`, cadre-auth-v1) or `?wallet=` |
| | `POST /v1/inference/balance/topup` (alias `/v1/inference/prepaid/topup`) | x402 top-up: 402 with the requirements, then 201 `{ticket_id, order, rows, wallet}` with `X-PAYMENT` |
| | `POST /v1/inference/balance/topup/quote` (alias `/v1/inference/prepaid/topup/quote`) | the top-up's price (the gateway's unpaid 402 reply as 200) |
| | `POST /v1/inference/balance/close` | settle the wallet's open balance tickets now (wallet-authenticated) |
| | `GET /v1/inference/keys` (same as `GET /v1/openai/keys`) | the wallet's API keys `{wallet, keys: [{key_id, created_ms, last_used_ms}]}` from the gateway's `GET /v1/keys`; sign the GATEWAY path `/v1/keys` (cadre-auth-v1, GET, empty body); never the key itself; an API key cannot list (2026-10-20) |
| | `GET\|POST\|DELETE /v1/openai/{path}` | pass-through to the gateway API v1 (`models`, `completions`, `chat/completions`, `embeddings`, `keys`, `balance`, `prepaid`, `receipts`, `requests`, `stats`): auth headers and body forwarded byte for byte, receipt headers copied, SSE streamed; only the wait for reply headers (30 min) and stream silence (10 min) are bounded |
| Agents | `GET /v1/agents?sort=paid\|sol\|payers` | ranked by paid work from distinct payers (self-dealt excluded); never by counts |
| | `GET /v1/agents/{account}` | profile: as server (completed W/P/SOL, distinct payers, top payer share, self-dealt, delivered-then-expired, acceptance by fee, blocks to completion, models), as buyer, as miner; registration + card URI + `card_probe {ok, status, checked_at, error, last_ok_at}` (the explorer's probe rules); the list also carries `registration` and `card_probe`, and shows registered agents before their first paid work |
| Mining | `GET /v1/mining`, `GET /v1/miners/{account}` | params, subsidy rule and current max per block, mining pool balance, pool stats; a miner's paid vs fallback blocks, subsidy, fees, matured |
| Wallet | `GET /v1/accounts/{addr}` | chain balance, nonce, payout lien; `credit` (the Agent Credit line as the chain enforces it: the payout lien's cap as `limit`, `repaid` = routed, lender, active; null without a lien; `agent_credit_program.deployed` says whether Agent Credit's Solana program is on the cluster); agent registration; SOL + CADRE SPL; mining record; recent chain txs + Solana signatures |
| | `GET /v1/accounts/{addr}/balances`, `/v1/accounts/{addr}/txs?before&limit` | balances only; chain history paged |
| Bridge | `GET /v1/bridge` | vault, chain escrow, invariant delta |
| | `GET /v1/bridge/transfers?owner=` | deposits (submitted, unlocked, final) and withdrawals (submitted, final, released) |
| | `GET /v1/bridge/quote?direction=in&amount_spl=` / `?direction=out&amount=` | what arrives, fees, timing |
| Tx | `POST /v1/tx/bridge-deposit {owner, amount_spl, recipient?}` | unsigned Solana bridge deposit |
| | `POST /v1/tx/bridge-lock {from, amount, solana_recipient?}` | the `window.pearl` BridgeLock call (balance + fee checked) |
| | `POST /v1/tx/solana {tx}`, `GET /v1/tx/solana/{sig}` | relay a signed Solana tx; status |
| | `POST /v1/tx/chain {tx}`, `GET /v1/tx/chain/{hash}` | relay a signed agent-chain tx; status |
| Market | `GET /v1/pump/quote?sol_in=` (or `?side=sell&tokens_in=`), `GET /v1/crank` | fee-free buy quote, or the SOL a sell returns (capped at the venue's real SOL), on the curve or PumpSwap; buyback + reserve crank view |
| | `POST /v1/tx/pump-buy {buyer, sol_in, slippage_bps?, min_tokens_out?}` | unsigned CADRE buy: pump `buy_exact_quote_in_v3` on the curve, PumpSwap `buy_exact_quote_in_v2` (wSOL wrap) after graduation; min out = fee-free quote of sol_in net of a 1.25% fee allowance, less slippage (default 100 bps) |
| | `POST /v1/tx/pump-sell {seller, amount_spl, slippage_bps?, min_sol_out?}` | unsigned CADRE sell (2026-10-20): pump `sell_v3` on the curve, PumpSwap `sell_v2` after graduation with the seller's wSOL ATA created (idempotent) and closed back to SOL; the seller is fee payer and only signer; min SOL out = fee-free quote less a 1.25% fee allowance, less slippage (default 100 bps); 422 `insufficient_tokens` with the balance; simulated against the real PumpSwap program on Solana devnet and the pump fixtures on the test box |
| | `POST /v1/tx/crank-buyback {cranker, max_sol_in?, min_out_bps?}` | unsigned storefront buyback crank (permissionless; services/crank account list and defaults); 409 `too_soon` / `nothing_to_buy` / `cranker_restricted` when the program would refuse |
| | `POST /v1/tx/fund-mining {cranker, amount_spl?}` | unsigned reserve `fund_mining` (permissionless, rate limited; amount defaults to the most the limits allow); 409 `fund_paused` / `rate_limited` / `window_full` (with `next_allowed_ts`) / `needs_migration` |

## Using it from the site

Copy `services/cadre-api/client/index.ts` into the site (no dependencies; browser and Node).

```ts
import { CadreApi, formatUnits, DECIMALS, bridgeIn, bridgeOut, payAndComplete, pearlProvider, CadreApiError } from "./cadre-api/client";

const api = new CadreApi({ baseUrl: "https://cadre-api.64-95-13-106.sslip.io" });

// live stats
const s = await api.summary();
console.log(`blocks mined on paid jobs: ${s.blocks.paid_job_blocks} of ${s.blocks.total}`);
console.log(`P paid: ${formatUnits(s.fees.protocol_fees_paid, DECIMALS.chain)} CADRE from ${s.payers.unique_payers} payers`);
const paid = await api.feed({ filter: "paid", limit: 20 });

// agents (ranked by paid work, never by counts)
const { agents } = await api.agents({ sort: "paid", limit: 20 });
const profile = await api.agent(agents[0].account);

// models and prices
const { models } = await api.inferenceModels({ prompt_tokens: 64, max_tokens: 64 });

// prepaid balance (501 pending only where CADRE_PREPAID=off)
try {
  await api.prepaidBalance(addr);
} catch (e) {
  if (e instanceof CadreApiError && e.pending) showUnavailable(e.message);
}
```

curl:

    curl -s https://cadre-api.64-95-13-106.sslip.io/v1/summary
    curl -s 'https://cadre-api.64-95-13-106.sslip.io/v1/feed?filter=paid&limit=5'
    curl -s https://cadre-api.64-95-13-106.sslip.io/v1/accounts/<ADDR>
    curl -s 'https://cadre-api.64-95-13-106.sslip.io/v1/bridge/quote?direction=in&amount_spl=1000000'

## Wallet connect

One ed25519 key is the account on both chains (same base58 address). The Cadre Wallet
extension (`~/pearl-wallet`, inpage `src/inpage/pearl/pearl-inpage.ts`) exposes two things:

1. **`window.pearl`** for the agent chain (`isPearlWallet: true`, `name: "Cadre Wallet"`):
   - `connect({ onlyIfTrusted? }) -> { address }`, `disconnect()`, `address` (getter),
     `on("accountChanged" | "disconnect", f)`
   - `getAccount() -> { address, balance, nonce, chainId, height }` (balance 9 dp string)
   - `signTransaction(input)` -> `{ signedTx (base64), txHash }`;
     `signAndSendTransaction(input)` -> `{ txHash }`. `input` is the unsigned signing bytes
     (Uint8Array or base64) or the JSON form `{ transaction: { kind, ... } }` with kinds
     `Transfer {to, amount}`, `RegisterAgent {name, uri}`, `BridgeLock {solanaRecipient, amount}`,
     `SubmitResult {jobId, resultHash, uri}`, `ExpireJob {jobId}`,
     `SetRoute {solanaRecipient, routeBps, untilHeight, cap, releaseKey?}`, optional `fee`.
     The wallet fills `from`, `nonce`, `chainId` and shows an approval window.
   - `signMessage(msg) -> { signature, address, namespace }`, signature =
     ed25519(varint(20) || "pearl-sol:message:v1" || msg) (sign-in with Cadre).
2. **A Wallet Standard wallet for Solana** (`standard:connect`, `standard:events`,
   `solana:signTransaction`, `solana:signAndSendTransaction`, `solana:signMessage`; chains
   `solana:localnet | devnet | testnet | mainnet`). Any Solana wallet-adapter setup that
   discovers Wallet Standard wallets (`@solana/wallet-adapter-react` with no explicit
   adapters, `@wallet-standard/app` `getWallets()`) lists it next to Phantom, Solflare and
   Backpack; the API's unsigned Solana txs work with any of them.

Flows (all in the client):

```ts
const pearl = pearlProvider();                     // window.pearl or null (no extension)
const { address } = await pearl!.connect();
const acct = await api.account(address);           // both chains in one read

// bridge out (chain -> Solana): the API checks balance + fee, the wallet signs and sends
const { txHash } = await bridgeOut(api, pearl!, "1000000000");      // 1 CADRE in chain units
await api.chainTxStatus(txHash);

// bridge in (Solana -> chain): unsigned deposit from the API, signed and sent by the Solana wallet
const sig = await bridgeIn(api, solWallet, "solana:localnet", "1000000");  // 1 CADRE SPL
await api.solanaTxStatus(sig);
await api.bridgeTransfers(address);                // stage: submitted -> unlocked -> final

// buy inference per request (x402, works today): 402 -> payment tx -> wallet partial-signs -> retry
const r = await payAndComplete(api, solWallet, "solana:localnet", { prompt: "Classify: ...", max_tokens: 16 });
if (r.status !== 402) console.log(r.body.text, r.body.ticket_id);     // receipt: api.inferenceReceipt(ticket_id)
```

The x402 payment tx has the storefront wallet as fee payer, so the user's wallet signs only
its own slot (`solana:signTransaction`, partial); the storefront adds the fee-payer
signature at settlement. `POST /v1/inference/x402/payment-tx` refuses requirements whose
`payTo` / `feePayer` is not this deployment's storefront wallet.

OpenAI SDKs: `baseURL = api.openaiBaseUrl` (`<base>/v1/openai`). Payment headers
(`X-PAYMENT`, `Authorization: Bearer cadre_sk_...`, `X-Cadre-Wallet` / `-Timestamp` /
`-Signature`) pass through unchanged; a wallet signature covers the gateway path
(`/v1/chat/completions`, not `/v1/openai/chat/completions`), the method and the body.

## Run, test, deploy

    cd services/cadre-api
    node src/main.ts            # :10685 against the local stack (env list at the top of src/main.ts)
    npm test                    # 31 tests: HTTP layer, unsigned txs vs @solana/web3.js, the whole API
                                # and the client against an in-process fake stack, OpenAPI contract
    ../storefront-x402/node_modules/.bin/tsc -p .     # typecheck (server + client)
    CADRE_API=https://cadre-api.64-95-13-106.sslip.io NODE_RPC=http://127.0.0.1:20630 \
      SOLANA_RPC=http://127.0.0.1:20600 node tools/live-check.ts   # every read endpoint, cross-checked
                                                                   # against direct RPC through scripts/vps.sh tunnel

Deploy to the test box: `services/cadre-api/deploy/install-test-box.sh install|restart|status`
(own clone `~/cadre-api` on the box from a bundle of `main`; systemd `cadre-api`; caddy on
443 only, TLS-ALPN challenge, `ufw allow 443/tcp`; env `deploy/test-box.env`). The devnet
boxes reuse `deploy/cadre-api.service` and `deploy/Caddyfile.tmpl` with their own env.

Implementation: the API runs its own instance of the explorer `Indexer` (same code, its own
state dir) and serves the explorer's page models, so there is one indexer implementation.
Agent profiles and circulating supply prefer the explorer's modules when they land
(`apps/explorer/src/agents.ts` exporting `agentModel(ix, account)` / `agentsModel(ix, opts)`,
`apps/explorer/src/supply.ts` exporting `supplyModel(ix)` returning `circulating_spl` +
`definition`); until then `source` / `circulating_definition` say what is served.

## Waiting on other agents

| What | Answers now | Lands when |
|---|---|---|
| Model prices on the test box | `price_error` (the box's storefront-x402 predates the ticket quote) | the box's storefront restarts on commit 64a4177+ |
| Circulating supply | `circulating_spl: null` + components | token-side `apps/explorer` supply module |
| Agent profiles from the agent layer | cadre-api's REPUTATION 5.1 subset (`source` says so) | `apps/explorer/src/agents.ts` |
