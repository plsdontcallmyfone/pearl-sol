const {Connection,PublicKey,Keypair,Transaction,TransactionInstruction,SystemProgram,ComputeBudgetProgram,LAMPORTS_PER_SOL}=require('@solana/web3.js');
const {LiteSVM,Clock}=require('./litesvm-web3.cjs');
const BN=require('bn.js');
const sdk=require('@pump-fun/pump-sdk');
const {PUMP_SDK}=sdk;
const SO=process.env.PUMP_SO_DIR||(__dirname+'/so/');
const IDL=require(process.env.PUMP_IDL||(__dirname+'/idl/pump.json'));
const P=s=>new PublicKey(s);
const PUMP=P('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P'),AMM=P('pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA'),FEE=P('pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ'),MAY=P('MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e');
const WSOL=P('So11111111111111111111111111111111111111112'),TOK=P('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA'),T22=P('TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb'),ATA=P('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL');
const pda=(seeds,prog)=>PublicKey.findProgramAddressSync(seeds.map(s=>typeof s==='string'?Buffer.from(s):s.toBuffer()),prog)[0];
const ata=(owner,mint,tp)=>PublicKey.findProgramAddressSync([owner.toBuffer(),tp.toBuffer(),mint.toBuffer()],ATA)[0];
const CLONE={
 global:pda(['global'],PUMP), gva:pda(['global_volume_accumulator'],PUMP), feeCfg:pda(['fee_config',PUMP],FEE),
 ammGlobal:pda(['global_config'],AMM), ammFeeCfg:pda(['fee_config',AMM],FEE), ammGva:pda(['global_volume_accumulator'],AMM),
 feeGlobal:P('CHqnuTkj6sXDFknM652aEFPECZh9qVsBXWkhPohmV9dA'), wsol:P('So11111111111111111111111111111111111111112'), wauth:P('39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg'), mayGP:pda(['global-params'],MAY), maySV:pda(['sol-vault'],MAY),
};
const BUYBACK='5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD 9M4giFFMxmFGXtc3feFzRai56WbBqehoSeRE5GK7gf7 GXPFM2caqTtQYC2cJ5yJRi9VDkpsYZXzYdwYpGnLmtDL 3BpXnfJaUTiwXnJNe7Ej1rcbzqTTQUvLShZaWazebsVR 5cjcW9wExnJJiqgLjq7DEG75Pm6JBgE1hNv4B2vHXUW6 EHAAiTxcdDwQ3U4bU6YcMsQGaekdzLS3B5SmYo46kJtL 5eHhjP8JaYkz83CWwvGU2uMUXefd3AazWGx4gpcuEEYD A7hAgCzFw14fejgCp387JUJRMNyz4j89JKnhtKU8piqW'.split(' ').map(P);
const FEEREC='62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV 7VtfL8fvgNfhz17qKRMjzQEXgbdpnHHHQRh54R9jP2RJ 7hTckgnGnLQR6sdH7YkqFTAA7VwTfYFaZ6EhEsU3saCX 9rPYyANsfQZw3DnDmKE3YCQF5E8oD89UXoHn9JFEhJUz AVmoTthdrX6tKt4nDjco2D775W2YK3sDhxPcMmzUAmTY CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM FWsW1xNtWscwNmKv6wVsU1iTzRN6wmmk3MjxRP5tT7hz G5UZAVbAf46s7cKWoyKu8kYTip9DGTpbLZ2qa9Aq69dP'.split(' ').map(P);
const ONLY=process.env.ONLY?process.env.ONLY.split(','):null; // subset of clone keys to test minimal set
(async()=>{
 const c=new Connection(process.env.RPC,'confirmed');
 const svm=new LiteSVM();
 for(const [n,id] of [['pump',PUMP],['pump_amm',AMM],['pump_fees',FEE],['mayhem',MAY]].filter(([n])=>!process.env.PROGS||process.env.PROGS.split(',').includes(n))) svm.addProgramFromFile(id,SO+n+'.so');
 const keys=[...Object.entries(CLONE).filter(([k])=>!ONLY||ONLY.includes(k)).map(e=>e[1]),...(process.env.NOBB?[]:BUYBACK),...(process.env.NOFR?[]:FEEREC),...(process.env.NOATA?[]:[...BUYBACK,...FEEREC,P('JCRGumoE9Qi5BBgULTgdgTLjSgkCMSbF62ZZfGs84JeU')].map(o=>ata(o,P('So11111111111111111111111111111111111111112'),TOK)))];
 console.log('cloned',keys.length,'accounts');
 const infos=await c.getMultipleAccountsInfo(keys);
 keys.forEach((k,i)=>{const a=infos[i]; if(a) svm.setAccount(k,{lamports:a.lamports,data:a.data,owner:a.owner,executable:false,rentEpoch:0});});
 const now=BigInt(Math.floor(Date.now()/1000));
 const ck=svm.getClock(); svm.setClock(new Clock(ck.slot,ck.epochStartTimestamp,ck.epoch,ck.leaderScheduleEpoch,now));
 const dev=Keypair.generate(), trader=Keypair.generate(), mint=Keypair.generate();
 const reserve=Keypair.generate().publicKey; // stands in for the mining reserve PDA (creator)
 svm.airdrop(dev.publicKey,BigInt(100*LAMPORTS_PER_SOL)); svm.airdrop(trader.publicKey,BigInt(100*LAMPORTS_PER_SOL));
 const send=(label,ixs,signers)=>{const tx=new Transaction();tx.recentBlockhash=svm.latestBlockhash();tx.feePayer=signers[0].publicKey;tx.add(ComputeBudgetProgram.setComputeUnitLimit({units:1_000_000}),...ixs);tx.sign(...signers);const r=svm.sendTransaction(tx);
   if(r.err){console.log('FAIL',label,String(r.err()), '\n  '+r.meta().logs().slice(-12).join('\n  '));process.exit(1);} const m=r; console.log('OK  ',label,'CU',m.computeUnitsConsumed().toString(),'ixs',ixs.length); svm.expireBlockhash(); return m;};
 const getAI=(k)=>{const a=svm.getAccount(k);return a?{lamports:Number(a.lamports),data:Buffer.from(a.data),owner:a.owner,executable:a.executable,rentEpoch:0}:null};
 const global=PUMP_SDK.decodeGlobal(getAI(CLONE.global));
 const feeConfig=PUMP_SDK.decodeFeeConfig(getAI(CLONE.feeCfg));
 // 1. create_v2 + dev buy of 3 SOL
 const devSol=new BN(3*LAMPORTS_PER_SOL);
 const amount=sdk.getBuyTokenAmountFromSolAmount({global,feeConfig,mintSupply:null,bondingCurve:null,amount:devSol,quoteMint:WSOL});
 const createIxs=await PUMP_SDK.createV2AndBuyInstructions({global,mint:mint.publicKey,name:'Pearl Test',symbol:'PRLT',uri:'https://example.com/x.json',creator:reserve,user:dev.publicKey,amount,solAmount:devSol,mayhemMode:false});
 console.log('create+buy program ids:',createIxs.map(i=>i.programId.toBase58().slice(0,6)+'('+i.keys.length+')').join(' '));
 send('create_v2 + buy (dev 3 SOL)',createIxs,[dev,mint]);
 const bcPk=sdk.bondingCurvePda(mint.publicKey);
 { const st=require('@solana/spl-token'); const ma=svm.getAccount(mint.publicKey); const m=st.unpackMint(mint.publicKey,{...ma,data:Buffer.from(ma.data)},T22); console.log('  mint supply',m.supply.toString(),'decimals',m.decimals,'mintAuth',m.mintAuthority&&m.mintAuthority.toBase58(),'freezeAuth',m.freezeAuthority&&m.freezeAuthority.toBase58(),'ext',st.getExtensionTypes(m.tlvData).map(e=>st.ExtensionType[e]).join(','),'len',ma.data.length); }
 let bc=PUMP_SDK.decodeBondingCurve(getAI(bcPk));
 console.log('  dev got tokens',amount.toString(),'curve creator',bc.creator.toBase58()===reserve.toBase58()?'=reserve':'?','realQuote',bc.realQuoteReserves?.toString?.()??bc.realSolReserves?.toString());
 const cv=sdk.creatorVaultPda(reserve); console.log('  creator_vault lamports after buy_v2',svm.getBalance(cv)?.toString());
 // 2. trader buy_v3 2 SOL
 const tAta=ata(trader.publicKey,mint.publicKey,T22);
 const buyIxs=await PUMP_SDK.buyV3Instructions({bondingCurve:bc,associatedUserAccountInfo:null,mint:mint.publicKey,user:trader.publicKey,amount:new BN('50000000000000'),quoteAmount:new BN(5*LAMPORTS_PER_SOL),slippage:5,tokenProgram:T22});
 send('buy_v3 (trader, 50M tokens)',buyIxs,[trader]);
 bc=PUMP_SDK.decodeBondingCurve(getAI(bcPk));
 console.log('  curve creator_fee waiting',bc.creatorFee?.toString(),'protocol_fees',bc.protocolFees?.toString());
 // 3. sell_v3 half
 const sellIxs=await PUMP_SDK.sellV3Instructions({bondingCurve:bc,mint:mint.publicKey,user:trader.publicKey,amount:new BN('25000000000000'),quoteAmount:new BN(0),slippage:100,tokenProgram:T22});
 send('sell_v3 (trader, 25M tokens)',sellIxs,[trader]);
 bc=PUMP_SDK.decodeBondingCurve(getAI(bcPk));
 console.log('  curve creator_fee waiting',bc.creatorFee?.toString());
 // 4. sweep_creator_fee + collect_creator_fee_v2 (permissionless, signer = trader as random cranker)
 const sweep=await PUMP_SDK.sweepCreatorFeeInstruction({payer:trader.publicKey,mint:mint.publicKey,creator:reserve,quoteMint:WSOL});
 const disc=Buffer.from(IDL.instructions.find(i=>i.name==='collect_creator_fee_v2').discriminator);
 const collect=new TransactionInstruction({programId:PUMP,data:disc,keys:[
  {pubkey:reserve,isSigner:false,isWritable:true},{pubkey:ata(reserve,WSOL,TOK),isSigner:false,isWritable:true},
  {pubkey:cv,isSigner:false,isWritable:true},{pubkey:ata(cv,WSOL,TOK),isSigner:false,isWritable:true},
  {pubkey:WSOL,isSigner:false,isWritable:false},{pubkey:TOK,isSigner:false,isWritable:false},{pubkey:ATA,isSigner:false,isWritable:false},
  {pubkey:SystemProgram.programId,isSigner:false,isWritable:false},{pubkey:pda(['__event_authority'],PUMP),isSigner:false,isWritable:false},{pubkey:PUMP,isSigner:false,isWritable:false}]});
 const before=svm.getBalance(reserve)??0n;
 send('sweep_creator_fee + collect_creator_fee_v2 (cranker, creator does not sign)',[sweep,collect],[trader]);
 console.log('  reserve received lamports',(BigInt(svm.getBalance(reserve)??0n)-BigInt(before)).toString(),'vault left',svm.getBalance(cv)?.toString());

 if(!process.env.GRAD) return;
 const swap=require('@pump-fun/pump-swap-sdk');
 const fake={getAccountInfo:async(k)=>getAI(k),getMultipleAccountsInfo:async(ks)=>ks.map(getAI)};
 // 5. buy past the curve: synthetic migration on buy_v3
 bc=PUMP_SDK.decodeBondingCurve(getAI(bcPk));
 const big=bc.realTokenReserves.add(new BN('20000000000000'));
 const gIxs=await PUMP_SDK.buyV3Instructions({bondingCurve:bc,associatedUserAccountInfo:getAI(tAta),mint:mint.publicKey,user:trader.publicKey,amount:big,quoteAmount:new BN(95*LAMPORTS_PER_SOL),slippage:0,tokenProgram:T22});
 send('buy_v3 past curve end (synthetic migration)',gIxs,[trader]);
 bc=PUMP_SDK.decodeBondingCurve(getAI(bcPk)); console.log('  complete',bc.complete,'creator_fee waiting',bc.creatorFee?.toString());
 const mig=await PUMP_SDK.migrateV2Instruction({withdrawAuthority:global.withdrawAuthority,mint:mint.publicKey,user:trader.publicKey,quoteMint:WSOL,baseTokenProgram:T22,quoteTokenProgram:TOK});
 send('migrate_v2 (permissionless, trader signs)',[mig],[trader]);
 const poolKey=sdk.canonicalPumpPoolPda(mint.publicKey);
 const online=new swap.OnlinePumpAmmSdk(fake); const off=new swap.PumpAmmSdk();
 const pool=off.decodePool(getAI(poolKey)); console.log('  pool',poolKey.toBase58().slice(0,8),'coin_creator',pool.coinCreator.toBase58()===reserve.toBase58()?'=reserve':pool.coinCreator.toBase58());
 // 6. AMM buy (legacy-style quote input, fees paid per trade) and v2 buy (fees kept in pool)
 let st=await online.swapSolanaState(poolKey,trader.publicKey);
 send('PumpSwap buy (buyQuoteInput 2 SOL)',await off.buyQuoteInput(st,new BN(2*LAMPORTS_PER_SOL),5),[trader]);
 st=await online.swapSolanaState(poolKey,trader.publicKey);
 send('PumpSwap buy_v2 (1M tokens)',await off.buyV2Instructions(st,new BN('1000000000000'),new BN(5*LAMPORTS_PER_SOL)),[trader]);
 const cva=pda(['creator_vault',reserve],AMM);
 // 7. sweep both, collect both
 const pl=off.decodePool(getAI(poolKey)); console.log('  pool creator_fees waiting',pl.creatorFees?.toString());
 const sw1=await PUMP_SDK.sweepCreatorFeeInstruction({payer:trader.publicKey,mint:mint.publicKey,creator:reserve,quoteMint:WSOL});
 const sw2=await off.sweepCreatorFeeInstruction({payer:trader.publicKey,poolKey,pool:pl,quoteTokenProgram:TOK});
 const createResWsol=require('@solana/spl-token').createAssociatedTokenAccountIdempotentInstruction(trader.publicKey,ata(reserve,WSOL,TOK),reserve,WSOL,TOK);
 const cs=await online.collectCoinCreatorFeeSolanaState(reserve);
 const col2=await off.collectCoinCreatorFee(cs,trader.publicKey);
 const b0=svm.getBalance(reserve); 
 send('sweep curve + sweep pool + collect_creator_fee_v2 + collect_coin_creator_fee (cranker)',[sw1,sw2,createResWsol,collect,...col2],[trader]);
 const wa=svm.getAccount(ata(reserve,WSOL,TOK)); const amt=Buffer.from(wa.data).readBigUInt64LE(64);
 console.log('  reserve SOL delta',(BigInt(svm.getBalance(reserve))-BigInt(b0)).toString(),'reserve wSOL ATA amount',amt.toString(),'col2 ixs',col2.length);
})().catch(e=>{console.error(String(e.stack||e).replace(/api-key=[^&\s]+/g,'api-key=REDACTED'));process.exit(1)});
