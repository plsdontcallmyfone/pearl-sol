// Price knobs on the chain (SPEC 4.6 config keys 24-26): storefront-x402 uses the chain's
// price_base_lamports / price_mlamports_per_gflop / price_overhead_lamports where set and its
// own environment defaults otherwise. No LiteSVM here: pure pricing.

import { test } from "node:test";
import assert from "node:assert/strict";
import { Keypair } from "@solana/web3.js";
import { livePricing, Service } from "../src/service.ts";

const service = { baseLamports: 10_000_000n, lamportsPerGflop: 100_000n, overheadLamports: 14_000_000n };
const cfg: any = { buyback: { bps: 1000, dest: 0 }, minSolIn: 1n, maxSolIn: 10_000_000_000n };

test("livePricing: chain keys override, empty keys fall back to the service", () => {
  const none = livePricing(service, null);
  assert.deepEqual(none.source, { base: "service", work: "service", overhead: "service" });
  assert.equal(none.mlamportsPerGflop, 100_000_000n, "env lamports per GFLOP x 1000");
  const some = livePricing(service, { price_base_lamports: null, price_mlamports_per_gflop: "10410", price_overhead_lamports: "0" });
  assert.equal(some.baseLamports, 10_000_000n);
  assert.equal(some.mlamportsPerGflop, 10_410n);
  assert.equal(some.overheadLamports, 0n, "a chain value of 0 is a real 0, not unset");
  assert.deepEqual(some.source, { base: "service", work: "chain", overhead: "chain" });
});

test("price: the milli-lamport knob gives the same price as the env knob at x1000, and finer steps", async () => {
  const s = new Service({ chain: null as any, wallet: Keypair.generate(), network: "solana-localnet", publicUrl: "http://t", pricing: service });
  // Llama-3.2-1B: 1,235,746,816 MACs per row (SPEC 4.5); a 4096-row balance
  const flops = 2n * 1_235_746_816n * 4096n;
  const env = s.price(cfg, 4095, 1, 1, 1, flops);
  const same = s.price(cfg, 4095, 1, 1, 1, flops, livePricing(service, { price_mlamports_per_gflop: "100000000" }));
  assert.equal(env.solIn, same.solIn);
  const cheap = s.price(cfg, 4095, 1, 1, 1, flops, livePricing(service, { price_mlamports_per_gflop: "10410" }));
  // work = ceil(flops x 10410 / 1e12) lamports
  const work = (flops * 10_410n + 999_999_999_999n) / 1_000_000_000_000n;
  const job = 10_000_000n + work;
  assert.equal(cheap.solIn, (job * 10_000n + 8_999n) / 9_000n, "grossed up for the 1000 bps buyback as before");
  assert.equal(cheap.price, cheap.solIn + 14_000_000n);
  // pricing() reads getParams and caches it
  let calls = 0;
  const t = new Service({ chain: null as any, wallet: Keypair.generate(), network: "solana-localnet", publicUrl: "http://t", pricing: service,
    getParams: async () => { calls += 1; return { price_mlamports_per_gflop: "10410" }; } });
  assert.equal((await t.pricing()).mlamportsPerGflop, 10_410n);
  await t.pricing();
  assert.equal(calls, 1, "cached for 30 s");
  const down = new Service({ chain: null as any, wallet: Keypair.generate(), network: "solana-localnet", publicUrl: "http://t", pricing: service,
    getParams: async () => { throw new Error("node down"); } });
  assert.equal((await down.pricing()).source.work, "service", "an unreachable node falls back to the service knobs");
});
