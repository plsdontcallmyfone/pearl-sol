// One-time setup of the town program's Config (docs/TOWN-ON-SOLANA.md section 7). Anyone can
// run init_config once, so run this right after the deploy. Idempotent: an existing Config is
// printed and left alone (set_config by the admin changes it later).
//   node scripts/town-init.ts --rpc <SOLANA_RPC> --program <id> --admin <keypair.json> \
//     --house <operator pubkey> [--house ...] [--reclaim-min 300] [--reclaim-max 604800]
import { loadWallet } from "../services/swarm/src/wallet.ts";
import { TownNode } from "../services/swarm/src/townnode.ts";
import * as TS from "../apps/explorer/src/townsol.ts";

const args = process.argv.slice(2);
const one = (k: string, d?: string) => {
  const i = args.indexOf(k);
  if (i < 0 || i + 1 >= args.length) {
    if (d === undefined) throw new Error(`missing ${k}`);
    return d;
  }
  return args[i + 1];
};
const all = (k: string) => args.flatMap((a, i) => (a === k && i + 1 < args.length ? [args[i + 1]] : []));

const rpc = one("--rpc");
const program = one("--program");
const admin = loadWallet(one("--admin"));
const house = all("--house");
const min = Number(one("--reclaim-min", "300"));
const max = Number(one("--reclaim-max", "604800"));
if (house.length > TS.LIMITS.house) throw new Error(`at most ${TS.LIMITS.house} house operators`);

const tn = new TownNode("http://127.0.0.1:1", { solRpc: rpc, program });
const cur = await tn.config();
if (cur) {
  console.log(JSON.stringify({ event: "config_exists", config: TS.configPda(program), ...cur }));
} else {
  const sig = await tn.sendIxs([TS.initConfigIx(admin.address, house, min, max, program)], admin);
  for (let i = 0; i < 60; i++) {
    const c = await tn.config();
    if (c) {
      console.log(JSON.stringify({ event: "config_initialized", tx: sig, config: TS.configPda(program), ...c }));
      process.exit(0);
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`init_config ${sig} sent but the Config did not appear`);
}
