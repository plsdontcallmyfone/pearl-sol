// Swarms (docs/SWARMS.md, docs/API.md "Swarms"): many sovereign agents created in one go and
// registered on the swarms contract (contracts/swarms) with each member's own signed acceptance.
//   GET  /v1/swarms                      swarms newest first (?creator=, ?member= use the contract's indexes)
//   GET  /v1/swarms/:id                  one swarm (contract id, or the definition hash while registering):
//                                        members, roles, proposals, spend today, hires between members,
//                                        their deliveries (outputs), trading members' strategy records
//   GET  /v1/swarms/by-agent/:agent      every swarm an agent has been in
//   POST /v1/hosted/swarms/plan          pass-through to the hosted runtime {creator, goal, size,
//                                        total_sol_lamports, overrides?}, plus ONE unsigned Solana tx
//                                        sending every member its SOL
//   POST /v1/hosted/swarms               pass-through {plan_id, creator_sig}: the creator's one signature
//   GET  /v1/hosted/swarms(/styles)      pass-through
//   POST /v1/tx/swarm                    unsigned swarms contract calls (fund, leave, propose, ...)
// Every figure is a contract read (queryContract), a market contract read (the services market
// index), a strategies contract read, or the hosted runtime's own budget counters, each labelled
// with its source. No deploy file: 503 with the reason. Nothing is stubbed.
import type { Api, Ctx } from "../http.ts";
import { ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr, amount } from "./wallet.ts";
import { shared } from "../shared.ts";
import { TownState } from "./town.ts";
import { blake3 } from "../../../../apps/explorer/src/blake3.ts";
import { hex } from "../../../../apps/explorer/src/codec.ts";
import { TX_HDR, TX_SIG, contractFeeFloor, contractSigningBytes, encodeCallBody, framedMessage, isUnknownMethod } from "../../../../apps/explorer/src/contracts.ts";
import { compileLegacy, systemTransfer, unsignedTx, type Ix } from "../sol.ts";
import {
  callArgs, decodeCounts, decodeIds, decodeMembers, decodeProposal, decodeSwarm, decodeU64, goalBlob, KINDS, loadSwarmsConfig, proposalStanding, queryArgs, type SwarmKind, type SwarmRec, type SwarmsConfig,
} from "../../../swarm/agents/src/swarmabi.ts";

const HEX64 = /^[0-9a-f]{64}$/;
const DEC = /^(0|[1-9][0-9]{0,19})$/;
const FEE_RULE = "max(min_fee, contract_base_fee + contract_byte_fee x tx_len + ceil(gas_limit x gas_price / gas_price_unit)) from getParams; unused gas is refunded at gas_price";

