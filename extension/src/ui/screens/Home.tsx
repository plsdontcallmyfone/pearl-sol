import { useState } from "react";
import { CHAIN_NAME, NATIVE_DECIMALS, SOL_DECIMALS, SPL_TO_NATIVE, TOKEN_SYMBOL, formatFixed, formatUnits, parseUnits, short } from "../../shared/constants";
import type { Balances, PendingTransfer, WalletState } from "../../shared/types";
import { call } from "../api";
import { Btn, CopyBtn, Field, Icon, KV, Notice, Seg, Sub, usePoll } from "../kit";
import { PendingList } from "./Bridge";

export type HomeView = "home" | "send" | "receive";

export function useBalances(addr: string | undefined, netId: string) {
  return usePoll(() => call<Balances>("balances"), 6000, [addr, netId]);
}

function Amount({ v, decimals, unit, places = 4 }: { v: string | null; decimals: number; unit: string; places?: number }) {
  return (
    <span className="num">
      {v === null ? <span className="skel" /> : formatFixed(v, decimals, places)}
      <span className="unit">{unit}</span>
    </span>
  );
}

export function Home({ st, go, toBridge, toTab }: { st: WalletState; go: (v: HomeView) => void; toBridge: () => void; toTab: (t: string) => void }) {
  const bal = useBalances(st.selected?.address, st.network.id);
  const pend = usePoll(() => call<PendingTransfer[]>("pending"), 5000, [st.selected?.address, st.network.id]);
  const b = bal.data;
  const totalToken = b && (b.token !== null || b.native !== null)
    ? (BigInt(b.token ?? "0") * SPL_TO_NATIVE + BigInt(b.native ?? "0")).toString()
    : null;
  const live = (pend.data ?? []).filter((p) => p.stage !== "done" && p.stage !== "failed");
  return (
    <div className="scroll">
      <div className="card" data-testid="balances">
        <div className="hero">
          <div className="eyebrow">Total {TOKEN_SYMBOL}, both chains</div>
          <div className="hero-amount num" style={{ marginTop: 6 }}>
            {totalToken === null ? <span className="skel" style={{ width: 140, height: 24 }} /> : formatFixed(totalToken, NATIVE_DECIMALS, 4)}
            <span className="hero-unit">{TOKEN_SYMBOL}</span>
          </div>
        </div>
        <div className="chain-split">
          <div>
            <div className="row" style={{ gap: 6 }}>
              <span className={`dot ${b?.errors.solana ? "bad" : b ? "ok" : ""}`} />
              <span className="eyebrow">Solana</span>
            </div>
            <div className="asset-amount" style={{ marginTop: 6 }}>
              <Amount v={b?.errors.solana ? "0" : b?.token ?? null} decimals={b?.tokenDecimals ?? 6} unit={TOKEN_SYMBOL} places={2} />
            </div>
            <div className="small subtle" style={{ marginTop: 2 }}>
              <Amount v={b?.errors.solana ? "0" : b?.sol ?? null} decimals={SOL_DECIMALS} unit="SOL" />
            </div>
          </div>
          <div>
            <div className="row" style={{ gap: 6 }}>
              <span className={`dot ${b?.errors.chain ? "bad" : b ? "ok" : ""}`} />
              <span className="eyebrow">{CHAIN_NAME}</span>
            </div>
            <div className="asset-amount" style={{ marginTop: 6 }}>
              <Amount v={b?.errors.chain ? "0" : b?.native ?? null} decimals={NATIVE_DECIMALS} unit={TOKEN_SYMBOL} places={2} />
            </div>
            <div className="small subtle num" style={{ marginTop: 2 }}>
              {b?.chain ? <>height {b.chain.height.toLocaleString("en-US")}</> : b?.errors.chain ? "offline" : <span className="skel" />}
            </div>
          </div>
        </div>
      </div>

      {(b?.errors.solana || b?.errors.chain || b?.errors.bridge) && (
        <div style={{ marginTop: 10 }}>
          <Notice kind="warn">
            {b.errors.solana && <div>Solana RPC: {b.errors.solana}. Start it with scripts/localnet.sh up token, or change the endpoint in Settings.</div>}
            {b.errors.chain && <div>Agent chain node: {b.errors.chain}. Start the node (scripts/devnet.sh) or change the endpoint in Settings.</div>}
            {b.errors.bridge && !b.errors.solana && <div>Token: {b.errors.bridge}</div>}
          </Notice>
        </div>
      )}

      <div className="actions">
        <button className="action" onClick={() => go("send")} data-testid="act-send">{Icon.send}Send</button>
        <button className="action" onClick={() => go("receive")} data-testid="act-receive">{Icon.receive}Receive</button>
        <button className="action" onClick={toBridge} data-testid="act-bridge">{Icon.bridge}Bridge</button>
        <button className="action" onClick={() => toTab("jobs")} data-testid="act-jobs">{Icon.jobs}Jobs</button>
      </div>

      <div className="section-title">
        <span className="eyebrow">Bridge transfers</span>
        {live.length > 0 && <span className="badge warn num">{live.length} in flight</span>}
      </div>
      {(pend.data ?? []).length ? (
        <PendingList items={(pend.data ?? []).slice(0, 3)} />
      ) : (
        <div className="card">
          <div className="empty compact">
            <div className="glyph">{Icon.bridge}</div>
            <div className="grow">
              <div style={{ fontWeight: 600 }}>No transfers in flight</div>
              <p className="small subtle">Bridge {TOKEN_SYMBOL} either way; progress shows here.</p>
            </div>
            <Btn small onClick={toBridge}>Bridge</Btn>
          </div>
        </div>
      )}

      <div className="section-title"><span className="eyebrow">Agent</span></div>
      <div className="card divided">
        <div className="list-row click" onClick={() => toTab("jobs")} role="button" tabIndex={0}>
          <div className="glyph">{Icon.agent}</div>
          <div className="grow">
            <div style={{ fontWeight: 600 }} className="ell">{b?.agent ? b.agent.name : "Not registered"}</div>
            <div className="small faint ell">{b?.agent ? `registered at height ${b.agent.registered_height}` : "Register to post and take jobs under a name"}</div>
          </div>
          <span className="faint">{Icon.chevron}</span>
        </div>
        <KV k="Chain nonce" v={b?.nonce ?? "-"} />
        <KV k="Finalized height" v={b?.chain ? `${b.chain.finalizedHeight.toLocaleString("en-US")} (K ${b.chain.finalityK})` : "-"} />
      </div>
    </div>
  );
}

