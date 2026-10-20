// READ ONLY (getAccountInfo, no transactions): fetch the pump Global + FeeConfig on Solana mainnet and devnet
// and quote dev buys with pump-sdk's own math (services/crank node_modules). docs/MAINNET.md section 6.
// Run: node scripts/mainnet-prep/curve.cjs
const req = require("module").createRequire(require("path").resolve(__dirname, "../../services/crank/package.json"));
const { Connection, PublicKey } = req("@solana/web3.js");
const sdk = req("@pump-fun/pump-sdk");
const BN = req("bn.js");
const { OnlinePumpSdk, newBondingCurve, getBuyTokenAmountFromSolAmount, getBuySolAmountFromTokenAmount, computeFeesBps } = sdk;
const SOL = PublicKey.default;
(async () => {
  for (const [name, url] of [["mainnet", "https://api.mainnet-beta.solana.com"], ["devnet", "https://api.devnet.solana.com"]]) {
    const conn = new Connection(url, "confirmed");
    const on = new OnlinePumpSdk(conn);
    const g = await on.fetchGlobal(); const fc = await on.fetchFeeConfig().catch((e) => { console.log("feeconfig err", String(e).slice(0, 120)); return null; });
    const supply = g.tokenTotalSupply;
    const bc = newBondingCurve(g);
    const f = computeFeesBps({ global: g, feeConfig: fc, mintSupply: supply, virtualQuoteReserves: bc.virtualQuoteReserves ?? bc.virtualSolReserves, virtualTokenReserves: bc.virtualTokenReserves });
    console.log(JSON.stringify({ net: name, slot: await conn.getSlot(), initial_virtual_sol: g.initialVirtualSolReserves.toString(), initial_virtual_token: g.initialVirtualTokenReserves.toString(), initial_real_token: g.initialRealTokenReserves.toString(), total_supply: supply.toString(), pool_migration_fee: (g.poolMigrationFee ?? new BN(0)).toString(), fees_at_launch_bps: { protocol: f.protocolFeeBps.toString(), creator: f.creatorFeeBps.toString() } }));
    for (const sol of [0.3, 1, 2, 3, 5, 10, 20]) {
      const lam = new BN(Math.round(sol * 1e9));
      const t = getBuyTokenAmountFromSolAmount({ global: g, feeConfig: fc, mintSupply: null, bondingCurve: null, amount: lam, quoteMint: SOL });
      console.log(`  ${name} buy ${sol} SOL -> ${(Number(t.toString()) / 1e6).toLocaleString("en-US", { maximumFractionDigits: 0 })} tokens (${(Number(t.toString()) / 1e13).toFixed(2)}% of 1B)`);
    }
    for (const pct of [1, 5, 10, 15, 20]) {
      const tok = new BN(pct).mul(new BN(10).pow(new BN(13)));
      if (tok.gt(g.initialRealTokenReserves)) { console.log(`  ${name} ${pct}%: beyond the curve`); continue; }
      const s = getBuySolAmountFromTokenAmount({ global: g, feeConfig: fc, mintSupply: null, bondingCurve: null, amount: tok, quoteMint: SOL });
      console.log(`  ${name} ${pct}% of supply costs ${(Number(s.toString()) / 1e9).toFixed(4)} SOL incl. fees`);
    }
    const whole = getBuySolAmountFromTokenAmount({ global: g, feeConfig: fc, mintSupply: null, bondingCurve: null, amount: g.initialRealTokenReserves, quoteMint: SOL });
    console.log(`  ${name} whole curve (${(Number(g.initialRealTokenReserves.toString())/1e6).toLocaleString("en-US")} tokens) costs ${(Number(whole.toString()) / 1e9).toFixed(4)} SOL incl. fees`);
  }
})().catch((e) => { console.error(e); process.exit(1); });
