#!/usr/bin/env node
// cadre-agent: run your Cadre agent on your own machine, key kept local (docs/ECOSYSTEM.md 7.5).
// The same runtime as the hosted agents and the house swarm (AgentRunner, a Swarm subclass); only
// the key's home differs. Node 22.18 or later, no dependencies.
//
//   cadre-agent keygen <key.json>                       new agent key (0600); prints the address only
//   cadre-agent address <key.json>                      print the address
//   cadre-agent run --key <key.json> --spec <spec.json> run the agent (the site's "Self-hosted" step
//                                                       gives you spec.json, signed by your wallet)
//   cadre-agent status --key <key.json> --spec <spec.json>   one bootstrap pass, then the status JSON
//   cadre-agent withdraw --key <key.json> --spec <spec.json> send every balance back to the operator
// Options: --api (cadre-api, default the public devnet one), --node (Cadre chain RPC), --gateway
// (default <api>/v1/openai), --solana (Solana RPC for x402 blockhashes), --state <dir>, --once.
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { resolve } from "node:path";
import { spawn } from "node:child_process";
import { CadreApi, latestBlockhash, NodeRpc } from "../../src/clients.ts";
import { createKeyFile, loadWallet } from "../../src/wallet.ts";
import { AgentRunner } from "../src/agent.ts";
import { ApiClient } from "../src/chain.ts";
import { GatewayAt } from "../src/gateway.ts";
import { type SignedSpec, verifySigned } from "../src/spec.ts";

const DEVNET = { api: "https://api.64-95-12-116.sslip.io", node: "https://rpc.64-95-12-116.sslip.io", solana: "https://api.devnet.solana.com" };

function opts(argv: string[]) {
  const o: Record<string, string | boolean> = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const k = a.slice(2);
    const v = argv[i + 1];
    if (v === undefined || v.startsWith("--")) o[k] = true;
    else { o[k] = v; i++; }
  }
  return o;
}

function runner(o: Record<string, string | boolean>) {
  const keyFile = String(o.key ?? "");
  const specFile = String(o.spec ?? "");
  if (!keyFile || !specFile) throw new Error("--key <key.json> and --spec <spec.json> are required");
  const doc: SignedSpec = JSON.parse(readFileSync(specFile, "utf8"));
  const v = verifySigned(doc);
  if (!v.ok) throw new Error(`the spec does not verify: ${v.problems.join("; ")}`);
  const w = loadWallet(keyFile);
  if (w.address !== doc.spec.agent) throw new Error(`the key is ${w.address}, the spec is for ${doc.spec.agent}`);
  const api = String(o.api ?? process.env.CADRE_API ?? DEVNET.api).replace(/\/$/, "");
  const state = resolve(String(o.state ?? `${homedir()}/.cadre-agent`));
  mkdirSync(state, { recursive: true, mode: 0o700 });
  const r = new AgentRunner(doc, w, {
    node: new NodeRpc(String(o.node ?? process.env.CHAIN_RPC ?? DEVNET.node)),
    gateway: new GatewayAt(String(o.gateway ?? `${api}/v1/openai`), w),
    api: new CadreApi(api),
    latestBlockhash,
    now: () => Date.now(),
    rand: Math.random,
    log: (e) => console.log(JSON.stringify(e)),
  }, new ApiClient(api), {
    stateFile: `${state}/agent-${w.address}.json`, killFile: `${state}/STOP`, allowHosts: String(o["allow-hosts"] ?? "64-95-12-116.sslip.io").split(","),
    solRpc: String(o.solana ?? process.env.SOLANA_RPC ?? DEVNET.solana), topupRows: Number(o["topup-rows"] ?? 1024), minFreeRows: Number(o["min-free-rows"] ?? 256),
    cardUrl: (a) => `${api}/v1/useragents/${a}/card`,
  });
  return { r, doc, keyFile, node: String(o.node ?? process.env.CHAIN_RPC ?? DEVNET.node), state };
}

/** mine: start pearl-miner with the agent key as the payout key, when the binary is installed. */
function startMiner(keyFile: string, node: string): boolean {
  const bins = [process.env.PEARL_MINER, `${homedir()}/.cadre/bin/pearl-miner`, "pearl-miner"].filter(Boolean) as string[];
  const bin = bins.find((b) => b === "pearl-miner" || existsSync(b));
  if (!bin) return false;
  const p = spawn(bin, ["--rpc", node, "--payout", keyFile, "--threads", String(process.env.MINER_THREADS ?? 2)], { stdio: "inherit" });
  p.on("error", () => console.log(JSON.stringify({ event: "miner_unavailable", reason: "pearl-miner is not installed (curl <site>/install-miner.sh | bash)" })));
  return true;
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  const o = opts(rest);
  if (cmd === "keygen") {
    if (!rest[0]) throw new Error("usage: keygen <key.json>");
    const r = createKeyFile(resolve(rest[0]));
    console.log(JSON.stringify({ address: r.address, created: r.created, file: resolve(rest[0]) }));
    return;
  }
  if (cmd === "address") {
    console.log(loadWallet(resolve(rest[0])).address);
    return;
  }
  if (cmd === "status") {
    const { r } = runner(o);
    await r.bootstrap().catch((e) => { r.lastError = String(e.message); });
    console.log(JSON.stringify(r.statusJson(), null, 2));
    return;
  }
  if (cmd === "withdraw") {
    const { r } = runner(o);
    console.log(JSON.stringify(await r.withdraw(), null, 2));
    return;
  }
  if (cmd !== "run") {
    console.log("usage: cadre-agent keygen <key.json> | address <key.json> | run|status|withdraw --key <key.json> --spec <spec.json> [--api URL] [--node URL] [--gateway URL] [--solana URL] [--state DIR] [--once]");
    process.exit(cmd ? 1 : 0);
  }
  const { r, doc, keyFile, node, state } = runner(o);
  if (doc.spec.actions.mine && !startMiner(keyFile, node)) console.log(JSON.stringify({ event: "miner_unavailable", reason: "mine is allowed but pearl-miner is not installed" }));
  let stop = false;
  process.on("SIGINT", () => (stop = true));
  process.on("SIGTERM", () => (stop = true));
  console.log(JSON.stringify({ event: "start", agent: r.address, operator: doc.spec.operator, spec: doc.sha256, state }));
  do {
    const t = await r.tick();
    console.log(JSON.stringify({ event: "tick", result: t, state: r.state, why: r.why || null }));
    if (o.once) break;
    await new Promise((res) => setTimeout(res, Number(o["tick-ms"] ?? 20_000)));
  } while (!stop && !existsSync(`${state}/STOP`));
}

main().catch((e) => {
  console.error(JSON.stringify({ event: "fatal", error: String(e?.message ?? e) }));
  process.exit(1);
});
