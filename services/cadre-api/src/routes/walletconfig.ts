// Signed Cadre Wallet remote config (cadre-extension docs/REMOTE-CONFIG.md), served as a file.
// The API does not hold the signing key and does not sign: the operator signs offline with
// scripts/cadre/sign-wallet-config.mjs and drops the file at WALLET_CONFIG_FILE. The wallet
// verifies the ed25519 signature itself, so this route only checks the shape and serves the
// bytes as they are. Read at every request, so publishing is replacing the file.

import { readFileSync, statSync } from "node:fs";
import { Api, ApiError, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";

/** A wallet config is a few KB; the wallet refuses anything over 64 KiB. */
const MAX_BYTES = 64 * 1024;

export function registerWalletConfig(api: Api, d: Deps) {
  api.get("/v1/wallet/config", () => {
    const file = d.cfg.walletConfigFile;
    if (!file) throw notFound("no wallet config is published on this API (WALLET_CONFIG_FILE unset)");
    let text: string;
    try {
      if (statSync(file).size > MAX_BYTES) throw new ApiError(500, "internal", "the wallet config file is too large");
      text = readFileSync(file, "utf8");
    } catch (e) {
      if (e instanceof ApiError) throw e;
      throw notFound("the wallet config file is missing");
    }
    let doc: unknown;
    try {
      doc = JSON.parse(text);
    } catch {
      throw new ApiError(500, "internal", "the wallet config file is not JSON");
    }
    const sig = (doc as { signature?: { ed25519?: unknown } } | null)?.signature;
    if (typeof doc !== "object" || doc === null || Array.isArray(doc) || typeof sig?.ed25519 !== "string")
      throw new ApiError(500, "internal", "the wallet config file is not a signed config");
    return reply({ text: { type: "application/json; charset=utf-8", body: text }, cache: "no-store" });
  }, { cache: "no-store" });
}
