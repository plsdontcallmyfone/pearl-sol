// Template track record (routes/templates.ts) and the signed-spec attribution of v2 agents (routes/useragents.ts verifyAgent).
import assert from "node:assert/strict";
import { createHash, createPrivateKey, createPublicKey, sign as edSign } from "node:crypto";
import { test } from "node:test";
import { aggregate, FORMULA, rankTemplates, templateScore } from "../src/routes/templates.ts";
import { verifyAgent } from "../src/routes/useragents.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { b58encode, hex } from "../../../apps/explorer/src/codec.ts";
import { defaultSpecV2, profileBlob, specMessage, specSha256 } from "../../swarm/agents/src/spec.ts";

const NOW = 1_800_000_000_000;
const row = (agent: string, template: string | null, state = "running", counters: Record<string, number> = {}, created = NOW - 1000, live: number | null = NOW - 500) => ({ agent, state, spec: { template }, counters, hosted: { created_ms: created }, timings: { live_ms: live } });

test("aggregate counts only measured numbers, by template; untagged agents are not attributed", () => {
  const rows = [row("a1", "explainer", "running", { posts: 20, delivers: 5 }), row("a2", "explainer", "waiting_funds", {}, NOW - 2 * 86_400_000, null), row("a3", "momentum", "error"), row("a4", null)];
  const hires = [
    { provider: "a1", buyer: "x", status: "completed", delivered: 10 }, { provider: "a1", buyer: "x", status: "completed", delivered: 11 },
    { provider: "a1", buyer: "x", status: "rejected", delivered: 12 }, { provider: "a2", buyer: "x", status: "expired", delivered: null },
  ];
  const lines = [{ author: "a3", trades: 4, learned_fixes: 1 }];
  const s = aggregate(rows, hires, lines, NOW);
  const ex = s.find((x) => x.id === "explainer")!;
  assert.deepEqual([ex.agents, ex.active, ex.abandoned, ex.failing, ex.actions], [2, 1, 1, 0, 25]);
  assert.deepEqual([ex.hires_delivered, ex.hires_accepted, ex.hires_rejected, ex.hires_expired], [3, 2, 1, 1]);
  assert.equal(ex.practice_explained, 0);
  const mo = s.find((x) => x.id === "momentum")!;
  assert.deepEqual([mo.failing, mo.practice_explained, mo.practice_learned], [1, 4, 1]);
  assert.equal(s.length, 2, "the untagged agent is not counted");
  const unknown = aggregate(rows, null, null, NOW).find((x) => x.id === "explainer")!;
  assert.equal(unknown.hires_accepted, null, "unreadable = null, never zero");
});

test("ranking: the documented score; a template with no agents is New and last", () => {
  const base = { hires_delivered: 0, hires_accepted: 0, hires_rejected: 0, hires_expired: 0, hires_bought_completed: 0, practice_explained: 0, practice_learned: 0, abandoned: 0, failing: 0, active: 1 };
  const a = { ...base, id: "a", agents: 2, actions: 30, hires_accepted: 10, hires_rejected: 2 }; // proven 13, fail 2/14
  const b = { ...base, id: "b", agents: 1, actions: 0, practice_explained: 3 };
  const z = { ...base, id: "z", agents: 0, actions: 0, active: 0 };
  assert.equal(templateScore(a), Number((13 * (1 - 2 / 14)).toFixed(3)));
  assert.equal(templateScore(z), null);
  const r = rankTemplates([z, b, a]);
  assert.deepEqual(r.map((x) => x.id), ["a", "b", "z"]);
  assert.equal(r[2].label, "New");
  assert.equal(r[2].rank, null);
  assert.match(FORMULA, /failure rate/);
});

const PK8 = Buffer.from("302e020100300506032b657004220420", "hex");
function wallet(label: string) {
  const priv = createPrivateKey({ key: Buffer.concat([PK8, createHash("sha256").update(label).digest()]), format: "der", type: "pkcs8" });
  const der = createPublicKey(priv).export({ format: "der", type: "spki" }) as Buffer;
  return { address: b58encode(der.subarray(der.length - 32)), sign: (m: Uint8Array) => edSign(null, Buffer.from(m), priv) };
}

test("a v2 agent is attributed by its creator-signed spec without an OperatorLink; a v1 agent is not", async () => {
  const U = wallet("tpl-user"), A = wallet("tpl-agent");
  const s = defaultSpecV2(A.address, U.address, { handle: "tide_v2", name: "Tide", persona: "p", created_ms: 9 });
  const sha = specSha256(s);
  const blob = profileBlob({ spec: s, sha256: sha, sig: Buffer.from(U.sign(Buffer.from(specMessage(s, sha)))).toString("hex") });
  const id = hex(blake3(blob));
  const d: any = {
    chain: async (m: string, p: any) => {
      if (m === "getProfile") return { account: A.address, has_profile: true, profile_uri: `blob:${id}`, profile_hash: id, operator: U.address, operator_linked: false, set_height: 3 };
      if (m === "getAgent") return { account: A.address, registered_height: 4 };
      if (m === "getBlob") return p.uri === `blob:${id}` ? { data: blob.toString("base64") } : null;
      return null;
    },
  };
  const v = await verifyAgent(d, A.address);
  const link = v.checks.find((c) => c.name === "operator_linked")!;
  assert.equal(link.ok, true, link.detail);
  assert.match(link.detail, /creator-signed v2 spec/);
  assert.equal(v.verified, true);
});
