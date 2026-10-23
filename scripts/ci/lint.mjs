// House-rules lint for CI: no em dashes (U+2014) in any tracked text file. Detectors write the
// character as \u2014. Binary files (images, .so, keypairs) and the research clone are skipped.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const root = new URL("../..", import.meta.url).pathname;
const files = execFileSync("git", ["ls-files", "-z"], { cwd: root, encoding: "utf8" }).split("\0").filter(Boolean);
const text = /\.(rs|ts|tsx|mts|mjs|cjs|js|json|toml|ya?ml|md|sh|py|html|css|txt|sql)$/;
const EM = String.fromCharCode(0x2014);
const bad = [];
for (const f of files) {
  if (!text.test(f) || f.startsWith("research/")) continue;
  let src;
  try { src = readFileSync(root + f, "utf8"); } catch { continue; }
  if (!src.includes(EM)) continue;
  src.split("\n").forEach((l, i) => { if (l.includes(EM)) bad.push(`${f}:${i + 1}`); });
}
if (bad.length) {
  console.error(`em dash (U+2014) in ${bad.length} line(s):\n` + bad.join("\n"));
  process.exit(1);
}
console.log(`lint: ${files.length} tracked files, no em dashes`);
