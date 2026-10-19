// Resolve extensionless relative imports to .ts (DefiLlama's adapters import "../../adapters/types"
// without an extension; Node's type stripping needs the file name).
import { register } from "node:module";
register("data:text/javascript," + encodeURIComponent(`
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
export async function resolve(spec, ctx, next) {
  if ((spec.startsWith("./") || spec.startsWith("../")) && !/\\.[a-z]+$/.test(spec)) {
    for (const ext of [".ts", "/index.ts"]) {
      const u = new URL(spec + ext, ctx.parentURL);
      if (existsSync(fileURLToPath(u))) return next(u.href, ctx);
    }
  }
  return next(spec, ctx);
}`), import.meta.url);
