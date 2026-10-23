// Catalog rules: cadre_strategies is a live read in both modes; the three strategies writes are
// off for every driver with the reason in their listing (the strategies contract is the agent's
// own practice record, d79030e); the path guard admits only the /v1/strategies sub-routes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { byName, mcpTools, STRATEGY_WRITES_OFF, strategiesPath } from "../src/catalog.ts";

test("strategies: the read is live, the writes are off with their reason", () => {
  assert.equal(byName.get("cadre_strategies")!.status, "live");
  for (const n of ["cadre_strategy_commit", "cadre_strategy_trade", "cadre_strategy_postmortem"]) {
    const t = byName.get(n)!;
    assert.equal(t.status, "off");
    assert.equal(t.off, STRATEGY_WRITES_OFF);
    assert.ok(mcpTools("hosted").find((x) => x.name === n)!.description.startsWith("[off: "));
  }
  assert.ok(!mcpTools("hosted").find((x) => x.name === "cadre_strategies")!.description.startsWith("["));
});

test("strategies path guard", () => {
  assert.equal(strategiesPath(undefined), "/v1/strategies");
  assert.equal(strategiesPath("versions/12"), "/v1/strategies/versions/12");
  assert.equal(strategiesPath("/trades/3"), "/v1/strategies/trades/3");
  assert.equal(strategiesPath("knowledge"), "/v1/strategies/knowledge");
  assert.equal(strategiesPath("blobs/" + "ab".repeat(32)), "/v1/strategies/blobs/" + "ab".repeat(32));
  assert.equal(strategiesPath("postmortems?version=2&author=Fvqtw8jwj9Y97hMC5mu2VYRhAAi5z8GXHhp76X4qXRaP"), "/v1/strategies/postmortems?version=2&author=Fvqtw8jwj9Y97hMC5mu2VYRhAAi5z8GXHhp76X4qXRaP");
  for (const bad of ["..", "../agent", "versions/x", "versions/1/../../hosted", "knowledge?x=1", "postmortems?evil=1", "blobs/AB", "%2e%2e", "versions/1#x"]) assert.equal(strategiesPath(bad), null, bad);
});
