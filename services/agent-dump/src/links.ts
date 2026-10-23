// Every URL the dump prints. Built from config and validated ids only, never from free text.

export interface LinkConfig {
  api: string; // public cadre-api base, e.g. https://api.64-95-12-116.sslip.io
  rpc: string; // public agent-chain RPC, e.g. https://rpc.64-95-12-116.sslip.io
  explorer: string; // https://explorer.64-95-12-116.sslip.io
  site: string; // https://cadre.64-95-12-116.sslip.io
  solanaCluster: string; // devnet
  repo: string; // cadredotag/agent-dump
}

export const DEVNET: LinkConfig = {
  api: "https://api.64-95-12-116.sslip.io",
  rpc: "https://rpc.64-95-12-116.sslip.io",
  explorer: "https://explorer.64-95-12-116.sslip.io",
  site: "https://cadre.64-95-12-116.sslip.io",
  solanaCluster: "devnet",
  repo: "cadredotag/agent-dump",
};

const HEX = /^[0-9a-f]{64}$/;
const B58 = /^[1-9A-HJ-NP-Za-km-z]{32,90}$/;
const REQ = /^req_[0-9a-f]{8,64}$/;
const HANDLE = /^[a-z0-9_]{1,64}$/;

const ok = (re: RegExp, v: string) => {
  if (!re.test(v)) throw new Error(`refusing to build a link from ${JSON.stringify(v).slice(0, 40)}`);
  return v;
};

export class Links {
  c: LinkConfig;
  constructor(c: LinkConfig) {
    this.c = c;
  }
  cadreTx = (h: string) => `${this.c.site}/explorer/tx/${ok(HEX, h)}`;
  block = (height: number) => `${this.c.explorer}/block/${Math.trunc(height)}`;
  ticket = (id: string) => `${this.c.api}/v1/inference/receipts/${ok(HEX, id)}`; // the ticket: fee, order, blocks mined over it, subsidy
  receipt = (req: string) => `${this.c.api}/v1/openai/requests/${ok(REQ, req)}`;
  solTx = (sig: string) => `https://explorer.solana.com/tx/${ok(B58, sig)}?cluster=${this.c.solanaCluster}`;
  solAddr = (a: string) => `https://explorer.solana.com/address/${ok(B58, a)}?cluster=${this.c.solanaCluster}`;
  account = (a: string) => `${this.c.site}/explorer/account/${ok(B58, a)}`;
  profile = (handle: string) => `${this.c.site}/u/${ok(HANDLE, handle)}`;
  agentExplorer = (a: string) => `${this.c.explorer}/agent/${ok(B58, a)}`;
  strategyBlob = (id: string) => `${this.c.api}/v1/strategies/blobs/${ok(HEX, id)}`;
  strategyVersion = (id: number) => `${this.c.api}/v1/strategies/versions/${Math.trunc(id)}`;
  strategyTrade = (id: number) => `${this.c.api}/v1/strategies/trades/${Math.trunc(id)}`;
  hire = (id: number) => `${this.c.api}/v1/market/hires/${Math.trunc(id)}`;
  swarm = (id: number) => `${this.c.api}/v1/swarms/${Math.trunc(id)}`;
  tx = (chain: string, hash: string) => (chain === "solana" ? this.solTx(hash) : this.cadreTx(hash));
}

export const isHex = (v: unknown): v is string => typeof v === "string" && HEX.test(v);
export const isB58 = (v: unknown): v is string => typeof v === "string" && B58.test(v);
export const isReq = (v: unknown): v is string => typeof v === "string" && REQ.test(v);
export const isHandle = (v: unknown): v is string => typeof v === "string" && HANDLE.test(v);
