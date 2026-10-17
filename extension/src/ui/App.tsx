import { useEffect, useState } from "react";
import { APP_NAME, short } from "../shared/constants";
import type { WalletState } from "../shared/types";
import { applyTheme, call } from "./api";
import { Icon, initials } from "./kit";
import { Onboard, Unlock } from "./screens/Onboard";
import { Home, HomeView, Receive, Send } from "./screens/Home";
import { Bridge } from "./screens/Bridge";
import { Jobs } from "./screens/Jobs";
import { Activity } from "./screens/Activity";
import { Settings } from "./screens/Settings";

type Tab = "wallet" | "bridge" | "jobs" | "activity" | "settings";
const TABS: { id: Tab; label: string; icon: JSX.Element }[] = [
  { id: "wallet", label: "Wallet", icon: Icon.wallet },
  { id: "bridge", label: "Bridge", icon: Icon.bridge },
  { id: "jobs", label: "Jobs", icon: Icon.jobs },
  { id: "activity", label: "Activity", icon: Icon.activity },
  { id: "settings", label: "Settings", icon: Icon.settings },
];

function initialTab(): Tab {
  const h = location.hash.replace("#", "") as Tab;
  if (TABS.some((t) => t.id === h)) return h;
  try {
    const t = localStorage.getItem("tab") as Tab | null;
    if (t && TABS.some((x) => x.id === t)) return t;
  } catch {
    /* storage blocked */
  }
  return "wallet";
}

export function App() {
  const [st, setSt] = useState<WalletState | null>(null);
  const [tab, setTabRaw] = useState<Tab>(initialTab);
  const [homeView, setHomeView] = useState<HomeView>("home");
  const [menu, setMenu] = useState(false);
  const [toastMsg, setToast] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const setTab = (t: Tab) => {
    setTabRaw(t);
    setHomeView("home");
    try {
      localStorage.setItem("tab", t);
    } catch {
      /* storage blocked */
    }
  };
  const toast = (m: string) => {
    setToast(m);
    setTimeout(() => setToast(null), 2600);
  };

  useEffect(() => {
    call<WalletState>("state").then((s) => {
      setSt(s);
      applyTheme(s.settings.theme);
    }, (e) => setErr(e.message));
  }, []);

  if (err) return <div className="onboard"><h1>Background unavailable</h1><p>{err}</p></div>;
  if (!st) return <div className="app" />;
  if (!st.initialized) return <div className="app"><Onboard onDone={setSt} /></div>;
  if (st.locked) return <div className="app"><Unlock onDone={setSt} /></div>;

  const acct = st.selected!;
  return (
    <div className="app with-tabs">
      <div className="topbar">
        <button className="acct-btn" onClick={() => setMenu(!menu)} aria-expanded={menu} aria-label="Switch account" data-testid="acct-btn">
          <div className="avatar">{initials(acct.name)}</div>
          <div style={{ minWidth: 0 }}>
            <div className="acct-name">{acct.name}</div>
            <div className="acct-addr num">{short(acct.address, 4)}</div>
          </div>
          <span className="faint" style={{ width: 14, display: "grid" }}>{Icon.down}</span>
        </button>
        <div className="spacer" />
        <button className="pill" onClick={() => setTab("settings")} title={`${st.network.solanaRpc} | ${st.network.chainRpc}`} data-testid="net-pill">
          <span className="dot ok" />
          {st.network.name}
        </button>
        <button className="icon-btn" aria-label="Lock" onClick={async () => setSt(await call("lock"))} data-testid="lock">{Icon.lock}</button>
      </div>

      {menu && (
        <>
          <div className="scrim" onClick={() => setMenu(false)} />
          <div className="card menu divided" role="menu">
            {st.accounts.map((a) => (
              <div key={a.id} className="list-row click" role="menuitem" tabIndex={0} onClick={async () => { setSt(await call("selectAccount", { id: a.id })); setMenu(false); }}>
                <div className="avatar">{initials(a.name)}</div>
                <div className="grow">
                  <div style={{ fontWeight: 600 }} className="ell">{a.name}</div>
                  <div className="small faint num">{short(a.address, 6)}</div>
                </div>
                {a.id === acct.id && <span style={{ color: "var(--accent-text)", width: 16, display: "grid" }}>{Icon.check}</span>}
              </div>
            ))}
            <div className="list-row click" role="menuitem" tabIndex={0} onClick={() => { setMenu(false); setTab("settings"); }}>
              <div className="glyph">{Icon.plus}</div>
              <div className="grow" style={{ fontWeight: 600 }}>Manage accounts</div>
            </div>
          </div>
        </>
      )}

      {tab === "wallet" && homeView === "home" && <Home st={st} go={setHomeView} toBridge={() => setTab("bridge")} toTab={(t) => setTab(t as Tab)} />}
      {tab === "wallet" && homeView === "send" && <Send st={st} back={() => setHomeView("home")} done={(m) => { toast(m); setHomeView("home"); }} />}
      {tab === "wallet" && homeView === "receive" && <Receive st={st} back={() => setHomeView("home")} />}
      {tab === "bridge" && <Bridge st={st} toast={toast} />}
      {tab === "jobs" && <Jobs st={st} toast={toast} />}
      {tab === "activity" && <Activity st={st} />}
      {tab === "settings" && <Settings st={st} setSt={(s) => { setSt(s); applyTheme(s.settings.theme); }} toast={toast} />}

      <nav className="tabbar">
        {TABS.map((t) => (
          <button key={t.id} className={`tab ${tab === t.id ? "active" : ""}`} onClick={() => setTab(t.id)} aria-current={tab === t.id ? "page" : undefined} data-testid={`tab-${t.id}`}>
            {t.icon}
            {t.label}
          </button>
        ))}
      </nav>
      {toastMsg && <div className="toast" role="status">{toastMsg}</div>}
      <span hidden>{APP_NAME}</span>
    </div>
  );
}
