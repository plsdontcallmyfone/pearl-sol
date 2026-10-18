// A minimal Solana transaction page, so a signature linked from a block or job opens
// without a third-party explorer (localnet signatures are unknown to public ones).

import { BRAND } from "./brand.ts";
import * as D from "./decode.ts";
import { fmtInt, formatUnits } from "./math.ts";
import type { Summary } from "./models.ts";
import { esc, page } from "./views.ts";

const PROGRAMS: Record<string, string> = {
  [D.STOREFRONT_ID]: "storefront",
  [D.LIGHTCLIENT_ID]: "light client",
  [D.BRIDGE_ID]: "bridge",
  "11111111111111111111111111111111": "System",
  ComputeBudget111111111111111111111111111111: "Compute budget",
  Ed25519SigVerify111111111111111111111111111: "ed25519 precompile",
  "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P": "pump",
  pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA: "PumpSwap",
  TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb: "Token-2022",
  TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA: "Token",
  ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL: "Associated token",
};

export function solTxPage(sm: Summary, sig: string, tx: any) {
  const msg = tx.transaction.message;
  const keys: string[] = [...msg.accountKeys, ...(tx.meta?.loadedAddresses?.writable ?? []), ...(tx.meta?.loadedAddresses?.readonly ?? [])];
  const logs: string[] = tx.meta?.logMessages ?? [];
  const progs = [...new Set((msg.instructions ?? []).map((i: any) => keys[i.programIdIndex]))] as string[];
  const ours = [D.STOREFRONT_ID, D.LIGHTCLIENT_ID, D.BRIDGE_ID].flatMap((p) => D.anchorInstructions(logs, p).map((n) => `${PROGRAMS[p]}: ${n}`));
  const ok = !tx.meta?.err;
  const body = `<dl class="kv">
<div><dt>Signature</dt><dd><span class="hash wrap">${esc(sig)}</span></dd></div>
<div><dt>Status</dt><dd>${ok ? `<span class="badge b-ok">success</span>` : `<span class="badge b-bad">failed</span>`}</dd></div>
<div><dt>Slot</dt><dd><span class="num">${fmtInt(tx.slot)}</span></dd></div>
<div><dt>Time</dt><dd>${tx.blockTime ? esc(new Date(tx.blockTime * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC") : "–"}</dd></div>
<div><dt>Fee</dt><dd><span class="num">${formatUnits(BigInt(tx.meta?.fee ?? 0), 9, 9)}</span> SOL</dd></div>
<div><dt>Fee payer</dt><dd><span class="hash">${esc(keys[0])}</span></dd></div>
<div><dt>Programs</dt><dd>${progs.map((p) => `<span class="ix">${esc(PROGRAMS[p] ?? p)}</span>`).join(" ")}</dd></div>
<div><dt>Instructions</dt><dd>${ours.length ? ours.map((x) => `<span class="ix">${esc(x)}</span>`).join(" ") : `<span class="faint">none of ${esc(BRAND.name)}'s programs</span>`}</dd></div>
</dl>`;
  return page(
    `Solana tx ${sig.slice(0, 8)}`,
    `<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Solana tx</span></nav>
<div class="title-row"><h1>Solana transaction</h1><span class="pager"><a href="/api/sol/tx/${esc(sig)}">JSON</a></span></div>
<div class="cards">
<section class="card card-accent"><header class="card-h"><h2>Summary</h2></header>${body}</section>
<section class="card"><header class="card-h"><h2>Program logs</h2></header><ol class="logs">${logs.map((l) => `<li>${esc(l)}</li>`).join("")}</ol></section>
</div>`,
    { summary: sm },
  );
}