type Asset = "sol" | "token" | "native";

export function Send({ st, back, done }: { st: WalletState; back: () => void; done: (msg: string) => void }) {
  const bal = useBalances(st.selected?.address, st.network.id);
  const [asset, setAsset] = useState<Asset>("native");
  const [to, setTo] = useState("");
  const [amt, setAmt] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const b = bal.data;
  const dec = asset === "sol" ? SOL_DECIMALS : asset === "token" ? b?.tokenDecimals ?? 6 : NATIVE_DECIMALS;
  const have = asset === "sol" ? b?.sol : asset === "token" ? b?.token : b?.native;
  const unit = asset === "sol" ? "SOL" : TOKEN_SYMBOL;
  let parsed: bigint | null = null;
  let amtErr: string | null = null;
  if (amt) {
    try {
      parsed = parseUnits(amt, dec);
      if (have != null && parsed > BigInt(have)) amtErr = "More than the available balance.";
    } catch (e) {
      amtErr = (e as Error).message;
    }
  }
  const toOk = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(to.trim());
  const reason = !toOk ? "Enter a recipient" : !parsed ? "Enter an amount" : amtErr ? "Fix the amount" : null;
  const submit = async () => {
    setBusy(true);
    setErr(null);
    try {
      const type = asset === "sol" ? "sendSol" : asset === "token" ? "sendToken" : "sendNative";
      const ref = await call<string>(type, { to: to.trim(), amount: parsed!.toString() });
      done(`Sent. ${asset === "native" ? "Tx" : "Signature"} ${short(ref, 6)}`);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Sub title="Send" onBack={back} />
      <div className="scroll">
        <Seg<Asset>
          value={asset}
          onChange={(v) => { setAsset(v); setAmt(""); }}
          options={[{ value: "native", label: `${TOKEN_SYMBOL}, agent` }, { value: "token", label: `${TOKEN_SYMBOL}, Solana` }, { value: "sol", label: "SOL" }]}
        />
        <div className="card card-body" style={{ marginTop: 10 }}>
          <Field label="Recipient" hint="Any base58 address. The same address works on both chains.">
            <input className="input" value={to} onChange={(e) => setTo(e.target.value)} placeholder="Address" spellCheck={false} data-testid="send-to" />
          </Field>
          <Field
            label="Amount"
            aside={<span className="small faint num">Available {have != null ? formatUnits(have, dec, 6) : "-"} {unit}</span>}
            error={amtErr}
          >
            <div className="input-wrap">
              <input className="input" value={amt} onChange={(e) => setAmt(e.target.value)} placeholder="0.00" inputMode="decimal" data-testid="send-amount" />
              <div className="input-suffix">
                <button className="btn small ghost" onClick={(e) => { e.preventDefault(); if (have) setAmt(formatUnits(have, dec, dec).replace(/,/g, "")); }}>Max</button>
              </div>
            </div>
          </Field>
        </div>
        <div className="card divided" style={{ marginTop: 10 }}>
          <KV k="Network" v={asset === "native" ? CHAIN_NAME : `Solana (${st.network.solanaChain.split(":")[1]})`} />
          <KV k="Fee" v={asset === "native" ? "chain min_fee, paid to the miner" : "Solana base fee"} />
          {asset === "token" && <KV k="Recipient token account" v="created if missing" />}
        </div>
        {err && <div style={{ marginTop: 10 }}><Notice kind="bad">{err}</Notice></div>}
        <div style={{ marginTop: 12 }}>
          <Btn kind="primary" block disabled={!!reason} busy={busy} onClick={submit} testid="send-submit">{reason ?? `Send ${amt} ${unit}`}</Btn>
        </div>
      </div>
    </>
  );
}

export function Receive({ st, back }: { st: WalletState; back: () => void }) {
  const a = st.selected!.address;
  return (
    <>
      <Sub title="Receive" onBack={back} />
      <div className="scroll">
        <div className="card card-body">
          <div className="eyebrow">Your address</div>
          <div className="addr-big num" style={{ marginTop: 8 }} data-testid="receive-addr">
            {a.match(/.{1,4}/g)!.map((g, i) => (
              <span key={i} style={{ marginRight: 6, color: i % 2 ? "var(--dim)" : undefined }}>{g}</span>
            ))}
          </div>
          <div style={{ marginTop: 12 }}><CopyBtn text={a} label="Copy address" /></div>
        </div>
        <div className="card divided" style={{ marginTop: 10 }}>
          <div className="list-row">
            <div className="glyph">S</div>
            <div className="grow">
              <div style={{ fontWeight: 600 }}>Solana</div>
              <div className="small subtle">SOL and {TOKEN_SYMBOL} (Token-2022). Token arrives in your associated token account.</div>
            </div>
          </div>
          <div className="list-row">
            <div className="glyph">A</div>
            <div className="grow">
              <div style={{ fontWeight: 600 }}>{CHAIN_NAME}</div>
              <div className="small subtle">Native {TOKEN_SYMBOL} (9 decimals). Same address, same key.</div>
            </div>
          </div>
        </div>
        <div style={{ marginTop: 10 }}>
          <Notice>Both chains use the same ed25519 key, so this one address receives on either. Check which chain the sender uses.</Notice>
        </div>
      </div>
    </>
  );
}
