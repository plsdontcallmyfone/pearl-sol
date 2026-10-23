// Stale swarm proposals (audit follow-up 2026-10-23, docs/SWARMS.md): the deployed swarms contract
// (admin zero) keeps a proposal acceptable after its lead leaves, and nobody can cancel it once
// the swarm has no lead. Members treat only an open proposal of the swarm's current lead as an
// invitation (proposalStanding), and the hosted runtime declines the others for invitees it hosts.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { b58decode } from "../../src/b58.ts";
import { walletFromSeed } from "../../src/wallet.ts";
import { Runtime } from "../src/runtime.ts";
import { decodeProposal, decodeSwarm, proposalStanding } from "../src/swarmabi.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const op = walletFromSeed(seedOf("stale-creator"));
const CONTRACT = walletFromSeed(seedOf("stale-contract")).address;
const LEAD = walletFromSeed(seedOf("stale-lead")).address;
const NEW_LEAD = walletFromSeed(seedOf("stale-new-lead")).address;

class W {
  p: Buffer[] = [];
  u8(v: number) { this.p.push(Buffer.from([v])); return this; }
  u32(v: number) { const b = Buffer.alloc(4); b.writeUInt32LE(v); this.p.push(b); return this; }
  u64(v: number | bigint) { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); this.p.push(b); return this; }
  addr(a: string | null) { this.p.push(a ? Buffer.from(b58decode(a)) : Buffer.alloc(32)); return this; }
  h(x: string) { this.p.push(Buffer.from(x, "hex")); return this; }
  s(t: string) { const b = Buffer.from(t); this.u32(b.length); this.p.push(b); return this; }
  hex() { return Buffer.concat(this.p).toString("hex"); }
}
// layouts of contracts/swarms/src/lib.rs (swarm(id), proposal(id), proposals(...))
const swarmHex = (lead: string | null, proposals: number) => new W().addr(op.address).addr(lead).addr(null).h("11".repeat(32)).h("22".repeat(32)).u64(50).u32(3).u32(2).u32(proposals).s("crew").s("blob:goal").hex();
const proposalHex = (agent: string, proposer: string, state: number) => new W().u64(7).addr(agent).addr(proposer).u8(state).u64(60).u64(0).s("scout").hex();

test("proposalStanding: only an open proposal of the current lead is an invitation", () => {
  const agent = walletFromSeed(seedOf("x")).address;
  const p = decodeProposal(3, proposalHex(agent, LEAD, 0));
  assert.equal(p.proposer, LEAD);
  assert.equal(proposalStanding(p, decodeSwarm(swarmHex(LEAD, 1))), "valid");
  assert.equal(proposalStanding(p, decodeSwarm(swarmHex(null, 1))), "no_lead", "the lead left: leave clears the lead");
  assert.equal(proposalStanding(p, decodeSwarm(swarmHex(NEW_LEAD, 1))), "lead_changed", "handed on, or someone else took the lead");
  assert.equal(proposalStanding(decodeProposal(3, proposalHex(agent, LEAD, 3)), decodeSwarm(swarmHex(null, 1))), "closed");
});

/** Fake node: the registration lookup and the swarms contract's swarm/proposal queries. */
class Node {
  sent: Buffer[] = [];
  lead: string | null = null;
  props = new Map<number, string>();
  async call(method: string, p: any): Promise<any> {
    switch (method) {
      case "getParams": return { min_fee: "1000", social_min_fee: "1000", gas_price: "10000", contract_base_fee: "1000", contract_byte_fee: "10", tx_gas_limit: "50000000" };
      case "getChainInfo": return { chain_id: 7, height: 100 };
      case "getAccount": return { balance: "10000000000", nonce: 0 };
      case "simulateCall": return { ok: true, gas_used: 900_000 };
      case "getTx": return null;
      case "queryContract": {
        const a = Buffer.from(p.args, "hex");
        const m = a.subarray(1, 1 + a[0]).toString();
        const rest = a.subarray(1 + a[0]);
        if (m === "by_definition") return { ok: true, return: new W().u64(7).hex() };
        if (m === "swarm") return { ok: true, return: swarmHex(this.lead, this.props.size) };
        if (m === "proposals") { const ids = [...this.props.keys()]; const w = new W().u32(ids.length).u8(ids.length); ids.forEach((i) => w.u64(i)); return { ok: true, return: w.hex() }; }
        if (m === "proposal") return { ok: true, return: this.props.get(Number(rest.readBigUInt64LE(0)))! };
        return { ok: false, error: "" };
      }
      default: throw new Error(`fake node: ${method}`);
    }
  }
  async sendTx(tx: Uint8Array) { this.sent.push(Buffer.from(tx)); return { tx_hash: createHash("sha256").update(tx).digest("hex") }; }
  async putBlob(data: Uint8Array) { const id = (await import("../../src/blake3.ts")).blake3(data); const h = Buffer.from(id).toString("hex"); return { blob_id: h, uri: `blob:${h}`, size: data.length }; }
}

