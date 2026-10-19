// openapi.yaml is the contract: every route the server registers is in it (same method and
// path), and every path in it is served. Parameter names are compared positionally.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build, OPENAPI_FILE } from "../src/app.ts";

const norm = (p: string) => p.replace(/\{[^}]+\}/g, "{}").replace(/:[a-zA-Z_]+\*?/g, "{}");

function specOps(): Set<string> {
  const out = new Set<string>();
  let path: string | null = null;
  for (const line of readFileSync(OPENAPI_FILE, "utf8").split("\n")) {
    const m = line.match(/^  (\/\S*):\s*$/);
    if (m) {
      path = m[1];
      continue;
    }
    if (/^\S/.test(line)) path = null;
    const op = line.match(/^    (get|post|put|delete|patch):/);
    if (path && op) out.add(`${op[1].toUpperCase()} ${norm(path)}`);
  }
  return out;
}

test("every served route is in openapi.yaml and every documented path is served", () => {
  const d = new Deps(configFromEnv({}, { stateDir: "/nonexistent" }));
  const api = build(d);
  const served = new Set(api.routes.filter((r) => r.pattern !== "/").map((r) => `${r.method} ${norm(r.pattern)}`));
  const spec = specOps();
  const missingInSpec = [...served].filter((x) => !spec.has(x));
  const missingInServer = [...spec].filter((x) => !served.has(x));
  assert.deepEqual(missingInSpec, [], "served but not documented");
  assert.deepEqual(missingInServer, [], "documented but not served");
  assert.ok(spec.size >= 40);
});

test("openapi.yaml: 3.1, every $ref resolves, no em dashes", () => {
  const y = readFileSync(OPENAPI_FILE, "utf8");
  assert.match(y, /^openapi: 3\.1\.0$/m);
  for (const m of y.matchAll(/\$ref: "#\/components\/(\w+)\/(\w+)"/g)) assert.match(y, new RegExp(`^    ${m[2]}:`, "m"), `${m[1]}/${m[2]} defined`);
  assert.equal(y.includes("—"), false);
});
