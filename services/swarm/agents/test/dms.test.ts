// User agents and DMs: an agent whose spec allows replies publishes its enc_key and answers a paid
// DM (sealed in -> sealed out) as one paid thought inside the spec's budget, releasing the escrow;
// an agent whose spec has actions.reply false publishes its enc_key with a closed inbox and never
// answers. Same fakes as runtime.test.ts (ports 10733-10737); the agent spec v1 schema is unchanged.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { feeFor, messageId, signTx, type Body } from "../../src/chaintx.ts";
import { CadreApi, latestBlockhash, NodeRpc } from "../../src/clients.ts";
import { sealDm } from "../../src/dmcrypto.ts";
import { mulberry32 } from "../../src/swarm.ts";
import { walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { AgentRunner } from "../src/agent.ts";
import { ApiClient } from "../src/chain.ts";
import { GatewayAt } from "../src/gateway.ts";
import { defaultSpec, specMessage, specSha256, type AgentSpec } from "../src/spec.ts";
import { PORTS, startFakes2, type Fakes2 } from "./fakes.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const op = walletFromSeed(seedOf("dm-operator"));
const human = walletFromSeed(seedOf("dm-human"));
let f: Fakes2;
const ANSWER = "The boats are in and the gulls are loud.";

before(async () => {
  f = await startFakes2((msgs) => (/Answer yes or no/.test(msgs.at(-1)!.content) ? "no" : ANSWER));
});
after(async () => f.close());

function apply(w: Wallet, body: Body): bigint {
  const n = f.node.nonces.get(w.address) ?? 0n;
  f.node.apply(signTx(w, f.node.chainId, n, feeFor(body, f.node.params), body));
  return n;
}

function runner(name: string, reply: boolean) {
  const ag = walletFromSeed(seedOf(`dm-agent-${name}`));
  const spec: AgentSpec = defaultSpec(ag.address, op.address, {
    handle: `dm_${name}`, name: `DM ${name}`, persona: "A harbour clerk who answers letters.", appearance: "a clerk with a stamp", topics: ["letters"], created_ms: 1,
    schedule: { cadence_min: 1440, quiet_utc: null }, actions: { post: false, reply, react: false, follow: false, tip: false, trade: false, memo: false, hire: false, mine: false, browse: false },
    budget: { sol_lamports_per_day: "60000000", cadre_units_per_day: "5000000", rows_per_day: 3000 },
  });
  const sha256 = specSha256(spec);
  const doc = { spec, sha256, sig: hex(op.sign(Buffer.from(specMessage(spec, sha256)))) };
  const dir = mkdtempSync(join(tmpdir(), "agents-dm-"));
  let t = Date.UTC(2026, 9, 21, 12);
  const r = new AgentRunner(doc, ag, {
    node: new NodeRpc(`http://127.0.0.1:${PORTS.node}`), gateway: new GatewayAt(`http://127.0.0.1:${PORTS.gateway}`, ag), api: new CadreApi(`http://127.0.0.1:${PORTS.api}`),
    latestBlockhash, now: () => (t += 30_000), rand: mulberry32(5), log: () => {},
  }, new ApiClient(`http://127.0.0.1:${PORTS.api}`), {
    stateFile: join(dir, "state.json"), killFile: join(dir, "STOP"), allowHosts: [], solRpc: `http://127.0.0.1:${PORTS.sol}`, topupRows: 4096, minFreeRows: 256, cardUrl: () => "https://api.example/card", tickMs: 10,
  });
  f.node.balances.set(ag.address, 100_000_000n);
  f.node.balances.set(op.address, 100_000_000n);
  f.gw.rowsFree = 100_000;
  return { r, ag };
}

async function boot(r: AgentRunner, ag: Wallet) {
  for (let i = 0; i < 12; i++) {
    await r.tick();
    if (f.node.agents.has(ag.address) && !f.node.links.has(`${op.address}:${ag.address}`)) apply(op, { kind: "OperatorLink", agent: ag.publicKey, add: true });
    if (f.node.profiles.get(ag.address)?.encKey?.some((v: number) => v)) return;
  }
}

test("a user agent with actions.reply answers a sealed paid DM inside its budget and publishes its enc_key", async () => {
  f.node.balances.set(human.address, 10n ** 12n);
  apply(human, { kind: "SetProfile", handle: "dm_human", name: "Human", profileUri: "", profileHash: new Uint8Array(32), flags: 0, operator: new Uint8Array(32), encKey: human.encKey });
  const { r, ag } = runner("yes", true);
  await boot(r, ag);
  const p = f.node.profiles.get(ag.address);
  assert.equal(hex(p.encKey), hex(ag.encKey), "enc_key published");
  assert.equal(r.state, "running");
  // the fake api has no pump quote, so the inbox keeps its current (open) policy and the reason is logged
  assert.equal(p.dmPolicy, 0);
  const body = sealDm({ from: human.publicKey, to: ag.publicKey, toEncKey: ag.encKey, selfEncKey: human.encKey, text: "Did the boats come in?" });
  const n = apply(human, { kind: "SendMessage", to: ag.publicKey, mode: 1, pay: 1000n, reclaimAfter: 300, body });
  const id = hex(messageId(human.publicKey, n));
  const before = f.gw.receipts.size;
  const res = await r.tick();
  assert.equal(res, "dm_reply");
  assert.equal(f.gw.receipts.size, before + 1, "one paid thought");
  const rep = f.node.sent.filter((s) => s.kind === "SendMessage" && s.from === ag.address).at(-1)!;
  assert.equal(hex(rep.body.replyTo), id);
  assert.equal(human.openDm(rep.body.body, ag.publicKey, human.publicKey), ANSWER);
  assert.ok(f.node.released.includes(id), "escrow released to the agent");
  assert.ok(r.budget.st.rows > 0 && r.budget.st.rows <= 3000, "inside the spec's rows_per_day");
  assert.ok(BigInt(r.budget.st.transferUnits) <= 5_000_000n, "fees inside the spec's CADRE cap");
});

test("a user agent without actions.reply publishes its enc_key with a closed inbox and never answers", async () => {
  const { r, ag } = runner("no", false);
  await boot(r, ag);
  const p = f.node.profiles.get(ag.address);
  assert.equal(hex(p.encKey), hex(ag.encKey));
  assert.equal(p.dmPolicy, 3, "closed inbox");
  assert.throws(() => apply(human, { kind: "SendMessage", to: ag.publicKey, mode: 0, body: Buffer.from("hello?") }), /closed/);
  for (let i = 0; i < 3; i++) await r.tick();
  assert.equal(f.node.sent.filter((s) => s.kind === "SendMessage" && s.from === ag.address).length, 0);
});
