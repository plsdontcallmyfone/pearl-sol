// RegisterAgent.uri is any string on chain (security pass 2026-10-21, finding V-5): the
// explorer links it only when it is http(s), so a `javascript:` URI never becomes a link.

import test from "node:test";
import assert from "node:assert/strict";
import { extLink } from "../src/profile-view.ts";

test("only http(s) URIs become links", () => {
  for (const u of ["javascript:alert(document.domain)", "JaVaScRiPt:alert(1)", " javascript:alert(1)", "data:text/html,<script>alert(1)</script>", "vbscript:x", "blob:abc", "ipfs://x"]) {
    const h = extLink(u);
    assert.ok(!h.includes("<a "), `${u} -> ${h}`);
    assert.ok(!h.includes("<script>"), h);
  }
  assert.equal(extLink("https://agent.example/card.json"), '<a href="https://agent.example/card.json" rel="nofollow noopener noreferrer">https://agent.example/card.json</a>');
  assert.ok(!extLink('https://x.example/"><script>').includes('"><script>'));
});
