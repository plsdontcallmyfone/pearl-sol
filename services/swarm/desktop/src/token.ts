// View-only stream tokens. cadre-api mints them for a page that shows a desktop; the controller
// accepts them on the HLS route only. A token names one agent and one desktop and expires (default
// 10 minutes). It grants a picture, nothing else: there is no route on the controller that takes
// input for a desktop, and E2B's own control endpoints are never exposed.
//   token = <exp_unix_s>.<base64url(HMAC-SHA256(secret, "cadre-desktop-stream\n" + handle + "\n" + role + "\n" + exp))>
// The same function lives in services/cadre-api/src/routes/desktops.ts (test vector in both test suites).
import { createHmac, timingSafeEqual } from "node:crypto";

export function mintStreamToken(secret: string | Buffer, handle: string, role: string, ttlS = 600, now = Date.now()): string {
  const exp = Math.floor(now / 1000) + ttlS;
  const sig = createHmac("sha256", secret).update(`cadre-desktop-stream\n${handle}\n${role}\n${exp}`).digest("base64url");
  return `${exp}.${sig}`;
}

export function checkStreamToken(secret: string | Buffer, handle: string, role: string, token: string, now = Date.now()): boolean {
  const m = /^(\d{10})\.([A-Za-z0-9_-]{43})$/.exec(token ?? "");
  if (!m) return false;
  const exp = Number(m[1]);
  if (exp < Math.floor(now / 1000)) return false;
  const want = createHmac("sha256", secret).update(`cadre-desktop-stream\n${handle}\n${role}\n${exp}`).digest();
  const got = Buffer.from(m[2]!, "base64url");
  return got.length === want.length && timingSafeEqual(got, want);
}
