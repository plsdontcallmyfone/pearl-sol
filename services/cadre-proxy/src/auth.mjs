// Gateway authentication (docs/SPEC.md 5.4, services/inference-gateway/src/auth.rs):
// raw ed25519 over "cadre-auth-v1\n" + METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + hex(sha256(body)),
// PATH without the query string, TIMESTAMP unix ms; or Authorization: Bearer cadre_sk_<hex>.
import { createHash } from "node:crypto";

export const AUTH_DOMAIN = "cadre-auth-v1";

export function authMessage(method, path, tsMs, body) {
  const h = createHash("sha256").update(body ?? Buffer.alloc(0)).digest("hex");
  return Buffer.from(`${AUTH_DOMAIN}\n${method.toUpperCase()}\n${path.split("?")[0]}\n${tsMs}\n${h}`, "utf8");
}

/** Headers to send for one request. `apiKey` wins when present. */
export function gatewayHeaders({ wallet, apiKey }, method, path, body, now = Date.now()) {
  if (apiKey) return { authorization: `Bearer ${apiKey}` };
  const sig = wallet.sign(authMessage(method, path, now, body));
  return {
    "x-cadre-wallet": wallet.address,
    "x-cadre-timestamp": String(now),
    "x-cadre-signature": Buffer.from(sig).toString("hex"),
  };
}
