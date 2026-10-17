# Pearl Wallet (extension)

Chrome MV3 wallet for both pearl-sol chains with one ed25519 key:

- **Token chain = Solana**: SOL and the pump.fun Token-2022 token (6 dp), the
  `bridge` and `storefront` programs in `token/`.
- **Agent chain = our PoW chain**: native coin (9 dp), node JSON-RPC per
  `docs/RPC.md`, binary txs per SPEC section 2. 1 SPL unit = 1000 chain units.

The product name is a placeholder in one constant: `APP_NAME` in
`src/shared/constants.ts` (the manifest name is read from it at build time).
No logo: the toolbar icon slot is left empty and the Wallet Standard icon is a
neutral generic glyph.

## Build, load, test

```sh
cd extension
npm install
npm run build          # tsc + Vite (popup, approval) + esbuild (worker, content, inpage) -> dist/
npm test               # vitest: tx vectors, job parity, Solana ixs vs IDLs, vault, signing
npm run test:ui        # headless Chromium (Playwright) with dist/ loaded; screenshots -> screenshots/
# live writes against `DEMO_KEEP_UP=1 scripts/demo.sh` (bridge in, PostJob, storefront buy, both mined):
LIVE_KEY=key.json LIVE_EVALUATOR=<attester> LIVE_JOB_DIR=<pearl-job gen dir> node scripts/live-writes.mjs
```

Load `dist/` with chrome://extensions, Developer mode, "Load unpacked".

`test:ui` never opens a real browser profile: it runs Playwright's bundled
Chromium with `--headless=new` and a throwaway profile in `.ui-profile/`. If a
validator answers on :10600 and a node on :10630 it does a read-only live smoke
(no writes); otherwise it starts `scripts/mock-rpc.mjs` (Solana 10671, chain 10672,
test dApp page 10673). Dev ports stay inside 10670-10679.

## What it does

