import type { ActivityItem, WalletState } from "../../shared/types";
import { CHAIN_NAME, NATIVE_DECIMALS, SOL_DECIMALS, SPL_DECIMALS, TOKEN_SYMBOL, formatUnits, short } from "../../shared/constants";

function amountOf(a: ActivityItem): string | null {
  if (!a.amount) return null;
  if (a.kind === "send-sol" || a.kind === "storefront-buy") return `${formatUnits(a.amount, SOL_DECIMALS, 4)} SOL`;
  if (a.kind === "send-token" || a.kind === "bridge-in") return `${formatUnits(a.amount, SPL_DECIMALS, 4)} ${TOKEN_SYMBOL}`;
  return `${formatUnits(a.amount, NATIVE_DECIMALS, 4)} ${TOKEN_SYMBOL}`;
}
import { call } from "../api";
import { Empty, Icon, Notice, ago, usePoll } from "../kit";

const glyph = (a: ActivityItem) => (a.kind.startsWith("bridge") ? Icon.bridge : a.kind === "post-job" || a.kind.startsWith("storefront") ? Icon.cube : a.kind === "register" ? Icon.agent : a.kind === "dapp" ? Icon.globe : Icon.send);

export function Activity({ st }: { st: WalletState }) {
  const act = usePoll(() => call<{ items: ActivityItem[]; solanaError?: string }>("activity"), 6000, [st.selected?.address, st.network.id]);
  const items = act.data?.items ?? [];
  return (
    <div className="scroll">
      <div className="section-title"><span className="eyebrow">Activity</span><span className="small faint">{st.network.name}</span></div>
      {act.data?.solanaError && <div style={{ marginBottom: 10 }}><Notice kind="warn">Solana history unavailable: {act.data.solanaError}</Notice></div>}
      {!act.data ? (
        <div className="card divided">{[0, 1, 2, 3].map((i) => <div key={i} className="list-row"><div className="glyph" /><div className="grow"><span className="skel" style={{ width: 150 }} /></div></div>)}</div>
      ) : items.length === 0 ? (
        <div className="card"><Empty title="No activity yet" body={`Sends, bridge transfers, jobs and dApp approvals on both chains show up here.`} /></div>
      ) : (
        <div className="card divided" data-testid="activity-list">
          {items.map((a) => (
            <div className="list-row" key={a.id} title={a.ref}>
              <div className="glyph">{glyph(a)}</div>
              <div className="grow">
                <div style={{ fontWeight: 600 }} className="ell">{a.title}</div>
                <div className="small faint ell">
                  {a.chain === "solana" ? "Solana" : CHAIN_NAME} · {short(a.ref, 5)}{a.origin ? ` · ${new URL(a.origin).host}` : ""}
                </div>
                {a.chain === "pearl" && a.status === "preconfirmed" && a.detail && <div className="tiny faint" data-testid="preconf-note">{a.detail}</div>}
              </div>
              <div className="right">
                {amountOf(a) && <div className="num small" style={{ fontWeight: 600, marginBottom: 3 }}>{a.kind === "bridge-in" || a.kind === "bridge-out" ? "" : "-"}{amountOf(a)}</div>}
                <span className={`badge ${a.status === "confirmed" || a.status === "final" ? "ok" : a.status === "failed" ? "bad" : "warn"}`} title={a.detail}>{a.chain === "pearl" && a.status === "confirmed" ? "included" : a.status}</span>
                <div className="tiny faint" style={{ marginTop: 3 }}>{ago(a.at)}</div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
