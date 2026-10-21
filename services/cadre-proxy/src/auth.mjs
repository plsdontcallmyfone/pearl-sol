// Gateway authentication (docs/SPEC.md 5.4, services/inference-gateway/src/auth.rs):
// raw ed25519 over "cadre-auth-v2\n" + AUDIENCE + "\n" + METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + hex(sha256(body)),
// AUDIENCE = the gateway's server key (GET /v1/models -> data[0].cadre.server), PATH without the
// query string, TIMESTAMP unix ms; or Authorization: Bearer cadre_sk_<hex>. v2 binds the signature
// to one gateway (security pass V-9); v1 (no audience) is kept only for reference.
import { createHash } from "node:crypto";

export const AUTH_DOMAIN = "cadre-auth-v1";
export const AUTH_DOMAIN_V2 = "cadre-auth-v2";

export function authMessageV2(audience, method, path, tsMs, body) {
  const h = createHash("sha256").update(body ?? Buffer.alloc(0)).digest("hex");
  return Buffer.from(`${AUTH_DOMAIN_V2}\n${audience}\n${method.toUpperCase()}\n${path.split("?")[0]}\n${tsMs}\n${h}`, "utf8");
}

/** The gateway's server key (the v2 audience) from its GET /v1/models reply body. */
export function audienceOf(models) {
  const s = models?.data?.[0]?.cadre?.server;
  if (typeof s !== "string" || !s) throw new Error("the gateway's /v1/models names no cadre.server (cadre-auth-v2 audience)");
  return s;
}

/** Fetch the audience from `gatewayBase` + /v1/models. */
export async function fetchAudience(gatewayBase) {
  const r = await fetch(gatewayBase.replace(/\/+$/, "") + "/v1/models");
  if (!r.ok) throw new Error(`gateway /v1/models: HTTP ${r.status}`);
  return audienceOf(await r.json());
}

export function authMessage(method, path, tsMs, body) {
  const h = createHash("sha256").update(body ?? Buffer.alloc(0)).digest("hex");
  return Buffer.from(`${AUTH_DOMAIN}\n${method.toUpperCase()}\n${path.split("?")[0]}\n${tsMs}\n${h}`, "utf8");
}

/** Headers to send for one request. `apiKey` wins when present; a wallet signature needs the
 *  gateway's `audience` (cadre-auth-v2). */
export function gatewayHeaders({ wallet, apiKey, audience }, method, path, body, now = Date.now()) {
  if (apiKey) return { authorization: `Bearer ${apiKey}` };
  if (!audience) throw new Error("wallet auth needs the gateway's server key (cadre-auth-v2 audience)");
  const sig = wallet.sign(authMessageV2(audience, method, path, now, body));
  return {
    "x-cadre-wallet": wallet.address,
    "x-cadre-timestamp": String(now),
    "x-cadre-signature": Buffer.from(sig).toString("hex"),
  };
}
