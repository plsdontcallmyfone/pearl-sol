import { createHash } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { feeFor, messageId, signTx, type Body } from "../src/chaintx.ts";
import { CadreApi, Gateway, NodeRpc, latestBlockhash } from "../src/clients.ts";
import { sealDm } from "../src/dmcrypto.ts";
import { dmConfigFromEnv, type DmConfig } from "../src/dms.ts";
import { PERSONAS } from "../src/personas.ts";
import { type Agent, mulberry32, Swarm, type SwarmConfig } from "../src/swarm.ts";
import { walletFromSeed, type Wallet } from "../src/wallet.ts";
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

export function makeSwarm(f: Fakes, cfg: SwarmConfig, nAgents: number, clock: { t: number }, logs: any[] = [], offset = 0) {
  const op = walletFromSeed(seedOf("operator"));
  const agents = PERSONAS.slice(offset, offset + nAgents).map((persona) => ({ persona, wallet: walletFromSeed(seedOf(`agent-${persona.handle}`)) }));
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

// ---------------------------------------------------------------- DMs (test/dms.test.ts, soak, injection)

const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");

export function dmCfg(over: Partial<DmConfig> = {}): DmConfig {
  return dmConfigFromEnv({}, 1000, { pollMs: 0, agentDmsPerDay: 0, ...over });
}

let humans = 0;
export function human(f: Fakes, withEnc = true): Wallet {
  const w = walletFromSeed(seedOf(`human-${humans++}-${Date.now()}`));
  f.node.balances.set(w.address, 10_000_000_000n);
  if (withEnc) apply(f, w, { kind: "SetProfile", handle: `human_${humans}_${process.pid}`.slice(0, 24), name: "A human", profileUri: "", profileHash: new Uint8Array(32), flags: 0, operator: new Uint8Array(32), encKey: w.encKey });
  return w;
}

export function apply(f: Fakes, w: Wallet, body: Body, hold = false): { hash: string; nonce: bigint } {
  const nonce = f.node.nonces.get(w.address) ?? 0n;
  const tx = signTx(w, f.node.chainId, nonce, feeFor(body, f.node.params), body);
  return { hash: hold ? f.node.hold(tx) : f.node.apply(tx), nonce };
}

export function sendDm(f: Fakes, w: Wallet, to: Agent, o: { text: string; sealed?: boolean; pay?: bigint; reclaimAfter?: number; hold?: boolean }): string {
  const body = o.sealed ? sealDm({ from: w.publicKey, to: to.wallet.publicKey, toEncKey: to.wallet.encKey, selfEncKey: w.encKey, text: o.text }) : Uint8Array.from(Buffer.from(o.text));
  const { nonce } = apply(f, w, { kind: "SendMessage", to: to.wallet.publicKey, mode: o.sealed ? 1 : 0, pay: o.pay ?? 0n, reclaimAfter: o.reclaimAfter ?? 0, body }, o.hold);
  return hex(messageId(w.publicKey, nonce));
}

