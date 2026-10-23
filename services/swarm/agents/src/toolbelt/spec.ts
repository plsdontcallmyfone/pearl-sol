// The agent toolbelt section of a v2 agent spec (docs/TOOLBELT.md 2): the off-chain abilities a
// hosted agent may use, each a toggle with its own hard limits fixed by the creator at creation and
// enforced by the hosted runtime (this folder), never by the model. Absent (or null) = no toolbelt;
// specs signed before 2026-10-22 have no `toolbelt` key, so their hashes are unchanged.
//
// Every number here is a bound the creator chose (the site proposes the defaults below; they are
// design choices, not measurements). Spend caps are in lamports of the agent's own prepaid toolbelt
// balance (docs/TOOLBELT.md 4), per capability per UTC day; call counts are per UTC day; rates are
// per minute. Swarms: every agent of a swarm has its own spec, so its own caps and its own meter.
//
// Social accounts (X, Telegram, Discord) are NOT in M1 (owner 2026-10-22, "social isn't great").

export const CAPABILITIES = ["web", "computer", "code", "files", "email", "http"] as const;
export type Capability = (typeof CAPABILITIES)[number];

/** What each capability can and cannot do, shown in the site and in status (docs/TOOLBELT.md 3). */
export const CAN: Record<Capability, { can: string[]; cannot: string[] }> = {
  web: {
    can: ["search the public web (one search returns up to 10 results)", "fetch a public page and read it as plain text (GET only, at most max_page_kb)"],
    cannot: ["log in anywhere", "submit forms or POST", "reach private networks, localhost or cloud metadata addresses", "take instructions from page text (pages are quoted data)"],
  },
  computer: {
    can: ["run a short task in a fresh sandboxed Linux desktop with a browser, operated by a vision model, from a start URL", "return what it saw and a final screenshot (stored as a file)"],
    cannot: ["use the agent's wallet or keys (none are in the sandbox)", "keep state between tasks (every task gets a new sandbox, destroyed after)", "run longer than max_task_s or past minutes_per_day"],
  },
  code: {
    can: ["run code (bash, python, node) in a fresh sandbox with no secrets in it, for at most max_run_s", "with github.provided: get its own public repo in the Cadre GitHub org (named from its handle), commit files and open pull requests there, as the Cadre app's bot with its handle, Cx address and a signed statement in every commit"],
    cannot: ["see any GitHub token (the runtime mints a one-hour token scoped to the agent's own repo for each write; sandboxes never get one)", "write to any repo but its own (pull requests to outside repos are off in M1)", "force-push, delete repos, change settings or add workflow files"],
  },
  files: {
    can: ["store an artifact (a report, a dataset, a screenshot) as a content-addressed blob on the Cadre node; its blake3 hash goes into the tool receipts committed on Cadre"],
    cannot: ["store more than max_file_kb per file or max_kb_per_day", "prove that the content is true or good (a hash proves only which bytes were delivered)"],
  },
  email: {
    can: ["receive mail in the agent's own inbox", "send mail to the allowlisted recipients, or reply to a thread someone else started, inside sends_per_day"],
    cannot: ["send to anyone else", "send more than sends_per_day", "send attachments larger than the files cap"],
  },
  http: {
    can: ["call a public HTTP API whose host is on the allowlist fixed at creation, with the methods the spec allows"],
    cannot: ["call any other host", "reach private networks", "send the agent's key or any sealed credential (the runtime never puts them in a request)"],
  },
};

export interface CapBase {
  lamports_per_day: string; // most this capability may spend from the toolbelt balance per UTC day
  calls_per_day: number;
  per_minute: number;
}
export interface WebCap extends CapBase { max_page_kb: number; max_results: number }
export interface ComputerCap extends CapBase { max_task_s: number; minutes_per_day: number; max_steps: number }
/** GitHub that Cadre provides (owner 2026-10-22): the Cadre GitHub App, repos in a Cadre-owned org,
 * one repo per agent named from its handle (more up to max_repos), public, short-lived tokens. */
export interface GithubCap { provided: true; max_repos: number; commits_per_day: number; prs_per_day: number; max_repo_mb: number; outside_prs: false }
export interface CodeCap extends CapBase { max_run_s: number; github: GithubCap | null }
export interface FilesCap extends CapBase { max_file_kb: number; max_kb_per_day: number }
export interface EmailCap extends CapBase { sends_per_day: number; recipients: string[]; reply_to_inbound: boolean }
export interface HttpCap extends CapBase { allow: string[]; methods: ("GET" | "POST")[]; max_response_kb: number }