| Area | Details |
|---|---|
| Vault | Create (12-word phrase), import a **base58** secret key (64-byte keypair or 32-byte seed; a JSON byte array also works) or a BIP-39 phrase (Solana path `m/44'/501'/i'/0'`, SLIP-0010). PBKDF2-SHA256 (600k) -> AES-256-GCM, fresh salt and IV per write. Unlocked secrets live in `chrome.storage.session` (memory only, trusted contexts only); auto-lock via `chrome.alarms` (1 min to 4 h). Multiple accounts (derive, import, generate). Seeds never leave the service worker: the popup only gets addresses, and there is no key export. |
| Networks | Localnet preset (Solana `http://127.0.0.1:10600`, node `http://127.0.0.1:10630`), editable; custom networks; bridge / storefront program ids and an optional mint override are configurable (the mint is otherwise read from the bridge Config). |
| Balances | SOL, the token on Solana (owner's Token-2022 ATA), native coin + nonce on the agent chain, chain height and finality. |
| Send | SOL (System transfer), token (create-ATA-idempotent + TransferChecked), native (Transfer tx, fee = node `min_fee`). |
| Bridge in | `bridge.deposit(amount, agent_recipient)` on Solana; tracked until `getDeposit(solana_nonce)` reports the relayer unlock. Retries if another deposit took the nonce. |
| Bridge out | `BridgeLock` tx (amount a positive multiple of 1000); tracked through inclusion, the withdrawal nonce (`getBridgeEvents`), K-deep confirmations (progress bar), then the Solana `processed` PDA `["processed", 1, nonce]` that marks the release. Runs in the background (alarm every minute) as well. |
| Agents | `RegisterAgent` (name + URI), profile shown from `getAgent`. |
| Jobs | Browse (`listJobs` with status filters, paging), job detail (`getJob`): status, terms, mining progress, result, and a local check that `job_header` = `header_id || terms_hash || 0 || 0` and `job_key` = blake3(header, config). |
| Post job | Reads A (m x k) and B (k x n) int8 files, computes `job_id`, `terms_hash`, the 76-byte header, `job_key`, `hash_a`, `hash_b` in TS (blake3), previews, then sends `PostJob`. Byte parity with the `pearl-job` CLI is tested. |
| Storefront | Reads A and B like Post job and computes `hash_a` / `hash_b` itself against the order header `recipient || order_terms(recipient, m, n, k, max_blocks, deadline_blocks)` (chain/state `order_terms_hash`; parity with the Rust vector `chain/state/vectors/order_terms.json` and with `pearl-job`). Quote (curve or PumpSwap, slippage from the storefront config), then `open_order` + `buy_job` / `buy_job_amm` (two txs; the pair is over the 1232-byte limit together). |
| Mining | Scans recent canonical blocks (`getBlock`, depth configurable) for blocks whose `miner` is this account: count, job vs fallback, matured (>= K deep) vs immature, fees. |
| Activity | Local log of everything sent from the wallet (both chains) + `getSignaturesForAddress` on Solana. |

## dApp providers

Both are injected at `document_start` by a MAIN-world content script and talk to the
service worker through an isolated content script. The background takes the
requesting origin from Chrome's `MessageSender`, never from the page. Every
connect and every signature opens an approval window (376 x 640) that shows the
origin and a decoded summary of the transaction. Permissions are per origin and
per account; list and revoke them in Settings, Connected sites. Errors carry EIP-1193
style codes: `4001` user rejected, `4100` not connected / locked, `4901` wrong
chain, `-32602` bad params.

### Solana: Wallet Standard

Registered via the `wallet-standard:register-wallet` / `app-ready` protocol, so any
`@wallet-standard/app` or `@solana/wallet-adapter` app sees it.

- `name`: `APP_NAME`; `chains`: `solana:localnet`, `solana:devnet`, `solana:testnet`, `solana:mainnet`
  (the account carries the active network's chain, `solana:localnet` on the preset).
- Features: `standard:connect` (`{ silent }`), `standard:disconnect`, `standard:events`
  (`change`), `solana:signTransaction`, `solana:signAndSendTransaction` (legacy and v0;
  sent through the wallet's configured RPC), `solana:signMessage`.
- The approval decodes System transfers, Token / Token-2022 transfers, approvals and
  authority changes (flagged), ATA creation, compute budget, `bridge.deposit`, and the
  storefront's `open_order` / `buy_job`; anything else shows program, account count and
  data size. It warns when the fee payer is not you or lookup tables are used.
- `signMessage` signs raw ed25519 (as Solana wallets do) but refuses bytes that parse
  as a Solana transaction message or carry agent-chain tx framing.

```js
const wallet = wallets.find((w) => w.name === "Pearl Wallet");
const { accounts } = await wallet.features["standard:connect"].connect();
const [{ signature }] = await wallet.features["solana:signAndSendTransaction"]
  .signAndSendTransaction({ account: accounts[0], transaction: txBytes, chain: "solana:localnet" });
```

### Agent chain: `window.pearl`

```ts
window.pearl.isPearlWallet                    // true
await window.pearl.connect()                  // { address } (base58); { onlyIfTrusted: true } never prompts
await window.pearl.disconnect()
await window.pearl.getAccount()               // { address, balance, nonce, chainId, height }; balance in base units (9 dp), decimal string

// Either unsigned signing bytes (Uint8Array or base64 of: version | chain_id | from | nonce | fee | kind | body)...
await window.pearl.signTransaction(bytes)     // { signedTx: base64, txHash: hex }
// ...or a JSON description; the wallet fills chain_id, nonce and fee (min_fee):
await window.pearl.signAndSendTransaction({ transaction: { kind: "Transfer", to: "<base58>", amount: "2500000000" } })
                                              // { txHash: hex }
// JSON kinds: Transfer {to, amount}, RegisterAgent {name, uri}, BridgeLock {amount, solanaRecipient?},
//             SubmitResult {jobId, resultHash, uri}, ExpireJob {jobId}; optional fee. Other kinds: pass bytes.

await window.pearl.signMessage("hello")       // { signature: Uint8Array, address, namespace: "pearl-sol:message:v1" }
window.pearl.on("accountChanged", ({ address }) => {}); window.pearl.on("disconnect", () => {});
```

Signing rules:
- Transactions: `sig = ed25519(varint(15) || "pearl-sol:tx:v1" || signing bytes)`
  (Commonware framing, SPEC 2). The wallet checks `from` is the connected account and
  `chain_id` matches the node before it asks.
- `signMessage`: `sig = ed25519(varint(20) || "pearl-sol:message:v1" || message)`, a
  separate namespace, so a signed message can never be a valid transaction signature.
  Verify with Commonware `verify(Some(b"pearl-sol:message:v1"), msg, sig)` or by
  rebuilding the framed bytes.

## Layout

```
src/core      bytes, ed25519/blake3/sha256, Commonware framing, vault crypto, message signing
src/chain     agent-chain tx codec, domain messages and ids, Pearl job commitments, node RPC
src/solana    tx compile/parse (legacy + v0), PDAs, bridge/storefront/token instructions, RPC
src/background service worker: keyring, wallet ops + pending tracker, dApp permissions/approvals
src/content   isolated-world relay;  src/inpage  window.pearl + Wallet Standard
src/ui        popup (360 x 600) and approval window; tokens in styles.css (light + dark), Geist only
test/         vitest suites;  test/fixtures/job  pearl-job output used for parity
scripts/      build.mjs, ui-check.mjs (Playwright), mock-rpc.mjs
```

## Known limits

- Mining rewards: `getBlock` in docs/RPC.md has no subsidy field, so the wallet sums
  fees only (it uses a `reward` / `subsidy` field if a node adds one).
- Agent-chain activity is the wallet's own log (the node has no history-by-address
  method).
- No key export by design; back up the original key or phrase.
