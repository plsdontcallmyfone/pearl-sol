import { useState } from "react";
import type { Job } from "../../chain/rpc";
import { LOCALNET_CONFIG_HEX } from "../../chain/job";
import { CHAIN_NAME, NATIVE_DECIMALS, SOL_DECIMALS, SPL_DECIMALS, TOKEN_SYMBOL, formatUnits, parseUnits, short } from "../../shared/constants";
import type { WalletState } from "../../shared/types";
import { call } from "../api";
import { Btn, CopyBtn, Empty, Field, Icon, KV, Notice, Seg, Sub, usePoll } from "../kit";
import { useBalances } from "./Home";

type View = "market" | "post" | "store" | "rewards";
const nat = (v: string) => `${formatUnits(v, NATIVE_DECIMALS, 4)} ${TOKEN_SYMBOL}`;
const natFee = (v: string) => `${formatUnits(v, NATIVE_DECIMALS, 9)} ${TOKEN_SYMBOL}`;
const STATUS_BADGE: Record<string, string> = { open: "accent", delivered: "warn", completed: "ok", expired: "" };

export function Jobs({ st, toast }: { st: WalletState; toast: (m: string) => void }) {
  const [view, setView] = useState<View>("market");
  const [open, setOpen] = useState<string | null>(null);
  if (open) return <JobDetail id={open} back={() => setOpen(null)} me={st.selected!.address} />;
  return (
    <div className="scroll">
      <Seg<View> value={view} onChange={setView} options={[{ value: "market", label: "Market" }, { value: "post", label: "Post" }, { value: "store", label: "Buy" }, { value: "rewards", label: "Mining" }]} />
      <div style={{ marginTop: 10 }}>
        {view === "market" && <Market st={st} onOpen={setOpen} toast={toast} />}
        {view === "post" && <PostJob st={st} toast={toast} onPosted={(id) => setOpen(id)} />}
        {view === "store" && <Storefront st={st} toast={toast} />}
        {view === "rewards" && <Rewards st={st} />}
      </div>
    </div>
  );
}