export interface ToolbeltSpec {
  /** a top-up from the agent's own SOL into its prepaid toolbelt balance, at most this per top-up */
  topup_lamports: string;
  /** receipts are committed on Cadre as one Merkle root per `batch_every` tool calls (or daily) */
  batch_every: number;
  /** most tool calls in one task (one market hire, one watch round) */
  max_calls_per_task: number;
  web?: WebCap | null;
  computer?: ComputerCap | null;
  code?: CodeCap | null;
  files?: FilesCap | null;
  email?: EmailCap | null;
  http?: HttpCap | null;
  /** optional standing watch: re-read these pages every `every_min` and post when one changes */
  watch?: { urls: string[]; every_min: number } | null;
}

export const enabled = (t: ToolbeltSpec | null | undefined): Capability[] => (t ? CAPABILITIES.filter((c) => !!(t as any)[c]) : []);

/** Proposed GitHub limits (design choices the creator edits, not measurements). */
export const defaultGithub = (): GithubCap => ({ provided: true, max_repos: 1, commits_per_day: 50, prs_per_day: 10, max_repo_mb: 100, outside_prs: false });

/** Proposed defaults (design choices the creator edits in the wizard, not measurements). */
export function defaultToolbelt(caps: (Capability | "github")[], over: Partial<ToolbeltSpec> = {}): ToolbeltSpec {
  const t: ToolbeltSpec = { topup_lamports: "20000000", batch_every: 16, max_calls_per_task: 12 };
  if (caps.includes("web")) t.web = { lamports_per_day: "10000000", calls_per_day: 200, per_minute: 10, max_page_kb: 512, max_results: 5 };
  if (caps.includes("computer")) t.computer = { lamports_per_day: "20000000", calls_per_day: 10, per_minute: 1, max_task_s: 300, minutes_per_day: 30, max_steps: 25 };
  if (caps.includes("code")) t.code = { lamports_per_day: "10000000", calls_per_day: 50, per_minute: 4, max_run_s: 120, github: null };
  if (caps.includes("code") && over.code === undefined && (caps as string[]).includes("github")) t.code!.github = defaultGithub();
  if (caps.includes("files")) t.files = { lamports_per_day: "0", calls_per_day: 100, per_minute: 10, max_file_kb: 1024, max_kb_per_day: 8192 };
  if (caps.includes("email")) t.email = { lamports_per_day: "2000000", calls_per_day: 100, per_minute: 4, sends_per_day: 20, recipients: [], reply_to_inbound: true };
  if (caps.includes("http")) t.http = { lamports_per_day: "2000000", calls_per_day: 200, per_minute: 20, allow: [], methods: ["GET"], max_response_kb: 512 };
  return { ...t, ...over };
}

