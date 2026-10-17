import { useState } from "react";
import { APP_NAME, short } from "../../shared/constants";
import { Network, SOLANA_CHAINS } from "../../shared/networks";
import type { Permission, WalletState } from "../../shared/types";
import { applyTheme, call } from "../api";
import { Btn, Empty, Field, Icon, KV, Notice, Seg, Sub, ago, initials, usePoll } from "../kit";

type View = "main" | "network" | "accounts" | "sites" | "add";

export function Settings({ st, setSt, toast }: { st: WalletState; setSt: (s: WalletState) => void; toast: (m: string) => void }) {
  const [view, setView] = useState<View>("main");
  const [editNet, setEditNet] = useState<Network | null>(null);
  const sites = usePoll(() => call<Permission[]>("permissions"), 0, [view]);
  if (view === "network" && editNet) return <NetworkEditor net={editNet} back={() => setView("main")} save={async (n) => { setSt(await call("saveNetwork", { network: n })); setView("main"); toast("Network saved"); }} remove={editNet.builtin ? undefined : async () => { setSt(await call("deleteNetwork", { id: editNet.id })); setView("main"); }} />;
  if (view === "accounts") return <Accounts st={st} setSt={setSt} back={() => setView("main")} add={() => setView("add")} />;
  if (view === "add") return <AddAccount st={st} setSt={setSt} back={() => setView("accounts")} toast={toast} />;
  if (view === "sites") return <Sites list={sites.data ?? []} back={() => setView("main")} revoke={async (o) => { await call("revoke", { origin: o }); sites.reload(); toast(`Disconnected ${o}`); }} />;
  return (
    <div className="scroll">
      <div className="section-title"><span className="eyebrow">Network</span></div>
      <div className="card divided">
        {st.networks.map((n) => (
          <div key={n.id} className="list-row click" role="button" tabIndex={0} onClick={async () => setSt(await call("selectNetwork", { id: n.id }))}>
            <div className="glyph">{Icon.globe}</div>
            <div className="grow">
              <div style={{ fontWeight: 600 }}>{n.name}</div>
              <div className="small faint ell num">{n.solanaRpc.replace(/^https?:\/\//, "")} · {n.chainRpc.replace(/^https?:\/\//, "")}</div>
            </div>
            {n.id === st.network.id && <span className="badge accent">Active</span>}
            <button className="btn small ghost" onClick={(e) => { e.stopPropagation(); setEditNet(n); setView("network"); }}>Edit</button>
          </div>
        ))}
        <div className="list-row click" role="button" tabIndex={0} onClick={() => { setEditNet({ ...st.network, id: `custom-${Date.now()}`, name: "Custom", builtin: false }); setView("network"); }}>
          <div className="glyph">{Icon.plus}</div>
          <div className="grow" style={{ fontWeight: 600 }}>Add custom network</div>
        </div>
      </div>

      <div className="section-title"><span className="eyebrow">Wallet</span></div>
      <div className="card divided">
        <div className="list-row click" role="button" tabIndex={0} onClick={() => setView("accounts")}>
          <div className="glyph">{Icon.agent}</div>
          <div className="grow"><div style={{ fontWeight: 600 }}>Accounts</div><div className="small faint">{st.accounts.length} in this wallet</div></div>
          <span className="faint">{Icon.chevron}</span>
        </div>
        <div className="list-row click" role="button" tabIndex={0} onClick={() => setView("sites")} data-testid="sites-row">
          <div className="glyph">{Icon.globe}</div>
          <div className="grow"><div style={{ fontWeight: 600 }}>Connected sites</div><div className="small faint">{sites.data ? `${sites.data.length} with access` : " "}</div></div>
          <span className="faint">{Icon.chevron}</span>
        </div>
        <div className="list-row">
          <div className="glyph">{Icon.lock}</div>
          <div className="grow"><div style={{ fontWeight: 600 }}>Auto-lock</div><div className="small faint">after inactivity</div></div>
          <select className="select" style={{ width: 110, height: 32 }} value={st.settings.autoLockMinutes} onChange={async (e) => setSt(await call("saveSettings", { settings: { autoLockMinutes: Number(e.target.value) } }))}>
            {[1, 5, 15, 30, 60, 240].map((m) => <option key={m} value={m}>{m < 60 ? `${m} min` : `${m / 60} h`}</option>)}
          </select>
        </div>
        <div className="list-row">
          <div className="glyph">{Icon.settings}</div>
          <div className="grow" style={{ fontWeight: 600 }}>Theme</div>
          <div style={{ width: 170 }}>
            <Seg value={st.settings.theme} onChange={async (t) => { applyTheme(t); setSt(await call("saveSettings", { settings: { theme: t } })); }} options={[{ value: "system", label: "Auto" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} />
          </div>
        </div>
      </div>

      <div className="section-title"><span className="eyebrow">Danger zone</span></div>
      <div className="card card-body">
        <div className="small subtle">Removes the encrypted vault, accounts, settings and site permissions from this browser. Funds stay on chain; you need your key or phrase to restore.</div>
        <div style={{ marginTop: 10 }}>
          <Btn kind="danger" small onClick={async () => { if (confirm(`Reset ${APP_NAME} on this browser?`)) setSt(await call("reset")); }}>Reset wallet</Btn>
        </div>
      </div>
      <div className="tiny faint center" style={{ marginTop: 14 }}>{APP_NAME} 0.2.0 · keys stay in the extension background</div>
    </div>
  );
}

function NetworkEditor({ net, back, save, remove }: { net: Network; back: () => void; save: (n: Network) => void; remove?: () => void }) {
  const [n, setN] = useState<Network>(net);
  const [err, setErr] = useState<string | null>(null);
  const set = (patch: Partial<Network>) => setN({ ...n, ...patch });
  const ok = (u: string) => { try { new URL(u); return true; } catch { return false; } };
  const b58 = (s: string) => /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(s);
  const submit = () => {
    if (!ok(n.solanaRpc) || !ok(n.chainRpc)) return setErr("RPC endpoints must be full URLs.");
    if (!b58(n.programs.bridge) || !b58(n.programs.storefront)) return setErr("Program ids must be base58 addresses.");
    if (n.mint && !b58(n.mint)) return setErr("Mint override must be a base58 address.");
    save(n);
  };
  return (
    <>
      <Sub title={net.builtin ? `${net.name} (preset)` : "Network"} onBack={back} />
      <div className="scroll">
        <div className="card card-body">
          <Field label="Name"><input className="input" value={n.name} onChange={(e) => set({ name: e.target.value })} /></Field>
          <Field label="Solana RPC (token chain)"><input className="input" value={n.solanaRpc} onChange={(e) => set({ solanaRpc: e.target.value })} spellCheck={false} data-testid="net-sol" /></Field>
          <Field label="Wallet Standard chain">
            <select className="select" value={n.solanaChain} onChange={(e) => set({ solanaChain: e.target.value as Network["solanaChain"] })}>
              {SOLANA_CHAINS.map((c) => <option key={c} value={c}>{c}</option>)}
            </select>
          </Field>
          <Field label="Agent-chain node RPC"><input className="input" value={n.chainRpc} onChange={(e) => set({ chainRpc: e.target.value })} spellCheck={false} data-testid="net-chain" /></Field>
        </div>
        <div className="section-title"><span className="eyebrow">Programs</span></div>
        <div className="card card-body">
          <Field label="Bridge program"><input className="input" value={n.programs.bridge} onChange={(e) => set({ programs: { ...n.programs, bridge: e.target.value.trim() } })} spellCheck={false} /></Field>
          <Field label="Storefront program"><input className="input" value={n.programs.storefront} onChange={(e) => set({ programs: { ...n.programs, storefront: e.target.value.trim() } })} spellCheck={false} /></Field>
          <Field label="Token mint override" hint="Empty: read from the bridge Config."><input className="input" value={n.mint} onChange={(e) => set({ mint: e.target.value.trim() })} spellCheck={false} /></Field>
          <Field label="Reward scan depth (blocks)"><input className="input" value={n.rewardScanDepth} onChange={(e) => set({ rewardScanDepth: Number(e.target.value) || 0 })} inputMode="numeric" /></Field>
        </div>
        {net.builtin && <div style={{ marginTop: 10 }}><Notice>Saving a preset stores your edited copy; the preset values come back if you delete it.</Notice></div>}
        {err && <div style={{ marginTop: 10 }}><Notice kind="bad">{err}</Notice></div>}
        <div className={remove ? "btn-row" : ""} style={{ marginTop: 12 }}>
          {remove && <Btn kind="danger" onClick={remove}>Delete</Btn>}
          <Btn kind="primary" block onClick={submit} testid="net-save">Save</Btn>
        </div>
      </div>
    </>
  );
}

function Accounts({ st, setSt, back, add }: { st: WalletState; setSt: (s: WalletState) => void; back: () => void; add: () => void }) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [name, setName] = useState("");
  return (
    <>
      <Sub title="Accounts" onBack={back} right={<Btn small onClick={add}>Add</Btn>} />
      <div className="scroll">
        <div className="card divided">
          {st.accounts.map((a) => (
            <div key={a.id} className="list-row">
              <div className="avatar">{initials(a.name)}</div>
              <div className="grow">
                {renaming === a.id ? (
                  <input className="input" style={{ height: 30 }} value={name} autoFocus onChange={(e) => setName(e.target.value)} onKeyDown={async (e) => { if (e.key === "Enter") { setSt(await call("renameAccount", { id: a.id, name })); setRenaming(null); } }} />
                ) : (
                  <>
                    <div style={{ fontWeight: 600 }} className="ell">{a.name}</div>
                    <div className="small faint num">{short(a.address, 6)} · {a.source === "mnemonic" ? `phrase #${a.index}` : a.source}</div>
                  </>
                )}
              </div>
              {renaming === a.id ? (
                <Btn small onClick={async () => { setSt(await call("renameAccount", { id: a.id, name })); setRenaming(null); }}>Save</Btn>
              ) : (
                <>
                  <button className="btn small ghost" onClick={() => { setRenaming(a.id); setName(a.name); }}>Rename</button>
                  {st.accounts.length > 1 && (
                    <button className="btn small ghost danger" onClick={async () => {
                      const pw = prompt(`Password to remove ${a.name}`);
                      if (pw) {
                        try { setSt(await call("removeAccount", { id: a.id, password: pw })); } catch (e) { alert((e as Error).message); }
                      }
                    }}>Remove</button>
                  )}
                </>
              )}
            </div>
          ))}
        </div>
        <div style={{ marginTop: 10 }}><Notice>Secret keys are never shown or exported by the popup; they stay in the background. Keep your original key or phrase as the backup.</Notice></div>
      </div>
    </>
  );
}

function AddAccount({ st, setSt, back, toast }: { st: WalletState; setSt: (s: WalletState) => void; back: () => void; toast: (m: string) => void }) {
  const hasPhrase = st.accounts.some((a) => a.source === "mnemonic");
  const [mode, setMode] = useState<"derive" | "secret" | "generate">(hasPhrase ? "derive" : "secret");
  const [secret, setSecret] = useState("");
  const [name, setName] = useState("");
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    setBusy(true);
    setErr(null);
    try {
      await call("addAccount", { password: pw, mode, secret, name: name || undefined });
      setSt(await call("state"));
      toast("Account added");
      back();
    } catch (e) {
      setErr((e as Error).message === "wrong password" ? "That password is not right." : (e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Sub title="Add account" onBack={back} />
      <div className="scroll">
        <Seg value={mode} onChange={setMode} options={[...(hasPhrase ? [{ value: "derive" as const, label: "From phrase" }] : []), { value: "secret", label: "Import key" }, { value: "generate", label: "New key" }]} />
        <div className="card card-body" style={{ marginTop: 10 }}>
          {mode === "secret" && (
            <Field label="Base58 secret key"><textarea className="textarea" value={secret} onChange={(e) => setSecret(e.target.value)} spellCheck={false} data-testid="add-secret" /></Field>
          )}
          {mode === "derive" && <div className="small subtle" style={{ marginBottom: 10 }}>Derives the next account from your recovery phrase.</div>}
          {mode === "generate" && <div className="small subtle" style={{ marginBottom: 10 }}>A fresh random key, not tied to the phrase. Back it up some other way: the popup never exports keys.</div>}
          <Field label="Name (optional)"><input className="input" value={name} onChange={(e) => setName(e.target.value)} /></Field>
          <Field label="Password" error={err}><input className="input" type="password" value={pw} onChange={(e) => setPw(e.target.value)} data-testid="add-pw" /></Field>
        </div>
        <div style={{ marginTop: 12 }}>
          <Btn kind="primary" block busy={busy} disabled={!pw || (mode === "secret" && !secret.trim())} onClick={submit} testid="add-submit">{!pw ? "Enter your password" : "Add account"}</Btn>
        </div>
      </div>
    </>
  );
}

function Sites({ list, back, revoke }: { list: Permission[]; back: () => void; revoke: (o: string) => void }) {
  return (
    <>
      <Sub title="Connected sites" onBack={back} />
      <div className="scroll">
        {list.length === 0 ? (
          <div className="card"><Empty title="No connected sites" body="When a dApp asks to connect, you approve it per origin. Approved sites show here and can be revoked." /></div>
        ) : (
          <div className="card divided" data-testid="sites-list">
            {list.map((p) => (
              <div key={p.origin} className="list-row">
                <div className="favicon">{Icon.globe}</div>
                <div className="grow">
                  <div style={{ fontWeight: 600 }} className="ell">{p.origin.replace(/^https?:\/\//, "")}</div>
                  <div className="small faint">{[p.solana && "Solana", p.pearl && "agent chain"].filter(Boolean).join(" + ")} · {short(p.account, 4)} · used {ago(p.lastUsed)}</div>
                </div>
                <Btn small kind="danger" onClick={() => revoke(p.origin)} testid="revoke">Revoke</Btn>
              </div>
            ))}
          </div>
        )}
        <div style={{ marginTop: 10 }}><KV k="Signing" v="every request asks again" /></div>
      </div>
    </>
  );
}
