import assert from "node:assert/strict";
import { test } from "node:test";
import { Budget, split } from "../src/budget.ts";

const cfg = { rowsPerDay: 1000, agentRowsCap: 400, topupLamportsPerDay: 100n, transferUnitsPerDay: 50n, agentFloat: 10n };
const DAY = 86_400_000;

test("split by weight with a floor", () => {
  assert.deepEqual(split(1000, { a: 1, b: 3 }), { a: 250, b: 750 });
  assert.deepEqual(split(1000, { a: 1, b: 99 }, 50), { a: 50, b: 990 });
});

test("day cap, agent cap, release, day roll", () => {
  const t0 = Date.UTC(2026, 9, 20, 12);
  const b = new Budget(cfg, { a: 1, b: 1, c: 1 }, undefined, t0);
  assert.equal(b.agentCap("a"), 400); // min(hard cap 400, 3 x 333)
  assert.ok(b.reserve("a", 300, t0));
  b.commit("a", 300, 250);
  assert.ok(!b.reserve("a", 200, t0), "agent cap 400: 250 + 200");
  assert.ok(b.reserve("a", 150, t0));
  b.commit("a", 150, 150);
  assert.ok(b.reserve("b", 390, t0));
  b.commit("b", 390, 390);
  assert.ok(b.reserve("c", 210, t0));
  assert.ok(!b.reserve("c", 1, t0), "day cap 1000 counts the open reservation");
  b.release(210);
  assert.ok(b.reserve("c", 210, t0));
  b.commit("c", 210, 210);
  assert.equal(b.st.rows, 1000);
  assert.ok(!b.reserve("c", 1, t0));
  assert.ok(b.st.refused.day_rows >= 1 && b.st.refused.agent_rows >= 1);
  assert.ok(b.reserve("c", 100, t0 + DAY), "next UTC day starts fresh");
});

test("top-up and transfer caps", () => {
  const t0 = Date.UTC(2026, 9, 20, 12);
  const b = new Budget(cfg, { a: 1 }, undefined, t0);
  assert.ok(b.canTopup(60n, t0));
  b.bookTopup(60n);
  assert.ok(!b.canTopup(60n, t0));
  assert.ok(b.canTransfer(50n, t0));
  b.bookTransfer(50n);
  assert.ok(!b.canTransfer(1n, t0));
  assert.ok(b.canTopup(60n, t0 + DAY));
});
