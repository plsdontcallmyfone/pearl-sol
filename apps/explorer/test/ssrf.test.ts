// Card probe SSRF guard (security pass 2026-10-21, finding V-1): every IPv6 spelling of a
// private IPv4 address is refused, and the socket's own DNS lookup refuses private answers.

import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { fetchCard, guardedLookup, ipv6Bytes, isPrivateAddress } from "../src/agentcard.ts";

const cfg = { probeMs: 1000, timeoutMs: 2000, staleMs: 10_000, activeMs: 5_000, allowPrivate: false };

test("IPv6 forms that embed a private IPv4 are private", () => {
  for (const ip of [
    "::ffff:7f00:1", // what new URL() makes of [::ffff:127.0.0.1]
    "::ffff:127.0.0.1",
    "0:0:0:0:0:ffff:7f00:0001",
    "::ffff:a9fe:a9fe", // 169.254.169.254, cloud metadata
    "::ffff:c0a8:101",
    "::7f00:1", // IPv4-compatible
    "::127.0.0.1",
    "64:ff9b::7f00:1", // NAT64
    "64:ff9b::a00:1",
    "64:ff9b:1::1",
    "2002:7f00:1::", // 6to4 of 127.0.0.1
    "2002:a9fe:a9fe::1",
    "2001:0:4136:e378:8000:63bf:3fff:fdd2", // Teredo
    "2001:db8::1",
    "fec0::1",
    "fe80::1%lo0",
    "::",
    "::1",
    "198.18.0.1",
    "192.0.0.1",
    "not-an-ip",
  ]) assert.ok(isPrivateAddress(ip), ip);
  for (const ip of ["8.8.8.8", "1.1.1.1", "2606:4700::1111", "::ffff:808:808", "64:ff9b::808:808", "2002:808:808::1", "2a00:1450:4001:80b::200e"]) assert.ok(!isPrivateAddress(ip), ip);
  assert.deepEqual(ipv6Bytes("::ffff:127.0.0.1")?.slice(10), [0xff, 0xff, 127, 0, 0, 1]);
  assert.equal(ipv6Bytes("1.2.3.4"), null);
});

test("fetchCard refuses mapped and NAT64 loopback URLs (the URL parser rewrites them)", async () => {
  const srv = createServer((_q, r) => r.end(JSON.stringify({ name: "internal" }))).listen(10691, "127.0.0.1");
  try {
    for (const u of ["http://[::ffff:127.0.0.1]:10691/", "http://[::ffff:7f00:1]:10691/", "http://[::127.0.0.1]:10691/", "http://[64:ff9b::7f00:1]:10691/"]) {
      const r = (await fetchCard(u, cfg)) as any;
      assert.equal(r.ok, false, u);
      assert.match(r.error, /private/, u);
    }
    assert.match(((await fetchCard("http://user:pw@example.com/", cfg)) as any).error, /credentials/);
  } finally {
    srv.close();
  }
});

test("the socket lookup refuses a name that resolves to loopback (DNS rebinding guard)", async () => {
  const err = await new Promise<NodeJS.ErrnoException | null>((res) => guardedLookup("localhost", { all: true }, (e) => res(e)));
  assert.ok(err);
  assert.equal(err!.code, "EPRIVATE");
  const err2 = await new Promise<NodeJS.ErrnoException | null>((res) => guardedLookup("localhost", {}, (e) => res(e)));
  assert.equal(err2?.code, "EPRIVATE");
});