export function registerSwarms(api: Api, d: Deps, env: NodeJS.ProcessEnv = process.env) {
  const town = new TownState(d);
  const need = (): SwarmsConfig => {
    const c = loadSwarmsConfig(env);
    if (!c) throw new ApiError(503, "unavailable", "no swarms contract is configured on this cadre-api (deploy/swarms-devnet.json or CADRE_SWARMS_CONFIG): the contract is not deployed yet");
    return c;
  };
  async function query(c: SwarmsConfig, args: Uint8Array, missing = "not found"): Promise<string> {
    let r: any;
    try {
      r = await d.chain<any>("queryContract", { address: c.contract, args: Buffer.from(args).toString("hex") });
    } catch (e) {
      if (isUnknownMethod(e)) throw new ApiError(503, "unavailable", "this node does not serve the contract RPC");
      throw e;
    }
    if (!r?.ok) {
      const msg = r?.error ? Buffer.from(r.error, "hex").toString("utf8") : "query failed";
      if (/no such/.test(msg)) throw notFound(`${missing} (${msg})`);
      throw new ApiError(502, "query_failed", msg);
    }
    return r.return as string;
  }

  // ---------------------------------------------------------------- hosted runtime

  const runtimeBase = () => env.CADRE_AGENTS_URL ?? process.env.CADRE_AGENTS_URL ?? "";
  let hostedCache: { at: number; v: Map<string, any> | null } = { at: 0, v: null };
  async function hosted(): Promise<Map<string, any> | null> {
    if (!runtimeBase()) return null;
    if (Date.now() - hostedCache.at < 5_000) return hostedCache.v;
    let v: Map<string, any> | null = null;
    try {
      const r = await fetch(runtimeBase().replace(/\/$/, "") + "/v1/hosted/swarms", { signal: AbortSignal.timeout(4000) });
      if (r.ok) v = new Map(((await r.json()).swarms ?? []).map((s: any) => [s.key, s]));
    } catch { v = null; }
    hostedCache = { at: Date.now(), v };
    return v;
  }

  // ---------------------------------------------------------------- reads

  const goalCache = new Map<string, { text: string | null; kind: SwarmKind | null; hash_ok: boolean | null }>();
  async function goalOf(rec: SwarmRec, h: any | null) {
    if (h?.goal && h.goal_hash === rec.goal_hash) {
      const g = goalBlob(h.goal, h.kind, h.name, h.proof?.specs);
      return { hash: rec.goal_hash, uri: rec.goal_uri, text: h.goal, kind: h.kind as SwarmKind, hash_ok: g.hash === rec.goal_hash, source: "hosted runtime, checked against goal_hash" };
    }
    let c = goalCache.get(rec.goal_hash);
    if (!c) {
      c = { text: null, kind: null, hash_ok: null };
      if (/^blob:[0-9a-f]{64}$/.test(rec.goal_uri)) {
        const b = await d.chain<any>("getBlob", { blob_id: rec.goal_uri }).catch(() => null);
        if (b?.data) {
          const data = Buffer.from(b.data, "base64");
          const ok = hex(blake3(data)) === rec.goal_hash;
          let j: any = null;
          try { j = JSON.parse(data.toString("utf8")); } catch { /* not JSON */ }
          c = { text: ok && typeof j?.goal === "string" ? j.goal : null, kind: ok && KINDS.includes(j?.kind) ? j.kind : null, hash_ok: ok };
          goalCache.set(rec.goal_hash, c);
        }
      }
    }
    return { hash: rec.goal_hash, uri: rec.goal_uri, text: c.text, kind: c.kind, hash_ok: c.hash_ok, source: c.hash_ok === null ? "the goal blob is not readable from the node" : "node blob, checked against goal_hash" };
  }

  async function swarmRec(c: SwarmsConfig, id: number): Promise<SwarmRec> {
    return decodeSwarm(await query(c, queryArgs.swarm(id), "no such swarm"));
  }

  async function summary(c: SwarmsConfig, id: number, hs: Map<string, any> | null) {
    const s = await swarmRec(c, id);
    const h = hs?.get(s.definition) ?? null;
    const goal = await goalOf(s, h);
    return {
      id, definition: s.definition, name: s.name, kind: goal.kind, goal, creator: s.creator, creator_role: "attribution",
      lead: s.lead, successor: s.successor, height: s.height, members_active: s.active, members_total: s.slots, proposals: s.proposals, hosted: !!h,
    };
  }

  function card(a: string) {
    try {
      const x = town.idx().card(a);
      return { handle: x.handle ?? null, name: x.name ?? null };
    } catch {
      return { handle: null, name: null };
    }
  }

  api.get("/v1/swarms", async (cx) => {
    const c = need();
    const creator = cx.query.get("creator");
    const member = cx.query.get("member");
    if (creator) addr(creator, "creator");
    if (member) addr(member, "member");
    if (creator && member) throw badRequest("filter by creator or by member, not both");
    const offset = Math.max(0, Number(cx.query.get("offset") ?? 0) || 0);
    const limit = Math.min(64, Math.max(1, Number(cx.query.get("limit") ?? 50) || 50));
    let ids: number[];
    let total: number;
    if (creator || member) {
      const r = decodeIds(await query(c, creator ? queryArgs.byCreator(creator, offset, limit) : queryArgs.byMember(member!, offset, limit)));
      ids = r.ids.reverse();
      total = r.total;
    } else {
      const n = decodeCounts(await query(c, queryArgs.counts()));
      total = n.swarms;
      ids = [];
      for (let i = n.swarms - offset; i >= 1 && ids.length < limit; i--) ids.push(i);
    }
    const hs = await hosted();
    const swarms = await Promise.all(ids.map((id) => summary(c, id, hs)));
    return { contract: c.contract, start_height: c.start_height, total, offset, swarms, source: "swarms contract (queryContract counts / by_creator / by_member / swarm)" };
  }, { cache: 5 });

  api.get("/v1/swarms/by-agent/:agent", async (cx) => {
    const c = need();
    const agent = addr(cx.params.agent, "agent");
    const r = decodeIds(await query(c, queryArgs.byMember(agent, 0, 64)));
    const hs = await hosted();
    const swarms = await Promise.all(r.ids.reverse().map(async (id) => {
      const s = await summary(c, id, hs);
      const m = Buffer.from(await query(c, queryArgs.member(id, agent)), "hex");
      let role = "", active = false, joined = 0, left: number | null = null;
      if (m[0] === 1) {
        active = m[1] === 1;
        const rl = m.readUInt32LE(2);
        role = m.subarray(6, 6 + rl).toString("utf8");
        joined = Number(m.readBigUInt64LE(6 + rl));
        left = Number(m.readBigUInt64LE(14 + rl)) || null;
      }
      return { ...s, role, active, joined_height: joined, left_height: left, is_lead: s.lead === agent };
    }));
    return { contract: c.contract, agent, total: r.total, swarms };
  }, { cache: 5 });

  api.get("/v1/swarms/:id", async (cx) => {
    const c = need();
    const raw = String(cx.params.id).toLowerCase();
    let id: number;
    const hs = await hosted();
    if (HEX64.test(raw)) {
      id = decodeU64(await query(c, queryArgs.byDefinition(raw)));
      if (!id) {
        const h = hs?.get(raw);
        if (!h) throw notFound("no swarm with this definition on the contract or the hosted runtime");
        return { id: null, definition: raw, contract: c.contract, name: h.name, kind: h.kind, goal: { hash: h.goal_hash, uri: h.goal_uri, text: h.goal, hash_ok: goalBlob(h.goal, h.kind, h.name, h.proof?.specs).hash === h.goal_hash }, creator: h.creator, creator_role: "attribution", lead: h.lead, registration: h.registration, kickoff: h.kickoff ?? null, members: h.members.map((m: any) => ({ agent: m.agent, handle: m.handle, role: m.role, is_lead: m.agent === h.lead, active: false, practice_style: m.style, hosted: m.hosted })), note: "not on the swarms contract yet: membership counts only once the contract holds every member's acceptance" };
      }
    } else {
      id = Number(raw);
      if (!Number.isSafeInteger(id) || id < 1) throw badRequest("id is a swarm number (1, 2, ...) or a definition hash (64 hex)");
    }
    const base = await summary(c, id, hs);
    const s = await swarmRec(c, id);
    const h = hs?.get(s.definition) ?? null;
    const slots = [];
    for (let off = 0; off < s.slots; off += 32) slots.push(...decodeMembers(await query(c, queryArgs.members(id, off, 32))).slots);
    const pids = s.proposals ? decodeIds(await query(c, queryArgs.proposals(id, 0, 64))).ids : [];
    const proposals = await Promise.all(pids.map(async (p) => decodeProposal(p, await query(c, queryArgs.proposal(p)))));
    const everyone = new Set(slots.map((x) => x.agent));
    const hostedOf = new Map<string, any>((h?.members ?? []).map((m: any) => [m.agent, m]));
    // market: hires between members (both sides members), and members' hires from outsiders
    let market: any = null;
    let marketWhy: string | null = null;
    if (shared.market) {
      try { market = await shared.market(); } catch (e) { marketWhy = (e as Error).message.slice(0, 160); }
    } else marketWhy = "no services market is configured on this cadre-api";
    const hires = market ? [...market.hires.values()].filter((x: any) => everyone.has(x.buyer) && x.created >= s.height) : [];
    const internal = hires.filter((x: any) => everyone.has(x.provider)).sort((a: any, b: any) => b.id - a.id);
    const offersBy = new Map<string, any[]>();
    if (market) for (const o of market.offers) if (everyone.has(o.provider) && o.active) offersBy.set(o.provider, [...(offersBy.get(o.provider) ?? []), { id: o.id, title: o.title, price: o.price }]);
    const hireView = (x: any) => ({ id: x.id, offer: x.offer, title: market.offer?.(x.offer)?.title ?? null, buyer: x.buyer, provider: x.provider, price: x.price, total: x.total, status: x.status, height: x.created, delivery: x.result_uri ? { result_uri: x.result_uri, result_hash: x.result_hash, delivered_height: x.delivered } : null });
    const outputs = await Promise.all(internal.filter((x: any) => x.result_uri).slice(0, 10).map(async (x: any) => {
      const v = hireView(x);
      const b = await d.chain<any>("getBlob", { blob_id: x.result_uri }).catch(() => null);
      if (b?.data && v.delivery) {
        const data = Buffer.from(b.data, "base64");
        let j: any = null;
        try { j = JSON.parse(data.toString("utf8")); } catch { /* not JSON */ }
        (v.delivery as any).text = typeof j?.text === "string" ? j.text.slice(0, 2000) : null;
        (v.delivery as any).hash_ok = hex(blake3(data)) === x.result_hash;
      }
      return v;
    }));
    // spend today: the hosted runtime's budget counters (UTC day), summed over hosted members
    let cadre = 0n, sol = 0n, rows = 0, counted = 0, day: string | null = null;
    for (const m of h?.members ?? []) {
      const bt = m.hosted?.budget_today;
      if (!bt) continue;
      counted++;
      day = bt.day ?? day;
      cadre += BigInt(bt.cadre_units ?? 0);
      sol += BigInt(bt.topupLamports ?? bt.sol_lamports ?? 0);
      rows += Number(bt.rows ?? 0);
    }
    const internalPaid = internal.filter((x: any) => x.status === "completed").reduce((a: bigint, x: any) => a + BigInt(x.price), 0n);
    // trading swarms: each member's practice line on the strategies contract
    let strategies: any = null;
    if (base.kind === "trading") {
      if (shared.strategies) {
        try {
          const rows2 = ((await shared.strategies()) ?? []) as any[];
          const mine = rows2.filter((r) => everyone.has(r.author));
          strategies = {
            members: [...everyone].map((a) => ({ agent: a, handle: hostedOf.get(a)?.handle ?? card(a).handle, style: hostedOf.get(a)?.style ?? null, lines: mine.filter((r) => r.author === a) })),
            compare: mine.map((r) => ({ agent: r.author, style: hostedOf.get(r.author)?.style ?? null, head_version: r.head_version, versions: r.versions, trades: r.trades, wins: r.wins, practice_pnl_lamports: r.practice_pnl_lamports, learned_fixes: r.learned_fixes, detail: `/v1/strategies/versions/${r.head_version}` })),
            source: "strategies contract (the same rows as GET /v1/strategies); practice P&L is pretend money at real prices",
          };
        } catch (e) { strategies = { error: (e as Error).message.slice(0, 160) }; }
      } else strategies = { error: "no strategies contract is configured on this cadre-api" };
    }
    return {
      ...base, contract: c.contract,
      registration: h?.registration ?? null,
      kickoff: h?.kickoff ?? null,
      members: slots.map((x) => {
        const hm = hostedOf.get(x.agent);
        const cd = card(x.agent);
        return {
          agent: x.agent, handle: hm?.handle ?? cd.handle, name: cd.name, role: x.role, is_lead: x.agent === s.lead, active: x.left_height === null, joined_height: x.joined_height, left_height: x.left_height, via_proposal: x.via_proposal,
          hosted: hm?.hosted ?? null, practice_style: hm?.style ?? null, practice: hm?.practice ?? null, offers: offersBy.get(x.agent) ?? [],
        };
      }),
      // standing: only an open proposal of the swarm's current lead is an invitation (proposalStanding)
      proposals_list: proposals.map(({ swarm: _s, ...p }) => ({ ...p, standing: proposalStanding({ ...p, swarm: id }, s) })),
      spend: {
        source: h ? "hosted runtime budget counters (UTC day) of the hosted members, plus completed hires between members on the market contract" : "only the market contract (this swarm is not hosted here, so no runtime counters)",
        day, members_counted: counted, cadre_units: cadre.toString(), sol_lamports: sol.toString(), rows, internal_hire_units: internalPaid.toString(),
      },
      hires: market ? { internal: internal.slice(0, 50).map(hireView), internal_count: internal.length, internal_paid_units: internalPaid.toString(), outside_count: hires.length - internal.length } : { error: marketWhy },
      outputs,
      strategies,
      sources: { swarm: "swarms contract", members: "swarms contract (members query)", hires: "market contract (services market index)", spend: "hosted runtime", goal: base.goal.source },
    };
  }, { cache: 5 });

  // ---------------------------------------------------------------- unsigned calls

  async function buildCall(from: string, c: SwarmsConfig, value: bigint, args: Uint8Array) {
    const [acct, params, info] = await Promise.all([d.chain<any>("getAccount", { account: from }), d.chain<any>("getParams", {}), d.chain<any>("getChainInfo", {})]);
    const sim = await d.chain<any>("simulateCall", { from, address: c.contract, args: hex(args), value: Number(value) }).catch((e) => ({ ok: false, error: Buffer.from(String((e as Error).message)).toString("hex") }));
    if (!sim?.ok) throw new ApiError(409, "refused", `the swarms contract would refuse this call now: ${sim?.error ? Buffer.from(sim.error, "hex").toString("utf8") : "simulation failed"}`);
    const gasLimit = BigInt(Math.ceil(sim.gas_used * 1.25)) + 20_000n;
    const body = encodeCallBody({ contract: c.contract, value, gasLimit, args });
    const txLen = TX_HDR + body.length + TX_SIG;
    const floor = contractFeeFloor(params, txLen, gasLimit);
    if (BigInt(acct?.balance ?? 0) < floor.fee + value) throw new ApiError(422, "insufficient_funds", `balance ${acct?.balance ?? 0} < fee ${floor.fee}${value ? ` + value ${value}` : ""} (chain units; the sender must hold CADRE on the Cadre chain)`);
    const signing = contractSigningBytes(info.chain_id, from, acct.nonce, floor.fee, "Call", body);
    return {
      window_pearl: { method: "signAndSendTransaction", params: { transaction: { kind: "Call", contract: c.contract, value: value.toString(), gasLimit: gasLimit.toString(), args: hex(args), fee: floor.fee.toString() } } },
      chain_id: info.chain_id, nonce: acct.nonce, fee: floor.fee.toString(), fee_floor: floor.fee.toString(), fee_rule: FEE_RULE, simulated_gas: sim.gas_used,
      unsigned_tx: hex(signing), framed_message: hex(framedMessage(signing)), tx_len: txLen,
      sign_with: "window.cadre.signAndSendTransaction({ transaction: window_pearl.params.transaction }), or a Solana wallet's signMessage over framed_message, then POST /v1/tx/chain {tx: base64(unsigned_tx || signature)}",
    };
  }

  api.post("/v1/tx/swarm", async (cx) => {
    const c = need();
    const b = await cx.json();
    const from = addr(b.from, "from");
    const n = (k: string) => { const v = b[k]; if (!Number.isSafeInteger(Number(v)) || Number(v) < 1) throw badRequest(`${k} is a positive integer`); return Number(v); };
    let args: Uint8Array;
    let value = 0n;
    let plain: string;
    switch (b.action) {
      case "fund": {
        const pays = Array.isArray(b.payments) ? b.payments : [];
        if (pays.length < 1 || pays.length > 32) throw badRequest("payments is 1..32 of {agent, amount}");
        const list = pays.map((p: any) => ({ agent: addr(p?.agent, "payments[].agent"), amount: amount(p?.amount, "payments[].amount") }));
        value = list.reduce((a: bigint, p: any) => a + p.amount, 0n);
        args = callArgs.fund(n("swarm"), list);
        plain = `Pay ${list.length} member(s) of swarm #${b.swarm} directly, ${value} chain units in all; the contract keeps nothing`;
        break;
      }
      case "leave": args = callArgs.leave(n("swarm")); plain = `Leave swarm #${b.swarm}`; break;
      case "propose": {
        const role = String(b.role ?? "");
        if (!role || Buffer.byteLength(role) > 32) throw badRequest("role is 1..32 bytes");
        args = callArgs.propose(n("swarm"), addr(b.agent, "agent"), role);
        plain = `As the lead of swarm #${b.swarm}, invite ${b.agent} as ${role} (it joins only if it accepts)`;
        break;
      }
      case "cancel": args = callArgs.cancel(n("proposal")); plain = `Withdraw proposal #${b.proposal}`; break;
      case "accept": {
        // the deployed contract still accepts a proposal whose lead has left or handed the lead on
        // (audit follow-up 2026-10-23); this API builds no such accept, the agent can decline it
        const pid = n("proposal");
        const p = decodeProposal(pid, await query(c, queryArgs.proposal(pid), "no such proposal"));
        const st = proposalStanding(p, await swarmRec(c, p.swarm));
        if (st !== "valid") {
          throw new ApiError(409, "stale_proposal", st === "closed" ? `proposal #${pid} is ${p.state}` : `proposal #${pid} was made by a lead swarm #${p.swarm} ${st === "no_lead" ? "no longer has (the lead left; the swarm has no lead)" : "no longer has (another member leads it now)"}: it is not an invitation any more; decline it to close it`, { standing: st });
        }
        args = callArgs.accept(pid);
        plain = `Accept proposal #${b.proposal} and join the swarm`;
        break;
      }
      case "decline": args = callArgs.decline(n("proposal")); plain = `Decline proposal #${b.proposal}`; break;
      case "hand_lead": args = callArgs.handLead(n("swarm"), addr(b.to, "to")); plain = `Name ${b.to} to take the lead of swarm #${b.swarm}`; break;
      case "take_lead": args = callArgs.takeLead(n("swarm")); plain = `Take the lead of swarm #${b.swarm}`; break;
      default: throw badRequest("action is fund, leave, propose, cancel, accept, decline, hand_lead or take_lead (the creator has no action after create)");
    }
    return { ...(await buildCall(from, c, value, args)), plain, summary: { from, action: b.action, value: value.toString() } };
  });

  // ---------------------------------------------------------------- hosted runtime pass-through

  async function forward(cx: Ctx, body?: Buffer) {
    const base = runtimeBase();
    if (!base) throw new ApiError(501, "upstream_not_configured", "the hosted agent runtime is not configured on this deployment (CADRE_AGENTS_URL)");
    const headers: Record<string, string> = {};
    for (const k of ["content-type", "x-cadre-wallet", "x-cadre-timestamp", "x-cadre-signature"]) if (typeof cx.headers[k] === "string") headers[k] = cx.headers[k] as string;
    let r: Response;
    try {
      r = await fetch(base.replace(/\/$/, "") + cx.path + (cx.query.toString() ? `?${cx.query}` : ""), { method: cx.method, headers, body: body && body.length ? body : undefined, signal: AbortSignal.timeout(60_000) });
    } catch (e) {
      throw new ApiError(503, "agents_unavailable", `hosted agent runtime unreachable: ${(e as Error).message.slice(0, 120)}`);
    }
    const text = await r.text();
    let j: any;
    try { j = JSON.parse(text); } catch { j = { raw: text.slice(0, 500) }; }
    if (r.status >= 400) throw new ApiError(r.status, j?.error?.code ?? "agents_error", j?.error?.message ?? `runtime answered ${r.status}`);
    return { status: r.status, body: j };
  }

  api.get("/v1/hosted/swarms", async (cx) => reply({ body: (await forward(cx)).body, cache: "no-store" }), { cache: "no-store" });
  api.get("/v1/hosted/swarms/styles", async (cx) => reply({ body: (await forward(cx)).body, cache: "no-store" }), { cache: "no-store" });

  // the plan: the runtime's reply plus ONE unsigned Solana tx that sends every member its SOL
  api.post("/v1/hosted/swarms/plan", async (cx) => {
    const raw = await cx.raw();
    const out = (await forward(cx, raw)).body;
    const creator: string = (() => { try { return JSON.parse(raw.toString("utf8")).creator; } catch { return ""; } })();
    let solana: any = null;
    let solanaError: string | null = null;
    try {
      const pays = (out?.members ?? []).filter((m: any) => DEC.test(m.sol_lamports ?? "") && BigInt(m.sol_lamports) > 0n);
      const total = pays.reduce((a: bigint, m: any) => a + BigInt(m.sol_lamports), 0n);
      const bal = await d.solana<any>("getBalance", [creator, { commitment: "confirmed" }]);
      if (BigInt(bal?.value ?? 0) < total + 5000n) throw new Error(`${creator} holds ${bal?.value ?? 0} lamports; the members' SOL needs ${total} plus the 5000-lamport fee`);
      const ixs: Ix[] = pays.map((m: any) => systemTransfer(creator, m.agent, BigInt(m.sol_lamports)));
      const bh = (await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }])).value;
      const msg = compileLegacy(ixs, creator, bh.blockhash);
      const tx = unsignedTx(msg);
      if (tx.length > 1232) throw new Error(`the members' SOL transfers do not fit one Solana tx (${tx.length} bytes, the limit is 1232)`);
      solana = {
        transaction: Buffer.from(tx).toString("base64"), encoding: "base64", version: "legacy", fee_payer: creator, signers: msg.accountKeys.slice(0, msg.numRequiredSignatures),
        recent_blockhash: bh.blockhash, last_valid_block_height: bh.lastValidBlockHeight, tx_bytes: tx.length,
        summary: { from: creator, transfers: pays.map((m: any) => ({ to: m.agent, lamports: m.sol_lamports })), total_lamports: total.toString() },
        sign_with: "Wallet Standard solana:signAndSendTransaction (or sign, then POST /v1/tx/solana)",
      };
    } catch (e) { solanaError = (e as Error).message.slice(0, 300); }
    return reply({
      body: {
        ...out, solana, solana_error: solanaError,
        signatures: [
          { what: "the swarm: one signMessage over creator_message (it commits the definition and every member spec)", then: "POST /v1/hosted/swarms {plan_id, creator_sig}" },
          { what: "Solana: one tx sending every member its SOL to its own wallet", then: "solana:signAndSendTransaction on solana.transaction" },
        ],
      },
      cache: "no-store",
    });
  }, { cost: 10 });

  api.post("/v1/hosted/swarms", async (cx) => {
    const r = await forward(cx, await cx.raw());
    hostedCache.at = 0;
    return reply({ status: r.status, body: r.body, cache: "no-store" });
  }, { cost: 20 });
}