const DEC = /^(0|[1-9][0-9]{0,19})$/;
const HOST = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const EMAIL = /^[^\s@<>"]{1,64}@([a-z0-9-]{1,63}\.)+[a-z]{2,63}$/i;

const KEYS: Record<string, string[]> = {
  web: ["max_page_kb", "max_results"],
  computer: ["max_task_s", "minutes_per_day", "max_steps"],
  code: ["max_run_s", "github"],
  files: ["max_file_kb", "max_kb_per_day"],
  email: ["sends_per_day", "recipients", "reply_to_inbound"],
  http: ["allow", "methods", "max_response_kb"],
};

/** Field checks for spec.toolbelt; returns the problems (empty = valid). */
export function validateToolbelt(t: any): string[] {
  const e: string[] = [];
  if (t === undefined || t === null) return e;
  if (typeof t !== "object" || Array.isArray(t)) return ["toolbelt is an object or null"];
  const top = ["topup_lamports", "batch_every", "max_calls_per_task", "watch", ...CAPABILITIES];
  if (Object.keys(t).some((k) => !top.includes(k))) e.push(`toolbelt has only ${top.join(", ")} (social accounts are not in M1)`);
  if (!DEC.test(t.topup_lamports ?? "") || BigInt(t.topup_lamports ?? 0) < 1_000_000n || BigInt(t.topup_lamports ?? 0) > 5_000_000_000n) e.push("toolbelt.topup_lamports is a decimal string in 1000000..5000000000");
  const int = (o: any, path: string, k: string, lo: number, hi: number) => { if (!Number.isInteger(o?.[k]) || o[k] < lo || o[k] > hi) e.push(`${path}.${k} is an integer in ${lo}..${hi}`); };
  int(t, "toolbelt", "batch_every", 1, 256);
  int(t, "toolbelt", "max_calls_per_task", 1, 64);
  const on = enabled(t);
  if (!on.length) e.push("toolbelt turns on at least one capability (or is null)");
  for (const c of on) {
    const o = t[c];
    const p = `toolbelt.${c}`;
    if (typeof o !== "object" || Array.isArray(o)) { e.push(`${p} is an object or null`); continue; }
    const allowed = ["lamports_per_day", "calls_per_day", "per_minute", ...KEYS[c]];
    if (Object.keys(o).some((k) => !allowed.includes(k)) || allowed.some((k) => o[k] === undefined)) e.push(`${p} has exactly ${allowed.join(", ")}`);
    if (!DEC.test(o.lamports_per_day ?? "")) e.push(`${p}.lamports_per_day is a decimal string`);
    int(o, p, "calls_per_day", 1, 100_000);
    int(o, p, "per_minute", 1, 600);
  }
  if (t.web) { int(t.web, "toolbelt.web", "max_page_kb", 8, 4096); int(t.web, "toolbelt.web", "max_results", 1, 10); }
  if (t.computer) { int(t.computer, "toolbelt.computer", "max_task_s", 30, 1800); int(t.computer, "toolbelt.computer", "minutes_per_day", 1, 1440); int(t.computer, "toolbelt.computer", "max_steps", 1, 100); }
  if (t.code) {
    int(t.code, "toolbelt.code", "max_run_s", 5, 900);
    const g = t.code.github;
    if (g !== null && g !== undefined) {
      const gk = ["provided", "max_repos", "commits_per_day", "prs_per_day", "max_repo_mb", "outside_prs"];
      if (typeof g !== "object" || Object.keys(g).some((k) => !gk.includes(k)) || gk.some((k) => g[k] === undefined)) e.push(`toolbelt.code.github has exactly ${gk.join(", ")}`);
      else {
        if (g.provided !== true) e.push("toolbelt.code.github.provided is true (Cadre provides the GitHub; creators connect no account)");
        int(g, "toolbelt.code.github", "max_repos", 1, 5);
        int(g, "toolbelt.code.github", "commits_per_day", 0, 500);
        int(g, "toolbelt.code.github", "prs_per_day", 0, 100);
        int(g, "toolbelt.code.github", "max_repo_mb", 1, 1000);
        if (g.outside_prs !== false) e.push("toolbelt.code.github.outside_prs is false (pull requests to outside repos are reserved for later)");
      }
    }
  }
  if (t.files) { int(t.files, "toolbelt.files", "max_file_kb", 1, 4000); int(t.files, "toolbelt.files", "max_kb_per_day", 1, 1_048_576); }
  if (t.email) {
    int(t.email, "toolbelt.email", "sends_per_day", 0, 1000);
    if (!Array.isArray(t.email.recipients) || t.email.recipients.length > 32 || t.email.recipients.some((r: unknown) => typeof r !== "string" || !EMAIL.test(r))) e.push("toolbelt.email.recipients is at most 32 email addresses");
    if (typeof t.email.reply_to_inbound !== "boolean") e.push("toolbelt.email.reply_to_inbound is a boolean");
  }
  if (t.http) {
    if (!Array.isArray(t.http.allow) || !t.http.allow.length || t.http.allow.length > 32 || t.http.allow.some((h: unknown) => typeof h !== "string" || !HOST.test(h))) e.push("toolbelt.http.allow is 1..32 lowercase host names (no wildcards, no IPs)");
    if (!Array.isArray(t.http.methods) || !t.http.methods.length || t.http.methods.some((m: unknown) => m !== "GET" && m !== "POST") || new Set(t.http.methods).size !== t.http.methods.length) e.push("toolbelt.http.methods is a subset of GET, POST");
    int(t.http, "toolbelt.http", "max_response_kb", 1, 4096);
  }
  if (t.watch !== undefined && t.watch !== null) {
    const w = t.watch;
    if (!t.web) e.push("toolbelt.watch needs toolbelt.web");
    if (typeof w !== "object" || !Array.isArray(w.urls) || !w.urls.length || w.urls.length > 8 || w.urls.some((u: unknown) => typeof u !== "string" || !/^https:\/\/[^\s]{4,500}$/.test(u)) || Object.keys(w).some((k) => k !== "urls" && k !== "every_min")) e.push("toolbelt.watch is {urls: 1..8 https URLs, every_min}");
    int(w, "toolbelt.watch", "every_min", 15, 1440);
  }
  return e;
}
