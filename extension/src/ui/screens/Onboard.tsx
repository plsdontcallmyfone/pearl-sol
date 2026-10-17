import { useState } from "react";
import { APP_NAME, CHAIN_NAME } from "../../shared/constants";
import type { WalletState } from "../../shared/types";
import { call } from "../api";
import { Btn, Field, Icon, KV, Notice } from "../kit";

type Mode = "generate" | "mnemonic" | "secret";

export function Onboard({ onDone }: { onDone: (s: WalletState) => void }) {
  const [step, setStep] = useState<"choose" | "secret" | "phrase" | "password">("choose");
  const [mode, setMode] = useState<Mode>("secret");
  const [secret, setSecret] = useState("");
  const [phrase, setPhrase] = useState<string>("");
  const [saved, setSaved] = useState(false);
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const pick = async (m: Mode) => {
    setMode(m);
    setErr(null);
    if (m === "generate") {
      setPhrase(await call<string>("generatePhrase"));
      setStep("phrase");
    } else setStep("secret");
  };

  const finish = async () => {
    setErr(null);
    if (pw.length < 8) return setErr("Use at least 8 characters.");
    if (pw !== pw2) return setErr("The passwords do not match.");
    setBusy(true);
    try {
      const s = await call<WalletState>("create", { password: pw, mode, mnemonic: mode === "mnemonic" ? secret : undefined, secret: mode === "secret" ? secret : undefined });
      setSecret("");
      onDone(s);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (step === "choose")
    return (
      <div className="onboard">
        <div className="wordmark" style={{ fontSize: 15 }}>{APP_NAME}</div>
        <div style={{ marginTop: 40 }}>
          <h1>One key, two chains</h1>
          <p>The same ed25519 account holds SOL and the token on Solana, and the native coin on the {CHAIN_NAME.toLowerCase()}.</p>
        </div>
        <div style={{ marginTop: 26 }}>
          <button className="choice" onClick={() => pick("secret")} data-testid="import-secret">
            <div className="glyph">{Icon.lock}</div>
            <div className="grow">
              <div style={{ fontWeight: 600 }}>Import a secret key</div>
              <div className="small subtle">Base58 key from Solana CLI, Phantom or a script</div>
            </div>
            <span className="faint">{Icon.chevron}</span>
          </button>
          <button className="choice" onClick={() => pick("mnemonic")} data-testid="import-phrase">
            <div className="glyph">{Icon.receive}</div>
            <div className="grow">
              <div style={{ fontWeight: 600 }}>Import a recovery phrase</div>
              <div className="small subtle">12 or 24 words, Solana path m/44'/501'/i'/0'</div>
            </div>
            <span className="faint">{Icon.chevron}</span>
          </button>
          <button className="choice" onClick={() => pick("generate")} data-testid="create-new">
            <div className="glyph">{Icon.plus}</div>
            <div className="grow">
              <div style={{ fontWeight: 600 }}>Create a new wallet</div>
              <div className="small subtle">Generates a 12-word recovery phrase</div>
            </div>
            <span className="faint">{Icon.chevron}</span>
          </button>
        </div>
        <div className="card divided" style={{ marginTop: 18 }}>
          <KV k="Solana" v="SOL, token, dApps (Wallet Standard)" />
          <KV k={CHAIN_NAME} v="native coin, agents, jobs, mining" />
          <KV k="Bridge" v="lock on one chain, release on the other" />
        </div>
        <div className="spacer" />
        <div className="tiny faint center">Keys are encrypted with your password (PBKDF2 + AES-GCM) and never leave the extension background.</div>
      </div>
    );

  if (step === "secret")
    return (
      <div className="onboard">
        <button className="link" onClick={() => setStep("choose")} style={{ alignSelf: "flex-start" }}>Back</button>
        <h1 style={{ marginTop: 18 }}>{mode === "secret" ? "Import secret key" : "Import recovery phrase"}</h1>
        <p>{mode === "secret" ? "Paste the base58 secret key (64 bytes, or a 32-byte seed). A JSON byte array also works." : "Enter the words separated by spaces."}</p>
        <div style={{ marginTop: 18 }}>
          <Field label={mode === "secret" ? "Secret key" : "Recovery phrase"}>
            <textarea className="textarea" value={secret} onChange={(e) => setSecret(e.target.value)} spellCheck={false} autoComplete="off" data-testid="secret-input" style={{ minHeight: 104 }} />
          </Field>
        </div>
        <div className="spacer" />
        <Btn kind="primary" block disabled={!secret.trim()} onClick={() => setStep("password")} testid="secret-next">
          {secret.trim() ? "Continue" : mode === "secret" ? "Paste a key to continue" : "Enter the phrase to continue"}
        </Btn>
      </div>
    );

  if (step === "phrase")
    return (
      <div className="onboard">
        <button className="link" onClick={() => setStep("choose")} style={{ alignSelf: "flex-start" }}>Back</button>
        <h1 style={{ marginTop: 18 }}>Write down this phrase</h1>
        <p>It restores every account in this wallet. Anyone with it controls your funds.</p>
        <div className="phrase" style={{ marginTop: 16 }}>
          {phrase.split(" ").map((w, i) => (
            <div className="word" key={i}>
              <span className="num">{i + 1}</span>
              {w}
            </div>
          ))}
        </div>
        <label className="check" style={{ marginTop: 14 }}>
          <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
          I saved the phrase somewhere safe and offline.
        </label>
        <div className="spacer" />
        <Btn kind="primary" block disabled={!saved} onClick={() => setStep("password")}>
          {saved ? "Continue" : "Confirm you saved it"}
        </Btn>
      </div>
    );

  return (
    <div className="onboard">
      <button className="link" onClick={() => setStep(mode === "generate" ? "phrase" : "secret")} style={{ alignSelf: "flex-start" }}>Back</button>
      <h1 style={{ marginTop: 18 }}>Set a password</h1>
      <p>Unlocks {APP_NAME} on this browser. It does not leave this device.</p>
      <div style={{ marginTop: 18 }}>
        <Field label="Password">
          <input className="input" type="password" value={pw} onChange={(e) => setPw(e.target.value)} autoFocus data-testid="pw1" />
        </Field>
        <Field label="Confirm password" error={err}>
          <input className="input" type="password" value={pw2} onChange={(e) => setPw2(e.target.value)} onKeyDown={(e) => e.key === "Enter" && finish()} data-testid="pw2" />
        </Field>
      </div>
      <div style={{ marginTop: 12 }}>
        <Notice>The wallet locks itself after 15 minutes without use. Change it in Settings.</Notice>
      </div>
      <div className="spacer" />
      <Btn kind="primary" block busy={busy} onClick={finish} testid="create-submit">Create wallet</Btn>
    </div>
  );
}

export function Unlock({ onDone, compact }: { onDone: (s: WalletState) => void; compact?: boolean }) {
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const go = async () => {
    setBusy(true);
    setErr(null);
    try {
      onDone(await call<WalletState>("unlock", { password: pw }));
    } catch (e) {
      setErr((e as Error).message === "wrong password" ? "That password is not right." : (e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="onboard" style={compact ? { height: "auto", padding: 0 } : undefined}>
      {!compact && <div className="wordmark" style={{ fontSize: 15 }}>{APP_NAME}</div>}
      <div style={{ marginTop: compact ? 0 : 120 }}>
        <h1>{compact ? "Unlock to continue" : "Welcome back"}</h1>
        <p>Enter your password to unlock.</p>
      </div>
      <div style={{ marginTop: 18 }}>
        <Field label="Password" error={err}>
          <input className="input" type="password" value={pw} onChange={(e) => setPw(e.target.value)} onKeyDown={(e) => e.key === "Enter" && go()} autoFocus data-testid="unlock-pw" />
        </Field>
      </div>
      {!compact && <div className="spacer" />}
      <div style={{ marginTop: 14 }}>
        <Btn kind="primary" block busy={busy} disabled={!pw} onClick={go} testid="unlock-submit">Unlock</Btn>
      </div>
    </div>
  );
}
