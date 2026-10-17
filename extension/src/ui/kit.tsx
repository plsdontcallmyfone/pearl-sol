// Small UI kit: icons (inline SVG, stroke style), buttons, fields, rows.
import { ReactNode, useEffect, useRef, useState } from "react";

const svg = (d: ReactNode) => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {d}
  </svg>
);

export const Icon = {
  send: svg(<><path d="M5 12h14" /><path d="m13 6 6 6-6 6" /></>),
  receive: svg(<><path d="M12 5v14" /><path d="m6 13 6 6 6-6" /></>),
  bridge: svg(<><path d="M4 8h13" /><path d="m14 4 4 4-4 4" /><path d="M20 16H7" /><path d="m10 12-4 4 4 4" /></>),
  wallet: svg(<><rect x="3" y="6" width="18" height="13" rx="3" /><path d="M16 12.5h2" /><path d="M3 9h18" /></>),
  jobs: svg(<><rect x="4" y="4" width="7" height="7" rx="1.5" /><rect x="13" y="4" width="7" height="7" rx="1.5" /><rect x="4" y="13" width="7" height="7" rx="1.5" /><rect x="13" y="13" width="7" height="7" rx="1.5" /></>),
  activity: svg(<><path d="M3 12h4l3-7 4 14 3-7h4" /></>),
  settings: svg(<><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3 1.6 1.6 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.6 1.6 0 0 0 1.5-1 1.6 1.6 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3h0a1.6 1.6 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8v0a1.6 1.6 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1Z" /></>),
  lock: svg(<><rect x="5" y="11" width="14" height="9" rx="2" /><path d="M8 11V8a4 4 0 0 1 8 0v3" /></>),
  back: svg(<path d="m15 6-6 6 6 6" />),
  copy: svg(<><rect x="9" y="9" width="11" height="11" rx="2" /><path d="M5 15V6a2 2 0 0 1 2-2h9" /></>),
  check: svg(<path d="m5 12 5 5 9-10" />),
  chevron: svg(<path d="m9 6 6 6-6 6" />),
  down: svg(<path d="m6 9 6 6 6-6" />),
  plus: svg(<><path d="M12 5v14" /><path d="M5 12h14" /></>),
  info: svg(<><circle cx="12" cy="12" r="9" /><path d="M12 11v5" /><path d="M12 8h0" /></>),
  alert: svg(<><path d="M12 4 2.5 20h19L12 4Z" /><path d="M12 10v4" /><path d="M12 17h0" /></>),
  cube: svg(<><path d="m12 3 8 4.5v9L12 21l-8-4.5v-9L12 3Z" /><path d="m4 7.5 8 4.5 8-4.5" /><path d="M12 12v9" /></>),
  agent: svg(<><circle cx="12" cy="8" r="4" /><path d="M4 21a8 8 0 0 1 16 0" /></>),
  globe: svg(<><circle cx="12" cy="12" r="9" /><path d="M3 12h18" /><path d="M12 3a14 14 0 0 1 0 18" /><path d="M12 3a14 14 0 0 0 0 18" /></>),
  refresh: svg(<><path d="M20 11a8 8 0 0 0-14.9-3" /><path d="M4 4v4h4" /><path d="M4 13a8 8 0 0 0 14.9 3" /><path d="M20 20v-4h-4" /></>),
  cart: svg(<><path d="M3 4h2l2.4 11h10.2L20 8H6.2" /><circle cx="9" cy="19" r="1.4" /><circle cx="17" cy="19" r="1.4" /></>),
  pick: svg(<><path d="M14 4c3 0 6 2 6 2s-4 0-7 3-3 7-3 7l-5 5-2-2 5-5s4 0 7-3 3-7 3-7" /></>),
};

export function Btn(p: { children: ReactNode; onClick?: () => void; kind?: "primary" | "danger" | "ghost"; small?: boolean; block?: boolean; disabled?: boolean; busy?: boolean; type?: "button" | "submit"; title?: string; testid?: string }) {
  return (
    <button
      type={p.type ?? "button"}
      className={`btn ${p.kind ?? ""} ${p.small ? "small" : ""} ${p.block ? "block" : ""}`}
      onClick={p.onClick}
      disabled={p.disabled || p.busy}
      title={p.title}
      data-testid={p.testid}
    >
      {p.busy ? "Working..." : p.children}
    </button>
  );
}

export function Field(p: { label: string; aside?: ReactNode; hint?: ReactNode; error?: string | null; children: ReactNode }) {
  return (
    <label className="field">
      <div className="label">
        <span>{p.label}</span>
        {p.aside}
      </div>
      {p.children}
      {p.hint && <div className="hint">{p.hint}</div>}
      {p.error && <div className="error-text">{p.error}</div>}
    </label>
  );
}

export function Notice(p: { kind?: "warn" | "bad" | "ok" | "accent"; children: ReactNode }) {
  return (
    <div className={`notice ${p.kind ?? ""}`} role={p.kind === "bad" ? "alert" : undefined}>
      {p.kind === "warn" || p.kind === "bad" ? Icon.alert : Icon.info}
      <div>{p.children}</div>
    </div>
  );
}

export function KV(p: { k: ReactNode; v: ReactNode; title?: string }) {
  return (
    <div className="kv" title={p.title}>
      <span>{p.k}</span>
      <span className="num">{p.v}</span>
    </div>
  );
}

export function Sub(p: { title: string; onBack: () => void; right?: ReactNode }) {
  return (
    <div className="subhead">
      <button className="icon-btn" onClick={p.onBack} aria-label="Back">
        {Icon.back}
      </button>
      <div className="title grow">{p.title}</div>
      {p.right}
    </div>
  );
}

export function Seg<T extends string>(p: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="seg" role="tablist">
      {p.options.map((o) => (
        <button key={o.value} className={o.value === p.value ? "on" : ""} onClick={() => p.onChange(o.value)} role="tab" aria-selected={o.value === p.value}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function CopyBtn({ text, label }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className={label ? "btn small" : "icon-btn"}
      aria-label={`Copy ${label ?? ""}`.trim()}
      onClick={async (e) => {
        e.preventDefault();
        e.stopPropagation();
        try {
          await navigator.clipboard.writeText(text);
        } catch {
          /* clipboard blocked */
        }
        setDone(true);
        setTimeout(() => setDone(false), 1200);
      }}
    >
      {done ? Icon.check : Icon.copy}
      {label && <span>{done ? "Copied" : label}</span>}
    </button>
  );
}

/** Poll while mounted, with an in-flight guard. */
export function usePoll<T>(fn: () => Promise<T>, ms: number, deps: unknown[] = []) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let alive = true;
    const run = async () => {
      if (busy.current) return;
      busy.current = true;
      try {
        const v = await fn();
        if (alive) {
          setData(v);
          setError(null);
        }
      } catch (e) {
        if (alive) setError((e as Error).message);
      } finally {
        busy.current = false;
      }
    };
    void run();
    const t = ms > 0 ? setInterval(run, ms) : undefined;
    return () => {
      alive = false;
      if (t) clearInterval(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { data, error, reload: () => setTick((x) => x + 1), setData };
}

export function Empty(p: { title: string; body: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      <div className="title">{p.title}</div>
      <p>{p.body}</p>
      {p.action}
    </div>
  );
}

export const initials = (name: string) => name.trim().split(/\s+/).map((w) => w[0]).join("").slice(0, 2).toUpperCase() || "A";

export function ago(ts: number) {
  if (!ts) return "";
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return new Date(ts).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
