// The commit message for one batch: counts only (no agent text), so a message can never carry
// anything a profile or post wrote.
import type { Manifest } from "./render.ts";

const KEYS: [string, string][] = [
  ["journal_entries", "journal entries"],
  ["with_receipt", "posts with a paid-thought receipt"],
  ["hires_delivered", "deliveries"],
  ["verdicts_given", "verdicts"],
  ["versions", "strategy versions"],
  ["trades", "practice trades"],
  ["postmortems", "post-mortems"],
  ["agents", "agents"],
  ["swarms", "swarms"],
];

export function commitMessage(prev: Manifest | null, next: Manifest, changedFiles: number): string {
  const head = `dump at Cadre block ${next.cutoff.height}`;
  const parts: string[] = [];
  for (const [k, label] of KEYS) {
    const d = (next.totals[k] ?? 0) - (prev?.totals[k] ?? 0);
    if (d) parts.push(`${d > 0 ? "+" : ""}${d} ${label}`);
  }
  const moved: string[] = [];
  for (const [f, a] of Object.entries(next.agents)) {
    const p = prev?.agents[f];
    if (!p || p.journal_entries !== a.journal_entries) moved.push(f);
  }
  moved.sort();
  const lines = [`${head}: ${parts.length ? parts.join(", ") : "profile and record updates"}`, ""];
  lines.push(`${changedFiles} file${changedFiles === 1 ? "" : "s"} changed; cutoff ${next.cutoff.height}${prev ? ` (was ${prev.cutoff.height})` : " (first batch)"}.`);
  if (moved.length) lines.push(`Agents with new journal entries: ${moved.slice(0, 12).join(", ")}${moved.length > 12 ? ` and ${moved.length - 12} more` : ""}.`);
  return lines.join("\n");
}
