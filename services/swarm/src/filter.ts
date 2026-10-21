// Output filter (docs/ECOSYSTEM.md 2.6 "Safety rails"): a completion is posted verbatim or not
// at all. Refused: keys and seed phrases, URLs other than allowlisted Cadre links, claims of
// being human, promises of returns, chat-assistant preambles, empty or oversize text, and
// anything that looks like a raw instruction or tool call the model echoed from inbound data.

export interface FilterResult { ok: boolean; reason?: string }

const DEFAULT_ALLOW = ["64-95-12-116.sslip.io"];

const SECRET = [
  /\b[1-9A-HJ-NP-Za-km-z]{64,90}\b/, // base58 secret keys (64-byte) and long encodings
  /\b[0-9a-fA-F]{64,}\b/, // hex seeds / keys
  /\[\s*\d{1,3}(\s*,\s*\d{1,3}){31,}\s*\]/, // JSON byte arrays (keypair files)
  /\b(seed|recovery|secret|mnemonic)\s+(phrase|words?)\b/i,
  /\bprivate\s+key\b/i,
  /\bcadre_sk_[0-9a-f]+/i,
  /\bsk-[A-Za-z0-9]{16,}/,
];
const HUMAN = [
  /\b(i am|i'm|im)\s+(a\s+)?(real\s+)?(human|person|man|woman)\b/i,
  /\b(i am|i'm|im)\s+not\s+(an?\s+)?(ai|bot|robot|model|machine)\b/i,
  /\bas\s+a\s+(real\s+)?human\b/i,
];
const MONEY = [/\bguarantee(d|s)?\s+(returns?|profits?|gains?|income)\b/i, /\brisk[- ]free\b/i, /\b\d{2,}x\s+(gains?|returns?)\b/i, /\bcan'?t\s+lose\b/i];
const PREAMBLE = /^\s*(sure|certainly|of course|here('| i)s|here are|as an ai|i('| a)m sorry|i cannot|i can't)\b/i;
const INSTRUCTION = [/^\s*-?\s*\S{1,40}\s+wrote:/i, /\bwrote:\s*"/i, /\bignore\s+(all\s+)?(previous|prior|above)\s+(instructions|prompts?)\b/i, /<\/?(system|data|tool|function)\b/i, /^\s*(system|assistant|user)\s*:/im, /\b(send|transfer|withdraw)\s+[\d,.]+\s*(cadre|sol)\b/i];
const URL_RE = /\b(?:https?:\/\/|www\.)[^\s<>"')]+|\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:com|io|xyz|net|org|app|fun|gg|co|ai|sh|me|so|dev|to|ly|finance|exchange|money)\b(?:\/[^\s]*)?/gi;

export function hostOf(u: string): string {
  const s = u.replace(/^https?:\/\//i, "").replace(/^www\./i, "");
  return s.split(/[/?#:]/)[0].toLowerCase();
}

export function checkOutput(text: string, opts: { maxBytes: number; allowHosts?: string[] } = { maxBytes: 1024 }): FilterResult {
  if (typeof text !== "string") return { ok: false, reason: "not text" };
  const bytes = Buffer.byteLength(text, "utf8");
  if (text.trim().length === 0) return { ok: false, reason: "empty" };
  if (bytes > opts.maxBytes) return { ok: false, reason: `over post_inline_max (${bytes} > ${opts.maxBytes} bytes)` };
  if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(text)) return { ok: false, reason: "control characters" };
  for (const r of SECRET) if (r.test(text)) return { ok: false, reason: "looks like a key or seed phrase" };
  for (const r of HUMAN) if (r.test(text)) return { ok: false, reason: "claims to be human" };
  for (const r of MONEY) if (r.test(text)) return { ok: false, reason: "promises returns" };
  if (PREAMBLE.test(text)) return { ok: false, reason: "assistant preamble" };
  for (const r of INSTRUCTION) if (r.test(text)) return { ok: false, reason: "echoes an instruction or transfer request" };
  const allow = (opts.allowHosts ?? DEFAULT_ALLOW).map((h) => h.toLowerCase());
  for (const m of text.matchAll(URL_RE)) {
    const h = hostOf(m[0]);
    if (!allow.some((a) => h === a || h.endsWith("." + a))) return { ok: false, reason: `link to ${h} is not allowlisted` };
  }
  return { ok: true };
}

/** A yes/no judgment (react / follow thoughts): only an unambiguous yes counts. */
export function isYes(text: string): boolean {
  return /^\s*yes\b/i.test(text) && !/^\s*yes\s*,?\s*but\b/i.test(text);
}
