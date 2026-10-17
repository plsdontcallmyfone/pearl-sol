import { useEffect, useState } from "react";
import { APP_NAME, CHAIN_NAME, short } from "../shared/constants";
import type { ApprovalRequest, WalletState } from "../shared/types";
import { applyTheme, call } from "./api";
import { Btn, Icon, KV, Notice } from "./kit";
import { Unlock } from "./screens/Onboard";

const TITLES: Record<ApprovalRequest["kind"], string> = {
  connect: "Connection request",
  "solana-sign": "Sign a Solana transaction",
  "solana-sign-send": "Approve a Solana transaction",
  "solana-message": "Sign a message (Solana)",
  "pearl-sign": `Sign an ${CHAIN_NAME.toLowerCase()} transaction`,
  "pearl-sign-send": `Approve an ${CHAIN_NAME.toLowerCase()} transaction`,
  "pearl-message": `Sign a message (${CHAIN_NAME.toLowerCase()})`,
};

export function Approval() {
  const id = location.hash.slice(1);
  const [req, setReq] = useState<ApprovalRequest | null | undefined>(undefined);
  const [st, setSt] = useState<WalletState | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    call<ApprovalRequest | null>("getApproval", { id }).then(setReq, (e) => setErr(e.message));
    call<WalletState>("state").then((s) => {
      setSt(s);
      applyTheme(s.settings.theme);
    });
  }, [id]);

  const decide = async (approve: boolean) => {
    setBusy(true);
    try {
      await call("resolveApproval", { id, approve });
      window.close();
    } catch (e) {
      setErr((e as Error).message);
      setBusy(false);
    }
  };

  if (req === undefined || !st) return <div className="app" />;
  if (req === null)
    return (
      <div className="app">
        <div className="onboard">
          <h1>Request expired</h1>
          <p>The site withdrew it or it was already answered.</p>
          <div className="spacer" />
          <Btn block onClick={() => window.close()}>Close</Btn>
        </div>
      </div>
    );
  let host = req.origin;
  try {
    host = new URL(req.origin).host;
  } catch {
    /* keep raw */
  }
  const insecure = req.origin.startsWith("http://") && !/^http:\/\/(localhost|127\.0\.0\.1)(:|$)/.test(req.origin);
  const verb = req.kind === "connect" ? "Connect" : req.kind.endsWith("send") ? "Approve" : "Sign";
  return (
    <div className="app">
      <div className="topbar">
        <span className="wordmark">{APP_NAME}</span>
        <div className="spacer" />
        <span className="pill" style={{ cursor: "default" }}><span className="dot ok" />{req.network}</span>
      </div>
      <div className="scroll">
        <div className="eyebrow">{TITLES[req.kind]}</div>
        <div className="card card-body" style={{ marginTop: 8 }}>
          <div className="origin-card">
            <div className="favicon">{Icon.globe}</div>
            <div className="grow" style={{ minWidth: 0 }}>
              <div style={{ fontWeight: 650, fontSize: 14 }} className="ell" data-testid="approval-origin">{host}</div>
              <div className="small faint ell">{req.origin}</div>
            </div>
          </div>
        </div>
        {insecure && <div style={{ marginTop: 10 }}><Notice kind="warn">This site is not using HTTPS.</Notice></div>}
        {st.locked ? (
          <div className="card card-body" style={{ marginTop: 10 }}>
            <Unlock compact onDone={setSt} />
          </div>
        ) : req.kind === "connect" ? (
          <div className="card divided" style={{ marginTop: 10 }}>
            <KV k="Account" v={short(req.account, 6)} title={req.account} />
            <KV k="Chains" v={(req.chains ?? []).map((c) => (c === "solana" ? "Solana" : CHAIN_NAME)).join(" + ")} />
          </div>
        ) : null}
        {!st.locked && req.kind === "connect" && (
          <>
            <div className="section-title"><span className="eyebrow">This site will be able to</span></div>
            <div className="card">
              <ul className="perm-list">
                <li className="yes">{Icon.check}<span>See your address and balances on the chains you allow</span></li>
                <li className="yes">{Icon.check}<span>Ask you to sign transactions and messages</span></li>
                <li className="no">{Icon.lock}<span>It cannot move funds or sign anything without a new approval</span></li>
              </ul>
            </div>
            <div className="small faint" style={{ marginTop: 10 }}>Revoke access any time in Settings, Connected sites.</div>
          </>
        )}
        {st.locked || req.kind === "connect" ? null : (
          req.summary && (
            <>
              <div className="title" style={{ marginTop: 14, fontSize: 16 }} data-testid="approval-headline">{req.summary.headline}</div>
              {req.summary.warnings.length > 0 && (
                <div style={{ marginTop: 8 }}>
                  <Notice kind="warn">{req.summary.warnings.map((w, i) => <div key={i}>{w}</div>)}</Notice>
                </div>
              )}
              {req.summary.sections.map((s, i) => (
                <div key={i}>
                  <div className="section-title"><span className="eyebrow">{s.title}</span></div>
                  <div className="card divided">
                    {s.rows.map((r, j) => <KV key={j} k={r.label} v={r.value} title={r.full} />)}
                  </div>
                </div>
              ))}
            </>
          )
        )}
        {!st.locked && req.kind !== "connect" && <div className="small faint" style={{ marginTop: 10 }}>Signing as <span className="num">{short(req.account, 6)}</span></div>}
        {err && <div style={{ marginTop: 10 }}><Notice kind="bad">{err}</Notice></div>}
      </div>
      <div className="footer-bar btn-row">
        <Btn onClick={() => decide(false)} disabled={busy} testid="reject">Reject</Btn>
        <Btn kind="primary" onClick={() => decide(true)} busy={busy} disabled={st.locked} testid="approve">{st.locked ? "Unlock first" : verb}</Btn>
      </div>
    </div>
  );
}
