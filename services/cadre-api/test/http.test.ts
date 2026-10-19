// The HTTP layer alone: routing with params, method handling, token bucket, cache headers.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Api, reply } from "../src/http.ts";

const mk = (o: Partial<ConstructorParameters<typeof Api>[0]> = {}) => new Api({ allowedOrigins: "*", rateLimitPerMin: 60, rateBurst: 3, trustProxy: false, maxBody: 1024, ...o });

test("router: params, wildcard, HEAD on GET routes, 405 lists methods", () => {
  const a = mk();
  a.get("/v1/blocks/:id", () => 1);
  a.get("/v1/blocks/:id/proof", () => 2);
  a.post("/v1/openai/:rest*", () => 3);
  const m1 = a.match("GET", "/v1/blocks/12") as any;
  assert.equal(m1.route.pattern, "/v1/blocks/:id");
  assert.deepEqual(m1.params, { id: "12" });
  assert.equal((a.match("GET", "/v1/blocks/12/proof") as any).route.pattern, "/v1/blocks/:id/proof");
  assert.deepEqual((a.match("POST", "/v1/openai/chat/completions") as any).params, { rest: "chat/completions" });
  assert.ok((a.match("HEAD", "/v1/blocks/1") as any).route);
  assert.deepEqual((a.match("GET", "/v1/openai/x") as any).allowed, ["POST"]);
  assert.equal(a.match("GET", "/nope"), null);
});

test("token bucket: burst, refill, cost", () => {
  const a = mk({ rateLimitPerMin: 60, rateBurst: 3 });
  assert.equal(a.take("ip", 1).ok, true);
  assert.equal(a.take("ip", 1).ok, true);
  assert.equal(a.take("ip", 1).ok, true);
  const no = a.take("ip", 1);
  assert.equal(no.ok, false);
  assert.ok(no.resetS >= 1);
  assert.equal(a.take("other", 3).ok, true);
  assert.equal(a.take("other2", 5).ok, false, "cost above the burst never passes");
  assert.equal(mk({ rateLimitPerMin: 0 }).take("x", 99).ok, true, "0 disables");
});

test("client ip: X-Forwarded-For only behind a trusted proxy, last hop", () => {
  const req: any = { headers: { "x-forwarded-for": "1.1.1.1, 2.2.2.2" }, socket: { remoteAddress: "127.0.0.1" } };
  assert.equal(mk().clientIp(req), "127.0.0.1");
  assert.equal(mk({ trustProxy: true }).clientIp(req), "2.2.2.2");
});

test("cache-control policies", () => {
  const a = mk();
  assert.equal(a.cacheControl("no-store"), "no-store");
  assert.equal(a.cacheControl(0), "public, no-cache");
  assert.equal(a.cacheControl(5), "public, max-age=5, stale-while-revalidate=20");
  assert.ok(reply({ body: 1 }));
});
