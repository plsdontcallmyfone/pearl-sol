import { useState } from "react";
import { CHAIN_NAME, NATIVE_DECIMALS, SPL_DECIMALS, SPL_TO_NATIVE, TOKEN_SYMBOL, formatUnits, parseUnits, short } from "../../shared/constants";
import type { PendingTransfer, WalletState } from "../../shared/types";
import { call } from "../api";
import { Btn, Empty, Field, Icon, KV, Notice, Seg, ago, usePoll } from "../kit";
import { useBalances } from "./Home";

const STAGES_IN = ["submitted", "relaying", "done"] as const;
const STAGES_OUT = ["submitted", "waiting-final", "relaying", "done"] as const;

function stageLabel(p: PendingTransfer) {
  if (p.stage === "failed") return "Failed";
  if (p.stage === "done") return "Arrived";
  if (p.direction === "in") return p.stage === "submitted" ? "Confirming on Solana" : "Waiting for the relayer unlock";
  if (p.stage === "submitted") return "Waiting for a block";
  if (p.stage === "waiting-final") return `Finalizing ${Math.min(p.confirmations ?? 0, p.finalityK ?? 30)} of ${p.finalityK ?? 30} blocks`;
  return "Final; waiting for the Solana release";
}

export function PendingList({ items }: { items: PendingTransfer[] }) {
  return (
    <div className="card divided" data-testid="pending-list">
      {items.map((p) => {
        const stages: readonly string[] = p.direction === "in" ? STAGES_IN : STAGES_OUT;
        const idx = p.stage === "failed" ? -1 : stages.indexOf(p.stage);
        const k = p.finalityK ?? 30;
        const frac = p.direction === "out" && p.stage === "waiting-final" ? Math.min(1, (p.confirmations ?? 0) / k) : null;
        return (
          <div key={p.id} style={{ padding: "10px 12px" }}>
            <div className="between">
              <div className="row" style={{ minWidth: 0 }}>
                <div className="glyph">{Icon.bridge}</div>
                <div style={{ minWidth: 0 }}>
                  <div style={{ fontWeight: 600 }} className="num ell">
                    {formatUnits(p.amountSpl, SPL_DECIMALS, 6)} {TOKEN_SYMBOL}
                  </div>
                  <div className="small faint ell">{p.direction === "in" ? `Solana to ${CHAIN_NAME.toLowerCase()}` : `${CHAIN_NAME} to Solana`} · {ago(p.createdAt)}</div>
                </div>
              </div>
              <span className={`badge ${p.stage === "done" ? "ok" : p.stage === "failed" ? "bad" : "warn"}`}>{p.stage === "done" ? "Done" : p.stage === "failed" ? "Failed" : "Pending"}</span>
            </div>
            <div className="steps" style={{ gridTemplateColumns: `repeat(${stages.length}, 1fr)` }}>
              {stages.map((s, i) => (
                <div key={s} className={`step ${idx > i || p.stage === "done" ? "done" : idx === i ? "active" : ""}`} />
              ))}
            </div>
            <div className="between small" style={{ marginTop: 6 }}>
              <span className="subtle num">{stageLabel(p)}</span>
              {frac !== null && <span className="faint num">{Math.round(frac * 100)}%</span>}
            </div>
            {frac !== null && (
              <div className="progress" style={{ marginTop: 6 }}>
                <div style={{ width: `${frac * 100}%` }} />
              </div>
            )}
            {p.error && <div className="error-text">{p.error}</div>}
            <div className="tiny faint ell" style={{ marginTop: 4 }} title={p.sourceRef}>
              {p.direction === "in" ? `deposit #${p.depositNonce} · sig ${short(p.sourceRef, 6)}` : `tx ${short(p.sourceRef, 6)}${p.withdrawalNonce !== undefined ? ` · withdrawal #${p.withdrawalNonce}` : ""}${p.sourceHeight ? ` · height ${p.sourceHeight}` : ""}`}
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function Bridge({ st, toast }: { st: WalletState; toast: (m: string) => void }) {
  const [dir, setDir] = useState<"in" | "out">("in");
  const [amt, setAmt] = useState("");
  const [recipient, setRecipient] = useState("");
  const [adv, setAdv] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const bal = useBalances(st.selected?.address, st.network.id);
  const pend = usePoll(() => call<PendingTransfer[]>("pending"), 4000, [st.selected?.address, st.network.id]);
  const b = bal.data;
  const k = b?.chain?.finalityK ?? 30;

  let spl: bigint | null = null;
  let amtErr: string | null = null;
  if (amt) {
    try {
      spl = parseUnits(amt, SPL_DECIMALS);
      if (spl <= 0n) amtErr = "Enter more than zero.";
      const have = dir === "in" ? (b?.token != null ? BigInt(b.token) : null) : b?.native != null ? BigInt(b.native) / SPL_TO_NATIVE : null;
      if (!amtErr && have !== null && spl > have) amtErr = "More than the available balance.";
    } catch (e) {
      amtErr = (e as Error).message.replace(/at most 6/, "at most 6 (the bridge moves whole SPL units: 1 = 1000 chain units)");
    }
  }
  const haveStr = dir === "in"
    ? b?.token != null ? formatUnits(b.token, b.tokenDecimals, 6) : "-"
    : b?.native != null ? formatUnits(b.native, NATIVE_DECIMALS, 6) : "-";
  const reason = !spl ? "Enter an amount" : amtErr ? "Fix the amount" : null;

  const submit = async () => {
    setBusy(true);
    setErr(null);
    try {
      if (dir === "in") await call("bridgeIn", { amountSpl: spl!.toString(), recipient: recipient.trim() || undefined });
      else await call("bridgeOut", { amount: (spl! * SPL_TO_NATIVE).toString(), recipient: recipient.trim() || undefined });
      setAmt("");
      pend.reload();
      toast(dir === "in" ? "Deposit sent on Solana" : "Lock sent on the agent chain");
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="scroll">
      <Seg value={dir} onChange={(v) => { setDir(v); setErr(null); }} options={[{ value: "in", label: "Solana to agent chain" }, { value: "out", label: "Agent chain to Solana" }]} />
      <div className="card" style={{ marginTop: 10 }}>
        <div className="card-body">
          <Field label="Amount" aside={<span className="small faint num">Available {haveStr} {TOKEN_SYMBOL}</span>} error={amtErr}>
            <div className="input-wrap">
              <input className="input" value={amt} onChange={(e) => setAmt(e.target.value)} placeholder="0.00" inputMode="decimal" data-testid="bridge-amount" />
              <div className="input-suffix">
                <button className="btn small ghost" onClick={() => {
                  if (dir === "in" && b?.token) setAmt(formatUnits(b.token, SPL_DECIMALS, 6).replace(/,/g, ""));
                  if (dir === "out" && b?.native) setAmt(formatUnits((BigInt(b.native) / SPL_TO_NATIVE) * SPL_TO_NATIVE, NATIVE_DECIMALS, 6).replace(/,/g, ""));
                }}>Max</button>
              </div>
            </div>
          </Field>
          {adv ? (
            <Field label={dir === "in" ? "Agent-chain recipient" : "Solana recipient"} hint="Leave empty to bridge to this same address.">
              <input className="input" value={recipient} onChange={(e) => setRecipient(e.target.value)} placeholder={short(st.selected!.address, 8)} spellCheck={false} />
            </Field>
          ) : (
            <button className="link small" style={{ marginTop: 8 }} onClick={() => setAdv(true)}>Send to a different address</button>
          )}
        </div>
        <div className="divided" style={{ borderTop: "1px solid var(--line-soft)" }}>
          <KV k="You receive" v={spl && !amtErr ? `${formatUnits(spl * SPL_TO_NATIVE, NATIVE_DECIMALS, 9)} ${TOKEN_SYMBOL}` : "-"} />
          <KV k="Unit conversion" v="1 SPL unit = 1000 chain units" />
          <KV k={dir === "in" ? "Arrives after" : "Release after"} v={dir === "in" ? "Solana confirmation + relayer unlock" : `${k} blocks (finality) + relayer release`} />
          <KV k="Program" v={dir === "in" ? `bridge.deposit ${short(st.network.programs.bridge, 4)}` : "BridgeLock tx (kind 7)"} />
        </div>
      </div>
      {dir === "out" && (
        <div style={{ marginTop: 10 }}>
          <Notice>The Solana release only happens once the lock is {k} blocks deep. You can close the popup; progress is tracked in the background.</Notice>
        </div>
      )}
      {err && <div style={{ marginTop: 10 }}><Notice kind="bad">{err}</Notice></div>}
      <div style={{ marginTop: 12 }}>
        <Btn kind="primary" block disabled={!!reason} busy={busy} onClick={submit} testid="bridge-submit">
          {reason ?? (dir === "in" ? `Bridge ${amt} ${TOKEN_SYMBOL} in` : `Bridge ${amt} ${TOKEN_SYMBOL} out`)}
        </Btn>
      </div>
      <div className="section-title"><span className="eyebrow">Transfers</span>{pend.error && <span className="small faint">{pend.error}</span>}</div>
      {(pend.data ?? []).length ? <PendingList items={pend.data!} /> : (
        <div className="card"><Empty title="Nothing in flight" body="Transfers you start here are tracked until they land on the other chain." /></div>
      )}
    </div>
  );
}