test("hosted invitees decline a departed lead's proposal and leave a current lead's alone", async () => {
  const dir = mkdtempSync(join(tmpdir(), "agents-stale-"));
  const cfgFile = join(dir, "swarms.json.deploy");
  writeFileSync(cfgFile, JSON.stringify({ contract: CONTRACT, start_height: 0 }));
  process.env.SWARMS_CONFIG = cfgFile;
  process.env.AGENTS_SWARM_KICKOFF_WAIT_MS = "300";
  process.env.AGENTS_SWARM_KICKOFF_POLL_MS = "20";
  process.env.AGENTS_SWARM_PROPOSAL_SCAN_MS = "0";
  const node = new Node();
  let t = 1_792_500_000_000;
  const rt = new Runtime({
    stateDir: dir, masterKeyFile: join(dir, "master.hex"), nodeRpc: "http://127.0.0.1:1", gatewayUrl: "http://127.0.0.1:1", apiUrl: "http://127.0.0.1:1",
    publicApiUrl: "https://api.example", network: "test", allowHosts: [], maxPerOperator: 2, maxAgents: 50, topupRows: 4096, minFreeRows: 256, tickMs: 10, killFile: join(dir, "STOP"),
  }, () => ({ node: node as any, gateway: {} as any, api: {} as any, latestBlockhash: async () => "x", now: () => (t += 61_000), rand: () => 0.5, log: () => {} }));
  const post = (path: string, body: unknown) => rt.handle("POST", path, {} as any, JSON.stringify(body)) as Promise<any>;
  const plan = await post("/v1/hosted/swarms/plan", { creator: op.address, goal: "Watch the harbour and write a daily report.", size: 2, total_sol_lamports: "200000000" });
  assert.equal(plan.status, 200, JSON.stringify(plan.body));
  const sig = Buffer.from(op.sign(Buffer.from(plan.body.creator_message, "utf8"))).toString("hex");
  assert.equal((await post("/v1/hosted/swarms", { plan_id: plan.body.plan_id, creator_sig: sig })).status, 201);
  for (let i = 0; i < 200 && (rt as any).busy.size; i++) await new Promise((res) => setTimeout(res, 5));
  const rec = rt.swarms.swarms.get(plan.body.plan_id)!;
  rec.registration.state = "registered";
  rec.registration.id = 7;
  const invitee = rt.runners.get(plan.body.members[1].agent)!;
  invitee.state = "running";
  const declines = () => node.sent.filter((x) => x.includes(Buffer.from("decline")));
  // proposal 3 by a lead who has since left (the swarm has no lead), proposal 4 by the current lead
  node.props.set(3, proposalHex(invitee.address, LEAD, 0));
  node.lead = null;
  await rt.swarms.staleProposalsTick();
  assert.equal(declines().length, 1, "the departed lead's open proposal is declined by its invitee");
  assert.equal(Buffer.from(declines()[0].subarray(5, 37)).toString("hex"), Buffer.from(invitee.me.wallet.publicKey).toString("hex"), "signed by the invited agent");
  const pid = declines()[0].subarray(declines()[0].indexOf(Buffer.from("decline")) + 7, declines()[0].indexOf(Buffer.from("decline")) + 15).readBigUInt64LE(0);
  assert.equal(pid, 3n);
  node.sent.length = 0;
  node.props.set(3, proposalHex(invitee.address, LEAD, 3)); // declined on the record now
  node.props.set(4, proposalHex(invitee.address, NEW_LEAD, 0));
  node.lead = NEW_LEAD;
  await rt.swarms.staleProposalsTick();
  assert.equal(declines().length, 0, "a current lead's proposal (and a closed one) is left alone; the runtime never accepts on its own");
});