function AgentCard({ st, toast }: { st: WalletState; toast: (m: string) => void }) {
  const bal = useBalances(st.selected?.address, st.network.id);
  const [name, setName] = useState("");
  const [uri, setUri] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const agent = bal.data?.agent;
  const offline = !!bal.data?.errors.chain;
  if (agent && !editing)
    return (
      <div className="card divided" data-testid="agent-card">
        <div className="list-row">
          <div className="glyph">{Icon.agent}</div>
          <div className="grow">
            <div style={{ fontWeight: 600 }} className="ell">{agent.name}</div>
            <div className="small faint ell">{agent.uri || "no profile URI"}</div>
          </div>
          <span className="badge ok">Registered</span>
        </div>
        <div className="between" style={{ padding: "8px 12px" }}>
          <span className="small subtle num">since height {agent.registered_height}</span>
          <button className="link small" onClick={() => { setName(agent.name); setUri(agent.uri); setEditing(true); }}>Update profile</button>
        </div>
      </div>
    );
  if (!agent && !editing)
    return (
      <div className="card" data-testid="agent-card">
        <div className="list-row">
          <div className="glyph">{Icon.agent}</div>
          <div className="grow">
            <div style={{ fontWeight: 600 }}>Not registered as an agent</div>
            <div className="small faint">Name + profile URI on the {CHAIN_NAME.toLowerCase()}</div>
          </div>
          <Btn small kind="primary" disabled={offline} onClick={() => setEditing(true)} testid="agent-open">{offline ? "Offline" : "Register"}</Btn>
        </div>
      </div>
    );
  const submit = async () => {
    setBusy(true);
    setErr(null);
    try {
      await call("registerAgent", { name, uri });
      toast("RegisterAgent sent");
      setEditing(false);
      setTimeout(bal.reload, 1500);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="card card-body" data-testid="agent-card">
      <div className="between">
        <div className="title" style={{ fontSize: 13.5 }}>{agent ? "Update agent profile" : "Register as an agent"}</div>
        <span className="badge">RegisterAgent</span>
      </div>
      <div className="small subtle" style={{ marginTop: 3 }}>A name and profile URI keyed by your account on the {CHAIN_NAME.toLowerCase()}.</div>
      <div style={{ marginTop: 10 }}>
        <div className="grid2">
          <Field label="Name"><input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="my-agent" maxLength={64} data-testid="agent-name" /></Field>
          <Field label="Profile URI"><input className="input" value={uri} onChange={(e) => setUri(e.target.value)} placeholder="https://..." /></Field>
        </div>
      </div>
      {err && <div className="error-text">{err}</div>}
      <div style={{ marginTop: 10 }} className="btn-row">
        <Btn onClick={() => setEditing(false)}>Cancel</Btn>
        <Btn kind="primary" block disabled={!name.trim() || offline} busy={busy} onClick={submit} testid="agent-submit">
          {offline ? "Agent chain offline" : !name.trim() ? "Enter a name" : agent ? "Update" : "Register"}
        </Btn>
      </div>
    </div>
  );
}

function JobRow({ j, onOpen, me }: { j: Job; onOpen: (id: string) => void; me: string }) {
  return (
    <div className="list-row click" onClick={() => onOpen(j.job_id)} role="button" tabIndex={0} onKeyDown={(e) => e.key === "Enter" && onOpen(j.job_id)} data-testid="job-row">
      <div className="glyph">{Icon.cube}</div>
      <div className="grow">
        <div className="row" style={{ gap: 6 }}>
          <span style={{ fontWeight: 600 }} className="num">{short(j.job_id, 5)}</span>
          {j.poster === me && <span className="badge">yours</span>}
        </div>
        <div className="small faint num ell">{j.m} x {j.n} · {j.blocks_mined}/{j.max_blocks} blocks · ends {j.deadline_height.toLocaleString("en-US")}</div>
      </div>
      <div className="right">
        <div className="num" style={{ fontWeight: 600, marginBottom: 3 }}>{formatUnits(j.worker_pay, NATIVE_DECIMALS, 2)}<span className="unit">{TOKEN_SYMBOL}</span></div>
        <span className={`badge ${STATUS_BADGE[j.status] ?? ""}`}>{j.status}</span>
      </div>
    </div>
  );
}

function Market({ st, onOpen, toast }: { st: WalletState; onOpen: (id: string) => void; toast: (m: string) => void }) {
  const [status, setStatus] = useState("");
  const [page, setPage] = useState(0);
  const limit = 20;
  const jobs = usePoll(() => call<{ total: number; jobs: Job[] }>("listJobs", { status, offset: page * limit, limit }), 10000, [status, page, st.network.id]);
  const total = jobs.data?.total ?? 0;
  return (
    <>
      <AgentCard st={st} toast={toast} />
      <div className="section-title">
        <span className="eyebrow">Jobs {jobs.data ? <span className="num">({total})</span> : null}</span>
        <button className="icon-btn" onClick={jobs.reload} aria-label="Refresh jobs">{Icon.refresh}</button>
      </div>
      <div className="chips">
        {["", "open", "mineable", "delivered", "completed", "expired"].map((s) => (
          <button key={s || "all"} className={`chip ${status === s ? "on" : ""}`} onClick={() => { setStatus(s); setPage(0); }}>{s || "all"}</button>
        ))}
      </div>
      <div style={{ marginTop: 8 }}>
        {jobs.error && !jobs.data ? (
          <div className="card"><Empty title="Agent chain unreachable" body={`${jobs.error}. Start the node on ${st.network.chainRpc} or change it in Settings.`} action={<Btn small onClick={jobs.reload}>Retry</Btn>} /></div>
        ) : !jobs.data ? (
          <div className="card divided">{[0, 1, 2].map((i) => <div key={i} className="list-row"><div className="glyph" /><div className="grow"><span className="skel" style={{ width: 120 }} /></div></div>)}</div>
        ) : jobs.data.jobs.length === 0 ? (
          <div className="card"><Empty title={status ? `No ${status} jobs` : "No jobs posted yet"} body="Post one from the Post tab, or buy one through the Solana storefront." /></div>
        ) : (
          <div className="card divided">{jobs.data.jobs.map((j) => <JobRow key={j.job_id} j={j} onOpen={onOpen} me={st.selected!.address} />)}</div>
        )}
      </div>
      {total > limit && (
        <div className="between" style={{ marginTop: 8 }}>
          <Btn small disabled={page === 0} onClick={() => setPage(page - 1)}>Newer</Btn>
          <span className="small faint num">{page * limit + 1} to {Math.min(total, (page + 1) * limit)} of {total}</span>
          <Btn small disabled={(page + 1) * limit >= total} onClick={() => setPage(page + 1)}>Older</Btn>
        </div>
      )}
    </>
  );
}

function JobDetail({ id, back, me }: { id: string; back: () => void; me: string }) {
  const job = usePoll(() => call<Job | null>("getJob", { jobId: id }), 6000, [id]);
  const j = job.data;
  const steps = ["open", "delivered", "completed"];
  const idx = j ? (j.status === "expired" ? -1 : steps.indexOf(j.status)) : 0;
  return (
    <>
      <Sub title="Job" onBack={back} right={<CopyBtn text={id} />} />
      <div className="scroll">
        {job.error && !j && <Notice kind="bad">{job.error}</Notice>}
        {!job.error && j === null && job.data === null && <div className="card"><Empty title="Loading" body="Fetching the job from the node." /></div>}
        {j && (
          <>
            <div className="card card-body">
              <div className="between">
                <span className="eyebrow">Worker pay</span>
                <span className={`badge ${STATUS_BADGE[j.status] ?? ""}`}>{j.status}{j.mineable ? ", mineable" : ""}</span>
              </div>
              <div className="hero-amount num" style={{ marginTop: 6, fontSize: 22 }}>{formatUnits(j.worker_pay, NATIVE_DECIMALS, 4)}<span className="hero-unit">{TOKEN_SYMBOL}</span></div>
              <div className="steps" style={{ gridTemplateColumns: "repeat(3, 1fr)", marginTop: 12 }}>
                {steps.map((s, i) => <div key={s} className={`step ${idx >= i ? "done" : ""}`} />)}
              </div>
              <div className="between tiny faint" style={{ marginTop: 5 }}><span>Open</span><span>Delivered</span><span>Completed</span></div>
              {j.status === "expired" && <div style={{ marginTop: 8 }}><Notice kind="warn">Expired at height {j.closed_height || j.deadline_height}; W went back to the poster.</Notice></div>}
            </div>
            <div className="section-title"><span className="eyebrow">Terms</span></div>
            <div className="card divided">
              <KV k="Job id" v={short(j.job_id, 8)} title={j.job_id} />
              <KV k="Poster" v={j.poster === me ? "you" : short(j.poster, 6)} title={j.poster} />
              <KV k="Shape (m x n x k)" v={`${j.m} x ${j.n} x ${j.k}`} />
              <KV k="Protocol fee" v={nat(j.protocol_fee)} />
              <KV k="Deadline height" v={j.deadline_height.toLocaleString("en-US")} />
              <KV k="Posted at height" v={j.posted_height.toLocaleString("en-US")} />
              <KV k="Evaluators" v={`${j.threshold} of ${j.evaluators.length}`} />
              <KV k="Data" v={j.data_uri || "none"} />
            </div>
            <div className="section-title"><span className="eyebrow">Mining</span></div>
            <div className="card divided">
              <KV k="Blocks mined" v={`${j.blocks_mined} of ${j.max_blocks}`} />
              <KV k="Subsidy paid" v={nat(j.subsidy_paid)} />
              <KV k="Cap remaining" v={nat(j.cap_remaining)} />
              <KV k="job_key" v={short(j.job_key, 8)} title={j.job_key} />
            </div>
            <div className="section-title"><span className="eyebrow">Result</span></div>
            <div className="card divided">
              <KV k="Deliverer" v={j.deliverer ? short(j.deliverer, 6) : "none yet"} title={j.deliverer ?? ""} />
              <KV k="Result hash" v={j.result_hash ? short(j.result_hash, 8) : "none yet"} title={j.result_hash ?? ""} />
              {j.result_uri && <KV k="Result URI" v={j.result_uri} />}
            </div>
          </>
        )}
      </div>
    </>
  );
}

// ---------------------------------------------------------------- post

async function fileB64(f: File): Promise<string> {
  const buf = new Uint8Array(await f.arrayBuffer());
  let s = "";
  for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode(...buf.subarray(i, i + 0x8000));
  return btoa(s);
}

function PostJob({ st, toast, onPosted }: { st: WalletState; toast: (m: string) => void; onPosted: (id: string) => void }) {
  const bal = useBalances(st.selected?.address, st.network.id);
  const [m, setM] = useState("8");
  const [n, setN] = useState("16");
  const [config, setConfig] = useState(LOCALNET_CONFIG_HEX);
  const [aFile, setA] = useState<File | null>(null);
  const [bFile, setB] = useState<File | null>(null);
  const [dataUri, setDataUri] = useState("");
  const [w, setW] = useState("");
  const [p, setP] = useState("");
  const [deadline, setDeadline] = useState("");
  const [maxBlocks, setMaxBlocks] = useState("10");
  const [evals, setEvals] = useState("");
  const [threshold, setThreshold] = useState("1");
  const [preview, setPreview] = useState<{ jobId: string; jobKey: string; hashA: string; hashB: string; nonce: string; fee: string } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const height = bal.data?.chain?.height;

  const build = async () => {
    if (!aFile || !bFile) throw new Error("choose the A and B matrix files");
    const evaluators = evals.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
    if (!evaluators.length) throw new Error("add at least one evaluator address (the attester in M1)");
    return {
      m: Number(m), n: Number(n), config: config.trim(), dataUri: dataUri.trim(),
      workerPay: parseUnits(w, NATIVE_DECIMALS).toString(), protocolFee: parseUnits(p || "0", NATIVE_DECIMALS).toString(),
      deadlineHeight: Number(deadline), maxBlocks: Number(maxBlocks), evaluators, threshold: Number(threshold),
      a: await fileB64(aFile), b: await fileB64(bFile),
    };
  };
  const doPreview = async () => {
    setErr(null);
    setBusy(true);
    try {
      setPreview(await call("preparePostJob", { input: await build() }));
    } catch (e) {
      setPreview(null);
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const submit = async () => {
    setErr(null);
    setBusy(true);
    try {
      const r = await call<{ jobId: string }>("postJob", { input: await build() });
      toast("PostJob sent");
      setTimeout(() => onPosted(r.jobId), 800);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const missing = !aFile || !bFile ? "Choose A and B" : !w ? "Enter worker pay" : !deadline ? "Enter a deadline" : !evals.trim() ? "Add an evaluator" : null;

  return (
    <>
      <div className="card card-body">
        <div className="title" style={{ fontSize: 13.5 }}>Matrices</div>
        <div className="small subtle" style={{ marginTop: 2 }}>int8 row-major files as written by <span style={{ fontWeight: 600 }}>pearl-job gen</span>: A is m x k, B is k x n. Commitments are computed here and match pearl-job byte for byte.</div>
        <div className="grid2" style={{ marginTop: 10 }}>
          <Field label="m (rows of A)"><input className="input" value={m} onChange={(e) => setM(e.target.value)} inputMode="numeric" /></Field>
          <Field label="n (cols of B)"><input className="input" value={n} onChange={(e) => setN(e.target.value)} inputMode="numeric" /></Field>
        </div>
        <div className="grid2">
          <Field label="A.i8" hint={aFile ? `${aFile.size.toLocaleString("en-US")} bytes` : "m x k bytes"}><input className="input" type="file" onChange={(e) => setA(e.target.files?.[0] ?? null)} style={{ paddingTop: 8 }} data-testid="file-a" /></Field>
          <Field label="B.i8" hint={bFile ? `${bFile.size.toLocaleString("en-US")} bytes` : "k x n bytes"}><input className="input" type="file" onChange={(e) => setB(e.target.files?.[0] ?? null)} style={{ paddingTop: 8 }} data-testid="file-b" /></Field>
        </div>
        <Field label="Mining config (52 bytes hex)" hint="Default: localnet k 1024, rank 32, 4 x 8 tile (m multiple of 4, n of 8).">
          <input className="input" value={config} onChange={(e) => setConfig(e.target.value)} spellCheck={false} />
        </Field>
        <Field label="Data URI"><input className="input" value={dataUri} onChange={(e) => setDataUri(e.target.value)} placeholder="file:///.../manifest.json or ipfs://..." spellCheck={false} /></Field>
      </div>
      <div className="card card-body">
        <div className="title" style={{ fontSize: 13.5 }}>Terms</div>
        <div className="grid2" style={{ marginTop: 10 }}>
          <Field label={`Worker pay W`} hint="Escrowed, paid on completion"><input className="input" value={w} onChange={(e) => setW(e.target.value)} placeholder="0.0" inputMode="decimal" data-testid="job-w" /></Field>
          <Field label={`Protocol fee P`} hint="To the mining pool, kept"><input className="input" value={p} onChange={(e) => setP(e.target.value)} placeholder="0.0" inputMode="decimal" /></Field>
        </div>
        <div className="grid2">
          <Field label="Deadline height" hint={height !== undefined ? `tip is ${height.toLocaleString("en-US")}` : "absolute block height"}>
            <input className="input" value={deadline} onChange={(e) => setDeadline(e.target.value)} inputMode="numeric" placeholder={height !== undefined ? String(height + 500) : ""} data-testid="job-deadline" />
          </Field>
          <Field label="Max blocks"><input className="input" value={maxBlocks} onChange={(e) => setMaxBlocks(e.target.value)} inputMode="numeric" /></Field>
        </div>
        <Field label="Evaluators" hint="Base58 addresses, comma separated. M1: the attester key.">
          <input className="input" value={evals} onChange={(e) => setEvals(e.target.value)} spellCheck={false} data-testid="job-evals" />
        </Field>
        <Field label="Threshold"><input className="input" value={threshold} onChange={(e) => setThreshold(e.target.value)} inputMode="numeric" /></Field>
      </div>
      {preview && (
        <div className="card divided" data-testid="job-preview">
          <KV k="Job id" v={short(preview.jobId, 8)} title={preview.jobId} />
          <KV k="job_key" v={short(preview.jobKey, 8)} title={preview.jobKey} />
          <KV k="hash_a" v={short(preview.hashA, 8)} title={preview.hashA} />
          <KV k="hash_b" v={short(preview.hashB, 8)} title={preview.hashB} />
          <KV k="Tx nonce / fee" v={`${preview.nonce} / ${formatUnits(preview.fee, NATIVE_DECIMALS, 9)}`} />
        </div>
      )}
      {err && <div style={{ marginTop: 10 }}><Notice kind="bad">{err}</Notice></div>}
      <div className="btn-row" style={{ marginTop: 12 }}>
        <Btn disabled={!!missing} busy={busy} onClick={doPreview}>Preview</Btn>
        <Btn kind="primary" disabled={!!missing} busy={busy} onClick={submit} testid="job-submit">{missing ?? "Post job"}</Btn>
      </div>
    </>
  );
}

// ---------------------------------------------------------------- storefront

interface Quote {
  venue: string;
  quoteSpl: string;
  minOutSpl: string;
  workerPaySpl: string;
  protocolFeeSpl: string;
  protocolFeeBps: number;
  maxSlippageBps: number;
  minSolIn: string;
  maxSolIn: string;
  paused: boolean;
  maxDim: number;
  maxBlocksCap: number;
  maxDeadlineBlocks: string;
}

function Storefront({ st, toast }: { st: WalletState; toast: (m: string) => void }) {
  const [sol, setSol] = useState("");
  const [m, setM] = useState("64");
  const [n, setN] = useState("64");
  const [config, setConfig] = useState(LOCALNET_CONFIG_HEX);
  const [hashA, setHA] = useState("");
  const [hashB, setHB] = useState("");
  const [uri, setUri] = useState("");
  const [deadline, setDeadline] = useState("500");
  const [maxBlocks, setMaxBlocks] = useState("10");
  const [evals, setEvals] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [placed, setPlaced] = useState<{ nonce: number; order: string; jobId: string; venue: string } | null>(null);
  let lamports: bigint | null = null;
  try {
    lamports = sol ? parseUnits(sol, SOL_DECIMALS) : null;
  } catch {
    lamports = null;
  }
  const quote = usePoll(() => (lamports && lamports > 0n ? call<Quote>("storefrontQuote", { solIn: lamports.toString() }) : Promise.resolve(null)), 0, [sol, st.network.id]);
  const q = quote.data;
  const hexOk = (h: string) => /^[0-9a-fA-F]{64}$/.test(h.trim());
  const missing = !lamports ? "Enter SOL to spend" : !hexOk(hashA) || !hexOk(hashB) ? "Enter hash_a and hash_b" : !evals.trim() ? "Add an evaluator" : q?.paused ? "Storefront is paused" : null;
  const buy = async () => {
    setBusy(true);
    setErr(null);
    try {
      const r = await call("storefrontBuy", {
        input: {
          solIn: lamports!.toString(),
          spec: { m: Number(m), n: Number(n), k: 1024, config: config.trim(), hashA: hashA.trim(), hashB: hashB.trim(), dataUri: uri.trim(), deadlineBlocks: deadline, maxBlocks: Number(maxBlocks), evaluators: evals.split(/[\s,]+/).filter(Boolean), threshold: 1 },
        },
      });
      setPlaced(r);
      toast("Order placed on Solana");
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Notice kind="accent">
        Pay SOL on Solana; the storefront buys {TOKEN_SYMBOL}, splits it into W and P, and bridges it so the job is posted on the {CHAIN_NAME.toLowerCase()} under your address. Two transactions: open_order, then buy_job.
      </Notice>
      <div className="card card-body" style={{ marginTop: 10 }}>
        <Field label="Spend" aside={<span className="small faint">SOL</span>}>
          <input className="input" value={sol} onChange={(e) => setSol(e.target.value)} placeholder="0.5" inputMode="decimal" data-testid="store-sol" />
        </Field>
        <div className="grid2">
          <Field label="m"><input className="input" value={m} onChange={(e) => setM(e.target.value)} /></Field>
          <Field label="n"><input className="input" value={n} onChange={(e) => setN(e.target.value)} /></Field>
        </div>
        <Field label="hash_a (hex)"><input className="input" value={hashA} onChange={(e) => setHA(e.target.value)} spellCheck={false} /></Field>
        <Field label="hash_b (hex)"><input className="input" value={hashB} onChange={(e) => setHB(e.target.value)} spellCheck={false} /></Field>
        <Field label="Mining config (hex)"><input className="input" value={config} onChange={(e) => setConfig(e.target.value)} spellCheck={false} /></Field>
        <Field label="Data URI"><input className="input" value={uri} onChange={(e) => setUri(e.target.value)} spellCheck={false} /></Field>
        <div className="grid2">
          <Field label="Deadline (blocks after unlock)"><input className="input" value={deadline} onChange={(e) => setDeadline(e.target.value)} /></Field>
          <Field label="Max blocks"><input className="input" value={maxBlocks} onChange={(e) => setMaxBlocks(e.target.value)} /></Field>
        </div>
        <Field label="Evaluators" hint="Base58, comma separated; threshold 1."><input className="input" value={evals} onChange={(e) => setEvals(e.target.value)} spellCheck={false} /></Field>
      </div>
      <div className="card divided" style={{ marginTop: 10 }}>
        {quote.error ? <div style={{ padding: 12 }}><Notice kind="warn">{quote.error}</Notice></div> : (
          <>
            <KV k="Venue" v={q ? (q.venue === "curve" ? "pump curve" : "PumpSwap") : "-"} />
            <KV k="Tokens (fee-free quote)" v={q ? `${formatUnits(q.quoteSpl, SPL_DECIMALS, 2)} ${TOKEN_SYMBOL}` : "-"} />
            <KV k={`Worker pay W (${q ? 100 - q.protocolFeeBps / 100 : "-"}%)`} v={q ? `${formatUnits(q.workerPaySpl, SPL_DECIMALS, 2)}` : "-"} />
            <KV k="Protocol fee P" v={q ? `${formatUnits(q.protocolFeeSpl, SPL_DECIMALS, 2)}` : "-"} />
            <KV k="Minimum out" v={q ? `${formatUnits(q.minOutSpl, SPL_DECIMALS, 2)} (${q.maxSlippageBps / 100}% max slippage)` : "-"} />
          </>
        )}
      </div>
      <div style={{ marginTop: 10 }}>
        <Notice kind="warn">An order's job_key depends on W, P and the absolute deadline, which are only fixed after the swap and the unlock, so hash_a / hash_b cannot be precomputed for it here. Enter commitments your miner pipeline expects.</Notice>
      </div>
      {err && <div style={{ marginTop: 10 }}><Notice kind="bad">{err}</Notice></div>}
      {placed && (
        <div className="card divided" style={{ marginTop: 10 }} data-testid="order-placed">
          <KV k="Order" v={short(placed.order, 6)} title={placed.order} />
          <KV k="Bridge nonce" v={placed.nonce} />
          <KV k="Agent-chain job id" v={short(placed.jobId, 8)} title={placed.jobId} />
        </div>
      )}
      <div style={{ marginTop: 12 }}>
        <Btn kind="primary" block disabled={!!missing} busy={busy} onClick={buy} testid="store-buy">{missing ?? `Buy job for ${sol} SOL`}</Btn>
      </div>
    </>
  );
}

// ---------------------------------------------------------------- rewards

interface RewardsView {
  height: number;
  finalityK: number;
  scanned: { from: number; to: number };
  rewardReported: boolean;
  totals: { blocks: number; maturedBlocks: number; immatureBlocks: number; jobBlocks: number; maturedFees: string; immatureFees: string; maturedReward: string; immatureReward: string };
  blocks: { height: number; hash: string; fees: string; reward?: string; jobBlock: boolean; confirmations: number; matured: boolean }[];
}

function Rewards({ st }: { st: WalletState }) {
  const r = usePoll(() => call<RewardsView>("miningRewards", {}), 15000, [st.network.id, st.selected?.address]);
  const d = r.data;
  return (
    <>
      <div className="card" data-testid="rewards">
        <div className="hero">
          <div className="eyebrow">Blocks mined by this account</div>
          <div className="hero-amount num" style={{ marginTop: 6 }}>{d ? d.totals.blocks : <span className="skel" />}<span className="hero-unit">blocks</span></div>
          <div className="small subtle num" style={{ marginTop: 4 }}>{d ? `${d.totals.jobBlocks} on paid jobs, ${d.totals.blocks - d.totals.jobBlocks} fallback` : " "}</div>
        </div>
        <div className="chain-split">
          <div>
            <div className="eyebrow">Matured</div>
            <div className="asset-amount num" style={{ marginTop: 5 }}>{d ? d.totals.maturedBlocks : "-"}<span className="unit">blocks</span></div>
            <div className="small subtle num">{d ? (d.rewardReported ? nat(d.totals.maturedReward) : `fees ${natFee(d.totals.maturedFees)}`) : " "}</div>
          </div>
          <div>
            <div className="eyebrow">Immature</div>
            <div className="asset-amount num" style={{ marginTop: 5 }}>{d ? d.totals.immatureBlocks : "-"}<span className="unit">blocks</span></div>
            <div className="small subtle num">{d ? (d.rewardReported ? nat(d.totals.immatureReward) : `fees ${natFee(d.totals.immatureFees)}`) : " "}</div>
          </div>
        </div>
      </div>
      {r.error && <div style={{ marginTop: 10 }}><Notice kind="warn">{r.error}</Notice></div>}
      {d && (
        <div style={{ marginTop: 10 }}>
          <Notice>
            Rewards mature {d.finalityK} blocks after the block. Scanned heights {d.scanned.from.toLocaleString("en-US")} to {d.scanned.to.toLocaleString("en-US")}.
            {!d.rewardReported && " The node's getBlock does not report the job subsidy, so only fees are summed; matured subsidy is already in your balance."}
          </Notice>
        </div>
      )}
      <div className="section-title"><span className="eyebrow">Recent blocks</span><button className="icon-btn" aria-label="Rescan" onClick={r.reload}>{Icon.refresh}</button></div>
      {d && d.blocks.length === 0 ? (
        <div className="card"><Empty title="No blocks mined by this account" body={`Run services/miner with --payout ${short(st.selected!.address, 4)} against ${st.network.chainRpc}.`} /></div>
      ) : (
        <div className="card divided">
          {(d?.blocks ?? []).map((b) => (
            <div className="list-row" key={b.hash}>
              <div className="glyph">{Icon.pick}</div>
              <div className="grow">
                <div style={{ fontWeight: 600 }} className="num">#{b.height.toLocaleString("en-US")}</div>
                <div className="small faint num">{b.jobBlock ? "job block" : "fallback"} · {b.confirmations} confirmations</div>
              </div>
              <div className="right">
                <div className="num small">{b.reward ? nat(b.reward) : `fees ${formatUnits(b.fees, NATIVE_DECIMALS, 9)}`}</div>
                <span className={`badge ${b.matured ? "ok" : "warn"}`}>{b.matured ? "matured" : `${d!.finalityK - b.confirmations} to go`}</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}
