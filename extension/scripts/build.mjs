// Builds the MV3 extension into dist/: Vite for the popup and approval pages,
// esbuild for the service worker (ESM) and the two content scripts (IIFE).
import { build as vite } from "vite";
import { build as esbuild } from "esbuild";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "dist");
rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

await vite({ root, logLevel: "warn", configFile: resolve(root, "vite.config.ts") });

const common = { bundle: true, minify: false, sourcemap: false, target: "chrome120", logLevel: "warning", legalComments: "none" };
await esbuild({ ...common, entryPoints: [resolve(root, "src/background/index.ts")], outfile: resolve(dist, "background.js"), format: "esm" });
await esbuild({ ...common, entryPoints: [resolve(root, "src/content/index.ts")], outfile: resolve(dist, "content.js"), format: "iife" });
await esbuild({ ...common, entryPoints: [resolve(root, "src/inpage/index.ts")], outfile: resolve(dist, "inpage.js"), format: "iife" });

const consts = readFileSync(resolve(root, "src/shared/constants.ts"), "utf8");
const name = /APP_NAME = "([^"]+)"/.exec(consts)[1];
const pkg = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
const manifest = {
  manifest_version: 3,
  name,
  version: pkg.version,
  description: "Wallet for Solana and the pearl-sol agent chain: one key, both chains.",
  action: { default_popup: "popup.html", default_title: name },
  background: { service_worker: "background.js", type: "module" },
  permissions: ["storage", "alarms", "tabs"],
  host_permissions: ["http://*/*", "https://*/*"],
  content_scripts: [
    { matches: ["http://*/*", "https://*/*"], js: ["content.js"], run_at: "document_start", all_frames: false },
    { matches: ["http://*/*", "https://*/*"], js: ["inpage.js"], run_at: "document_start", all_frames: false, world: "MAIN" },
  ],
  content_security_policy: { extension_pages: "script-src 'self'; object-src 'self'" },
  minimum_chrome_version: "120",
};
writeFileSync(resolve(dist, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
console.log(`built ${name} ${pkg.version} into dist/`);
