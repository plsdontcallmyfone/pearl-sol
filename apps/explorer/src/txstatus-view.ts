// Status cell of the tx page (docs/SPEED.md 2.4): pending / preconfirmed with its weak-block
// evidence / included N of K / final, plus the one-line risk note of SPEED.md 2.5.

import { badge, esc } from "./views.ts";
import { fmtAge, type TxStatus } from "./txstatus.ts";

export function txStatusCell(s: TxStatus | null): string {
  if (!s) return `<span class="faint">unknown</span>`;
  const note = `<div class="faint" style="margin-top:4px">${esc(s.risk)}</div>`;
  switch (s.status) {
    case "pending":
      return `${badge("neutral", "pending in the mempool")}${note}`;
    case "preconfirmed": {
      const p = s.preconf;
      const kind = p && p.confidence !== "low" ? "accent" : "warn";
      const facts = p
        ? ` <span class="faint">${esc(`${p.weak_count} weak block${p.weak_count === 1 ? "" : "s"} from ${p.weak_miners} miner${p.weak_miners === 1 ? "" : "s"}, ${Math.round(p.weak_share * 100)}% share, latest ${fmtAge(p.latest_age_ms)}`)}</span>`
        : "";
      return `${badge(kind, `preconfirmed, ${p?.confidence ?? "low"} confidence`)}${facts}${note}`;
    }
    case "included": {
      const conf = s.confirmations !== null ? ` <span class="faint">${esc(`${s.confirmations}${s.finality_k !== null ? ` of ${s.finality_k}` : ""} confirmations`)}</span>` : "";
      const blk = s.height !== null ? ` <a href="/block/${s.height}">#${esc(String(s.height))}</a>` : "";
      return `${badge("ok", "included")}${blk}${conf}${note}`;
    }
    case "final": {
      const blk = s.height !== null ? ` <a href="/block/${s.height}">#${esc(String(s.height))}</a>` : "";
      return `${badge("ok", "final")}${blk}${note}`;
    }
  }
}
