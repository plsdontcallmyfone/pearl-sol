import { createHash } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CadreApi, Gateway, NodeRpc, latestBlockhash } from "../src/clients.ts";
import { PERSONAS } from "../src/personas.ts";
import { mulberry32, Swarm, type SwarmConfig } from "../src/swarm.ts";
import { walletFromSeed } from "../src/wallet.ts";
import { PORTS, type Fakes } from "./fakes.ts";

export const seedOf = (s: string) => createHash("sha256").update(s).digest();

export function testConfig(over: Partial<SwarmConfig> = {}): SwarmConfig {
  const dir = mkdtempSync(join(tmpdir(), "swarm-test-"));
  return {
    budget: { rowsPerDay: 6000, agentRowsCap: 1500, topupLamportsPerDay: 10_000_000n, transferUnitsPerDay: 2_000_000_000n, agentFloat: 50_000_000n },
    maxTokens: 48, judgeMaxTokens: 2, topupRows: 4096, topupRoundSecs: 10800, minFreeRows: 1024, mempoolMax: 500, cadenceScale: 0.05,
    killFile: join(dir, "STOP"), stateFile: join(dir, "state.json"), allowHosts: ["64-95-12-116.sslip.io"], house: true,
    reactShare: 0.2, followShare: 0.1, replyShare: 0.25, solRpc: `http://127.0.0.1:${PORTS.sol}`, ...over,
  };
}

export function makeSwarm(f: Fakes, cfg: SwarmConfig, nAgents: number, clock: { t: number }, logs: any[] = []) {
  const op = walletFromSeed(seedOf("operator"));
  const agents = PERSONAS.slice(0, nAgents).map((persona) => ({ persona, wallet: walletFromSeed(seedOf(`agent-${persona.handle}`)) }));
  f.node.balances.set(op.address, 100_000_000_000n);
  f.node.params.house_operators = [op.address];
  const sw = new Swarm(cfg, {
    node: new NodeRpc(`http://127.0.0.1:${PORTS.node}`),
    gateway: new Gateway(`http://127.0.0.1:${PORTS.gateway}`, op),
    api: new CadreApi(`http://127.0.0.1:${PORTS.api}`),
    latestBlockhash,
    now: () => clock.t,
    rand: mulberry32(7),
    log: (e) => logs.push(e),
  }, op, agents);
  return { sw, op, agents, logs };
}
