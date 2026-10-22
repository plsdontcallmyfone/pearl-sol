// Markdown helpers. Every string that came from an agent, a user or the web goes through esc()
// (inline) or quote() (block) before it lands in a file: it is data, never markup.

const BIDI = /[‪-‮⁦-⁩‎‏؜]/g;
// C0 controls except \t and \n, DEL, C1 controls
const CTRL = /[\u0000-\u0008\u000B-\u001F\u007F-\u009F]/g;

/** Plain text safe for one markdown line: no links, images, HTML, emphasis, headings or tables. */
export function esc(input: unknown): string {
  let s = String(input ?? "");
  s = s.replace(/\r\n?/g, "\n").replace(CTRL, "").replace(BIDI, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`);
  s = s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  s = s.replace(/([\\`*_\[\]|~])/g, "\\$1");
  // block syntax only bites at the start of a line: headings, lists, quotes, setext, thematic breaks
  s = s.split("\n").map((l) => l.replace(/^(\s*)([#+\-=])/, "$1\\$2").replace(/^(\s*\d+)([.)])/, "$1\\$2")).join(" ");
  return s;
}

/** A multi-line text block as an escaped blockquote, one quoted line per input line. */
export function quote(input: unknown): string {
  const s = String(input ?? "").replace(/\r\n?/g, "\n");
  return s.split("\n").map((l) => (l.trim() ? `> ${esc(l)}` : ">")).join("\n");
}

/** A fenced code block whose fence is longer than any backtick run in the body. */
export function fence(body: string, lang = ""): string {
  const runs = body.match(/`+/g) ?? [];
  const n = Math.max(3, ...runs.map((r) => r.length + 1));
  const f = "`".repeat(n);
  return `${f}${lang}\n${body.replace(/\r\n?/g, "\n").replace(CTRL, "").replace(BIDI, "")}\n${f}`;
}

/** Inline code for ids and hashes we produced or validated (hex, base58, req ids). */
export function code(id: string): string {
  if (!/^[A-Za-z0-9_:.-]{1,200}$/.test(id)) return esc(id);
  return "`" + id + "`";
}

export const short = (h: string, n = 8) => (h.length > n * 2 + 1 ? `${h.slice(0, n)}...${h.slice(-4)}` : h);

/** A markdown link whose text and URL we built ourselves (never user text in the URL). */
export function link(text: string, url: string): string {
  return `[${text}](${url.replace(/[()\s]/g, encodeURIComponent)})`;
}

export function table(head: string[], rows: string[][]): string {
  const cell = (c: string) => c.replace(/\n/g, " ").replace(/\|/g, "\\|");
  return [`| ${head.join(" | ")} |`, `|${head.map(() => "---").join("|")}|`, ...rows.map((r) => `| ${r.map(cell).join(" | ")} |`)].join("\n");
}

/** Lamports as SOL with 9 decimals, trimmed; base-unit strings only (never floats). */
export function units(v: string | bigint | null | undefined, dp: number, unit: string): string {
  if (v === null || v === undefined || v === "") return "n/a";
  let b = BigInt(v);
  const neg = b < 0n;
  if (neg) b = -b;
  const d = 10n ** BigInt(dp);
  const frac = (b % d).toString().padStart(dp, "0").replace(/0+$/, "");
  return `${neg ? "-" : ""}${b / d}${frac ? "." + frac : ""} ${unit}`;
}
export const sol = (v: string | bigint | null | undefined) => units(v, 9, "SOL");
export const cadre = (v: string | bigint | null | undefined) => units(v, 9, "CADRE");

export function utc(ms: number): { date: string; time: string } {
  const iso = new Date(ms).toISOString();
  return { date: iso.slice(0, 10), time: iso.slice(11, 19) };
}
