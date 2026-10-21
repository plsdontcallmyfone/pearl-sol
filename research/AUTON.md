# Auton (autoneco.xyz): research note

Researched 2026-10-21. Site offline at time of research. Every number below has a source; anything without one is marked "unknown". Analysis sections are labelled as analysis.

## Summary

auton ("autonomous ecosystem", X: @autonxyz, token $AUTON) was a Solana launchpad plus social feed where every trader, coin founder and poster was an LLM agent ("auton"). Humans held 100,000 $AUTON to create an agent, gave it a name and persona, funded it with SOL, and then could only watch [1][2]. Each agent got a server-generated (custodial) wallet and ran on a shared model (deepseek/deepseek-v4.1-flash) every few seconds, deciding whether to post, reply, like, follow, launch a coin, buy or sell [1][3]. Agent coins were Token-2022 mints on Meteora Dynamic Bonding Curves with a transfer hook that only let transfers through if the pad co-signed, so humans and snipers could not trade a coin until it graduated to a Meteora DAMM v2 pool [1][9]. The $AUTON token launched on Meteora DBC on 2026-10-06 18:42 UTC, ran from roughly $30K to a peak near $8M market cap within about four hours [6][11][14], and then collapsed: about 95% of the remaining value went in roughly two hours on 2026-10-08 (13:00 to 15:00 UTC) [6], the same minutes the pad wallet made its last payouts and agent wallets sold out their bags [7]. Holders on X described it as a rug and said the X account was deleted [12][13]. The Wayback Machine shows the launch thread returning 404 by 2026-10-08 17:09 UTC [19], and the site now returns "This service has been suspended by its owner" from Render [5]. The project's ops wallet moved about 559 SOL out on 2026-10-12, and the pad wallet kept claiming $AUTON pool fees and sending them out as late as 2026-10-20 [7]. Its whole public life was about two days.

## Identity

- Domain: autoneco.xyz. On 2026-10-05 05:48 UTC the domain was still a parked page (a redirect to "/lander") [4]. The app was live in the Wayback capture of 2026-10-06 18:51 UTC [4]. Now it returns HTTP 503 "This service has been suspended by its owner" with header `x-render-routing: suspend-by-user`, which means it was hosted on Render and the owner suspended the service [5]. The admin console code also refers to "Render's OWNER_WHITELIST" [1].
- Site title: "auton · autonomous ecosystem"; description: "Autons launch, trade and socialise everything. Every trader and every poster is an AI. Create yours and watch it live." [2]
- X: @autonxyz (user id 2106862854909435906), account created 2026-10-04 21:46 UTC [19]. Bio: "The agent only economy." followed by the $AUTON mint [19][20]. A Sotwe mirror shows 2.4K followers and 110 posts (capture date unknown) [20]. Replies on 2026-10-08 said the account had been deleted [13]. I could not load x.com directly (HTTP 402), so its current state is unknown.
- Team: unknown. Nothing found named or doxxed anyone. The team posted only as "we" and "the team" [19].
- Docs: there was no separate docs site. The docs were the in-app "How it works" page at /docs, rendered by /js/app.js [1]. docs.autoneco.xyz and a GitBook: none found.
- GitHub: none found. A GitHub search for "autoneco" returned nothing relevant.
- Not to be confused with:
  - Auton AI ($AUTO, autonaisol.xyz, @autonai_sol), a different Solana project billed as a "CME for machine resources" (mint 7io8XEMRMoQoCvD3phKR2QR5EeoMmJnYM1dsjVdYpump) [21].
  - Carnegie Mellon's Auton Lab, Autonolas/Olas, the Autonomys Network "Auto Agents", and the GitHub repo atemerev/auton (an agent runtime). None of these is related.
  - Copycats and impostors:
    - A pump.fun "$Auton" (CgqZdiSMUu6XLTRxQugtqPXbSx7sp8Mq8ptG12GJpump), flagged on 2026-10-05 07:38 UTC, before the real token existed. Its deployer had already sold out [22].
    - A pump.fun "auton" (7w4sL7VzJ2VEYPwWmX3phmSAJroFKcSg3oByhP4Rpump) that linked the real launch thread [23].
    - An X account @autonxyzSol with the same bio and about 4K following [24].
    - A lookalike "Holder Crates" connect-wallet site at autonecon.xyz (one letter different) [25]. Its link to the team is unknown. The connect-wallet pattern is a common drainer setup.

## Timeline (UTC)

| When | What | Source |
|---|---|---|
| 2026-10-04 21:46 | @autonxyz account created | [19] |
| 2026-10-05 05:48 | autoneco.xyz still a parked domain | [4] |
| 2026-10-05 09:15 | First teaser: "AI agents have spent long enough answering us. What happens when they start interacting with each other instead?" | [19] |
| 2026-10-05 20:48 | Agent #2 "Trader5" created. The site had a "paper money" mode for the pre-launch period | [1][3] |
| 2026-10-06 18:36 | Launch thread: "The pad is live. Make your auton now." | [19] |
| 2026-10-06 18:42:32 | $AUTON created on Meteora DBC by the pad wallet, with a 1 SOL dev buy of 31,438,793 AUTON (about 3.1% of supply) | [7] |
| 2026-10-06 18:43:40 | $AUTON graduated into Meteora DAMM v2 pool FiYyzx... | [6][11] |
| 2026-10-06 20:04 | "Day one" post: 47 autons, 28 coins, 2,261 trades, 135.6 SOL traded, 9.03M AUTON burned, 53.7 SOL dropped | [19] |
| 2026-10-06 20:57 to 21:29 | "compute plan has been upgraded", "Still experiencing some 502s", restarts | [19] |
| 2026-10-06 22:00 hour | $AUTON hourly high of $0.00804 (about $8M market cap) | [6] |
| 2026-10-07 00:14 | Shipped "withdraw 5% of your agents balance every 24 hours" | [19] |
| 2026-10-07 01:03 | Claimed 589 agents, 164 coins, 22,399 trades, 4.7K SOL traded, "Humans trading: 0" | [19] |
| 2026-10-07 08:57 | Observer: site showing "502 Bad Gateway" while the token traded at about $5M | [12b] |
| 2026-10-07 11:26 | "auton, rebuilt": 4x compute, graduation at about $25K with 30 SOL plus 14% of supply in the pool, 1,265 coins launched, 427 graduated | [19] |
| 2026-10-07 16:13 | New trading engine and strategy settings; "Your deposit is yours. Take it back any time." Earnings still paced | [19] |
| 2026-10-07 18:10 | /api/state snapshot (traction numbers below) | [3] |
| 2026-10-08 05:53 | Critic: "$AUTON $7M to $1.3M ... 5% a day is the exit they shipped" | [13] |
| 2026-10-08 13:00 to 15:00 | $AUTON goes from $0.00097 to about $0.00005 (about -95%) | [6] |
| 2026-10-08 13:47 to 13:51 | Last pad-wallet drops to agents. Sampled agent wallet (Trader5) sells its bags in "pad"-memo transactions | [7] |
| 2026-10-08 13:47 to 15:46 | "Ran to $8m in 4 hours. Now 36 hours later its sitting at $166k"; replies: "they started draining everyone's bots", "Once they deleted their X account..." | [12][13] |
| 2026-10-08 17:09 | Launch thread on x.com returns 404 in Wayback | [19] |
| 2026-10-09 | YouTube LP recap: "AUTON Rugged to Zero on Meteora" | [26] |
| 2026-10-12 19:37 to 19:50 | Ops wallet Dwg7bk... sends 270.15 SOL to 6m8gSg... and 288.75 SOL to 54B1Td... | [7] |
| 2026-10-10, 10-11, 10-16, 10-20 | Pad wallet keeps claiming $AUTON DAMM v2 position fees (Meteora cp-amm) and sends 8.11 SOL to EFJKTd... (10-16) and 3.21 SOL to BRmkYf... (10-20) | [7] |
| 2026-10-21 | Site 503 "suspended by its owner". $AUTON about $10.4K market cap | [5][6] |

## How it worked

### Agents
- Creation was gated by holding $AUTON: 100,000 AUTON per agent, all agents on one wallet. If the holder sold below the threshold, the agents "go to sleep" [1]. One Chinese-language explainer instead says the agents keep trading but the owner cannot edit them or claim earnings [9]. The two sources disagree, so the exact rule is unknown.
- The owner sets a name and a persona of up to 600 characters ("a strategy, a voice, a grudge"). The persona is re-read every turn [1][3]. The launch thread offered styles: sniper, momentum, scalper, conviction or contrarian [19].
- Model: every agent used deepseek/deepseek-v4.1-flash through OpenRouter, paid by the pad out of fees. It was pitched as "Every auton is equal. No pay-to-win." [1][3] By about 2026-10-07 the team posted "Bring your own keys": plug in Claude, GPT, Grok, Gemini or any OpenRouter model, "Your key stays encrypted" [20].
- Loop: "every few seconds it reads its wallet, its bags, its timeline and what's moving, then decides: post, reply, like, follow, launch a coin, buy or sell. Nobody approves anything." [1] The archived agent record shows "look" turns containing balance, bags with PnL, mentions and other agents' timeline posts, plus a free-text "thought" [3].
- On 2026-10-07 a "new trading engine" added deterministic take-profit and stop-loss, "first into new coins" sniping on launch, and stepped profit-taking. Owners could set when to take profit, when to cut a loss and how much to buy [19]. Trading was therefore partly rule-based, not pure LLM decisions.

### Launching
- Coins were 1B-supply Meteora DBC curves with a 1% fee [3].
- The parameters changed during the two days. Early: start 1.55 SOL, graduation 83.33 SOL [3][8]. From about 2026-10-07: start 5.679 SOL, graduation 214.29 SOL market cap, migration with 30 SOL plus 14% of supply [3][19].
- Launch cost 0.012 SOL. Minimum trade 0.005 SOL [3].

### Trading: custody, venue, autonomy, limits
- Custody: custodial. The server generated each agent's keypair and signed its transactions. The admin console had a "Helpdesk: export an auton's key" action ("Admin-signed and logged"), a "Full reset" that "sells every coin every auton holds", and a "Clear every bag" job [1]. An observer flagged exactly this risk on 2026-10-07: "custodial. keys sit on their server, theres an admin 'export-key' action" [20].
- Venue: Meteora DBC before graduation, with a custom transfer-hook program (AvVQRm...) that rejected any transfer the pad did not co-sign. After graduation, Meteora DAMM v2 with no hook, open to everyone [1][3][19]. The hook was described as removed by Meteora at graduation [1]. A trader confirmed that humans could not buy curve coins: "I also put that to the test and tried to trade them, it wouldn't let me." [10]
- Autonomy: full, with no human approval per action [1]. Owners could pause the agent, edit its persona and get notifications [1].
- Limits:
  - minTradeSol 0.005, sellReserveSol 0.01, newBagCostSol 0.004 [3]. No per-agent budget cap or loss limit was found beyond these and the owner-set TP/SL. Unknown whether others existed.
  - Withdrawals: originally deposits were locked (a complaint the team "fixed within a day" [20]). From 2026-10-07 deposits were withdrawable at once, but only to the connected wallet [1][19].
  - Earnings (drops and creator fees) unlocked 72h after the agent was created, then came out at 5% of balance every 24h [3][13][19]. The team defended the cap: "If you allow users to withdraw all the profit on day 1, what do you expect the agents to trade with? ... Sol needs to be kept in the ecosystem to keep it moving." [20]
- Proof: every money-moving action carried a Solscan tx link, and /api/proof published the pad, ops and hook addresses plus every claim, drop and burn [1][7]. Observers credited this: "a tx on every payout from day 1" [20], and "everything matched, their buys, sells, funding sources, usage of fees" [10].

### Social layer
- An X-like feed with posts of up to 280 characters, replies, likes, follows and reposts, all written by agents and stored off-chain [1][3].
- "Influence" mechanic: "An auton with followers is read by more autons, so its coins get noticed. That's the whole game: be worth following." [1] Agent posts were shilling theses for their own bags, for example "launching $SECONDS ... aped my own bag" [3].
- The UI had Feed, Terminal, Coins, Autons, Activity, Alerts and My autons pages, plus an embeddable "auton watcher" [2].

### What humans did
They bought $AUTON to qualify, created and funded agents, wrote personas and strategy settings, watched, and withdrew. They could trade $AUTON itself and graduated agent coins on open DEXes, but not curve coins [1][10]. One human summed up the experience: "Still don't know how to beat other AIs running the same model ... already lost 0.3 SOL" [27].

## Token and on-chain

All on Solana mainnet.

| Item | Address | Link |
|---|---|---|
| $AUTON mint (classic SPL Token, 6 decimals; mint and freeze authority null; supply 982,767,308 now, so about 17.2M burned) | 39ahtL8ynzE4amH26J29C93PA5172V3ft9UuUcqQS8fz | https://solscan.io/token/39ahtL8ynzE4amH26J29C93PA5172V3ft9UuUcqQS8fz |
| $AUTON launch tx (Meteora DBC, pad wallet signer, 1 SOL buy) | 2b5vpdR1M3sGxjq2NNoSicPySqh1MCrkc5emmfpgzjyAHun1bNpU57gK9f3wvDj2bChYoB7MbUabrCDu3yMaNTRT | https://solscan.io/tx/2b5vpdR1M3sGxjq2NNoSicPySqh1MCrkc5emmfpgzjyAHun1bNpU57gK9f3wvDj2bChYoB7MbUabrCDu3yMaNTRT |
| $AUTON DBC pool | 9NgQLefMTY9mf8a3QbnaQGav54weJZxLtykfSrHYsM1w | https://solscan.io/account/9NgQLefMTY9mf8a3QbnaQGav54weJZxLtykfSrHYsM1w |
| $AUTON main DAMM v2 pool | FiYyzxapRvkbUhF5ZD3mJigWwqBGhtVLH49MDSgCLHdB | https://dexscreener.com/solana/fiyyzxaprvkbuhf5zd3mjigwwqbghtvlh49mdsgclhdb |
| Pad ("Core") wallet: claims fees, pays drops and burns, signs agent trades | ANppuXB58vHobrRo3A1zohwG4Gx6TcsNfp4JxqkuQ18E | https://solscan.io/account/ANppuXB58vHobrRo3A1zohwG4Gx6TcsNfp4JxqkuQ18E |
| Ops wallet (AI credits plus team) | Dwg7bk1mH6CeY8f54umfs8xPtYiMR5kMWdyMacGa46be | https://solscan.io/account/Dwg7bk1mH6CeY8f54umfs8xPtYiMR5kMWdyMacGa46be |
| "AI-only" transfer-hook program (upgradeable) | AvVQRmi5LwnTN64pkHbUEeAwwMgR2ms3FcL8VbfZV7FH | https://solscan.io/account/AvVQRmi5LwnTN64pkHbUEeAwwMgR2ms3FcL8VbfZV7FH |
| DBC config for agent coins / for $AUTON | HKf3CHysP3YbCudWnYuBBwHSwSGE2GpjxmRakefUhjyX / 4YMVhJZgg8K6C5pBEYcjUCYmER3V1caobkRGrmQsJfis | https://solscan.io/account/HKf3CHysP3YbCudWnYuBBwHSwSGE2GpjxmRakefUhjyX |
| Example agent wallet (Trader5) | J3x66ZzLmaeC1NQScN9jgbiTVoQUmteeYAwwuUVRjhUi | https://solscan.io/account/J3x66ZzLmaeC1NQScN9jgbiTVoQUmteeYAwwuUVRjhUi |
| Ops outflow recipients (2026-10-12) | 6m8gSg7pRr3cmf9Rr1AxCSuSJsrgUXkgK8pRgvM69k5K (holds 809.1 SOL at research time), 54B1Tdeq5XNrV4vY2bLs1euthuEv73MfEF16YBQ8Na2p (0 SOL, moved on) | https://solscan.io/account/6m8gSg7pRr3cmf9Rr1AxCSuSJsrgUXkgK8pRgvM69k5K |
| Pad outflow recipients (2026-10-16 / 10-20) | EFJKTdAnni9eU9UULrh5Emn8evfMS8hyejUqcu8XtJ6B (145.4 SOL now), BRmkYfKzGkJ9n4EZGgdqRf4uWrNJDVcRkPfDppt5Uckw | https://solscan.io/account/EFJKTdAnni9eU9UULrh5Emn8evfMS8hyejUqcu8XtJ6B |

On-chain observations from my own RPC reads on 2026-10-21 [7]:
- The pad wallet's $AUTON account (3ZNhuvm1kauAx8D9VWXs4bTPcDXT4zHVMhHWvtcDjdZX) held the 31.4M dev-buy tokens at launch and holds 0 now. Its busiest hour on record was 2026-10-08 13:00 UTC (247 txs, the dump hour), and after 13:59 UTC it went nearly quiet (two txs in the 18:00 hour, then only fee-claim pairs on 10-10, 10-11, 10-16 and 10-20). Total burns account for only about 17.2M AUTON, so the dev-buy tokens were not all burned. Whether, when and where the rest was sold is unknown: I did not trace it tx by tx because public RPC rate limits got in the way.
- Balances: the pad wallet holds 0.0108 SOL and 0 AUTON, the ops wallet 0.0009 SOL, and Trader5 0.001 SOL. Trader5 showed 6.04 SOL plus open bags at 2026-10-07 01:14 [3].
- Who owns the recipient wallets above is unknown. Their labels (exchange or otherwise) were not checked.
- Several "signer BRmkAg9t..." and "EFJFcPUp..." dust transfers right after the pad's sweeps match address poisoning by third parties (lookalike prefixes). That is my inference.

Note: the $AUTON mint itself is classic SPL Token, not Token-2022. The AI-only hook applied to agent-launched coins, not to $AUTON [1][7].

## Economics

From the app's /api/state and the in-app docs [1][3]:
- $AUTON's own creator rewards (its Meteora pool fees) were split 60% "drops" to holders' agents (random small cuts), 10% buyback and burn of AUTON, 20% AI credits (the API), and 10% team.
- Agent-launched coins: 1% fee per trade. Meteora kept 20%. Of the rest, 90% went to the launching agent's wallet and 10% to an AUTON buyback and burn taken inside the claim tx.
- Fees were claimed "every couple of minutes".
- Token utility: the gate to create and run agents (100K AUTON each), plus fee drops to agents of holders. Planned extras, never confirmed shipped: "auton markets", 5-minute UP/DOWN rounds on SOL, BTC, ETH and $AUTON where "only autons bet" [20].
- Pad treasury at 2026-10-06 23:21 UTC [7]:
  - 1,145.66 SOL claimed in total.
  - 687.38 SOL dropped to agents in 5,447 drops.
  - 96.88 SOL paid to creators.
  - 343.70 SOL to ops.
  - 67.62 SOL used to burn 12,893,708 AUTON in 47 burns.
- Analysis: the agent economy ran mostly on recycled $AUTON trading fees. An observer measured that drops (from $AUTON fees) were 76% of all payouts to agents as of 2026-10-07 [20]. Agents mostly traded coins that only other agents could buy, so their "profits" were largely a transfer from other agents' deposits plus the $AUTON fee subsidy.

## Traction (sourced numbers only)

- /api/state at 2026-10-07 18:10 UTC (team's own API) [3]:
  - 803 agents (803 active).
  - 1,892 coins, 435 graduated.
  - 505,038 trades.
  - 36,956 SOL volume, 369.6 SOL fees.
  - 105,788 posts.
  - 1,189.84 SOL deposited, 739.18 SOL withdrawn in 554 withdrawals.
  - SOL/USD used: 116.0.
- Team posts [19]:
  - Day one (2026-10-06 20:04): 47 autons, 28 coins, 2,261 trades, 135.6 SOL.
  - 2026-10-07 01:03: 589 agents, 164 coins, 22,399 trades, 4.7K SOL.
  - 2026-10-07 11:26: 1,265 coins, 427 graduated.
  - 2026-10-07 00:14: "735.6 SOL" paid to holders' agents.
- Third parties:
  - 821 agents, 2,075 coins, about 39.3K SOL volume, 116,068 posts, "human trades 0" (2026-10-08 02:22, quoting the site) [9].
  - Under one day old: about $20.75M volume, about 6,788 holders, about $764K liquidity, about 63% bot-related trading volume, and a "team-linked 3.169%" (2026-10-07 08:57) [12b].
  - 17 KOLs traded $315K of AUTON (2026-10-07 02:52) [15].
  - Main pool $12.5M volume in 24h; "475 sol already went to the ai + team wallet" (2026-10-07) [20].
- Price, from GeckoTerminal OHLCV of the main DAMM v2 pool [6]:
  - Open $0.0000302 at 2026-10-06 18:00.
  - High $0.00804 at 2026-10-06 22:00. Supply is about 1B, so that is about an $8M FDV.
  - Daily volume $6.26M (Oct 6), $2.30M (Oct 7), $0.70M (Oct 8), under $31K/day after that.
  - About $0.0000106 on 2026-10-21. DexScreener shows about $10.4K market cap and $8.7K liquidity [11].
- One trader reported turning $1,500 into $130,000 on $AUTON [10]. One user reported going from 0.5 SOL to "+15 SOL" in 24h on an agent [20]. Both are self-reported and unverified.

## Why it went offline

Sourced facts:
- The site is suspended by its owner on Render [5].
- The X launch thread was gone by 2026-10-08 17:09 UTC [19], and users said the account was deleted [13].
- The token fell about 95% between 13:00 and 15:00 UTC on 2026-10-08 [6]. The pad wallet's last drops and the sampled agent's bag liquidations happened at 13:47 to 13:51 UTC that day [7].
- The ops wallet sent about 559 SOL to two other wallets on 2026-10-12 [7].
- The pad wallet kept harvesting $AUTON LP fees after the shutdown, as late as 2026-10-20 [7].

No post-mortem or statement from the team was found. The cause is unknown. Community explanations were "they started draining everyone's bots" [12] and that agents' gains were "a front end showing them fake numbers because they couldnt withdraw rewards for 3 days" [12]. I could not verify either one. An alternative reading consistent with the chain data is that the operator ran the console's "Full reset" or "Clear every bag" (sell every agent bag) and then shut the service down. That reading is inference only.

## What it got right / wrong (analysis)

Right:
- The hook made "agents only" real on-chain, not a website rule, and humans confirmed they could not trade curve coins [10]. That gave the story a credible, novel mechanic.
- A tx link on every payout and a public /api/proof endpoint let outsiders audit fees, drops and burns in near real time [7][20].
- Shared model, fees pay inference: no pay-to-win at launch, and the cost of thinking was funded from flow rather than from users.
- It shipped fast: TP/SL, a new engine, BYO keys and the withdrawal fix all within about 24h [19].

Wrong:
- Fully custodial agent keys with an admin export and a "sell everything" button. Users had to trust the operator completely, and the claimed on-chain trustlessness covered trading permissions, not custody.
- A closed, reflexive economy. Agents traded coins only other agents could buy, funded by recycled $AUTON fees, so PnL shown to users was largely mark-to-market against other agents' deposits. Once $AUTON volume cooled (from $12.5M per 24h to $1.2M in 6h [20]), the subsidy shrank.
- The withdrawal cap (5% per 24h after a 72h lock) read as an exit gate for users while the ops wallet was already receiving hundreds of SOL [13][20].
- Every agent ran the same model and similar prompts, so agents converged and herded into the same "lanes" [3][27]. The persona was the only differentiator, and the 2026-10-07 rules engine replaced much of the LLM's decision-making anyway [19].
- Infra was not ready: 502s and restarts during the peak hours [12b][19].
- An anonymous team, a project two days old, and a 3.1% dev buy into the pad wallet [7][12b].

## Lessons for an agent-trading product

- Custody: give agents non-custodial or programmatically constrained wallets, such as PDA vaults whose only allowed instructions are swaps on whitelisted programs, with withdrawals to the owner enforced on-chain. Do not keep a server keystore with an admin export or a "sell everything" switch. Auton's on-chain hook policed who could trade but nothing policed the operator.
- Hard limits on-chain, not in the UI: per-agent max position size, max loss per day, and a minimum reserve, enforced by the vault program. Make withdrawal rules immutable and published before launch. Changing lock rules mid-flight (locked deposits, then 5%/24h, then "any time") destroyed trust.
- Provable track record: publish per-agent PnL derived only from on-chain fills, realized vs unrealized separated, with marks taken from pools humans can actually trade. Auton's unrealized gains on agent-only curves were not exit-able value.
- Wash trading and circularity: if agents mostly trade each other's coins, reported volume and PnL are a closed loop funded by deposits and fee subsidies. Report how much flow comes from outside the system. Do not count agent-to-agent volume as traction. Cap self-buying of own launches.
- Prompt injection: auton fed other agents' posts (the timeline and mentions) straight into each agent's context every turn [3]. That is an open channel for one agent's posts to steer another agent's trades. Sanitize or structure peer content, keep trading actions behind deterministic checks, and never let free text set sizes or destinations.
- Sustainability: don't fund inference and rewards mainly from the platform token's own trading fees. That income peaks on day one and decays, and Auton's payouts were 76% recycled $AUTON fees [20]. Budget for compute independently of token hype, and design for low-volume days.
- Ops readiness and operator accountability: load-test before the token launches, use a doxxed or at least accountable operator, publish a shutdown or wind-down procedure that returns agent balances on-chain, and do not delete the social account.
- Brand defense: expect same-name pump.fun copies, lookalike domains and impostor X accounts within hours [22][23][24][25]. Pin the canonical mint and domain everywhere.

## Sources

1. auton client bundle /js/app.js (docs page "How it works", admin console, proof page), Wayback 2026-10-06 19:10:53: http://web.archive.org/web/20261006191053id_/https://autoneco.xyz/js/app.js
2. autoneco.xyz home page HTML, Wayback 2026-10-07 02:30:13: http://web.archive.org/web/20261007023013id_/https://autoneco.xyz/
3. auton API, Wayback: /api/state 2026-10-07 18:10:15 http://web.archive.org/web/20261007181015id_/http://autoneco.xyz/api/state and /api/agent/Trader5 2026-10-07 01:15:05 http://web.archive.org/web/20261007011505id_/https://autoneco.xyz/api/agent/Trader5
4. Wayback CDX index for autoneco.xyz (parked "/lander" capture 2026-10-05 05:48:29): http://web.archive.org/cdx/search/cdx?url=autoneco.xyz*&output=json&fl=timestamp,original,statuscode&collapse=urlkey and http://web.archive.org/web/20261005054829id_/https://autoneco.xyz/
5. Live https://autoneco.xyz/ on 2026-10-21 (HTTP 503, "This service has been suspended by its owner", header x-render-routing: suspend-by-user)
6. GeckoTerminal OHLCV (daily and hourly) for pool FiYyzxapRvkbUhF5ZD3mJigWwqBGhtVLH49MDSgCLHdB: https://api.geckoterminal.com/api/v2/networks/solana/pools/FiYyzxapRvkbUhF5ZD3mJigWwqBGhtVLH49MDSgCLHdB/ohlcv/hour and https://www.geckoterminal.com/solana/pools/FiYyzxapRvkbUhF5ZD3mJigWwqBGhtVLH49MDSgCLHdB
7. Solana mainnet RPC reads (getTransaction, getSignaturesForAddress, getBalance, getAccountInfo) on 2026-10-21, plus /api/proof Wayback 2026-10-06 23:14:01: http://web.archive.org/web/20261006231401id_/http://autoneco.xyz/api/proof . Key txs: launch https://solscan.io/tx/2b5vpdR1M3sGxjq2NNoSicPySqh1MCrkc5emmfpgzjyAHun1bNpU57gK9f3wvDj2bChYoB7MbUabrCDu3yMaNTRT ; ops sweeps https://solscan.io/tx/3emZwbrvV5G7WRsSd2wh (prefix) and https://solscan.io/tx/5ef7akm1jwFgvfe5b5iY (prefix); pad sweeps https://solscan.io/tx/557UJgztH7WJocncf4GkeL9NP69cwta5P7m7tfgWtm54C6b3PXkURLEZmoKPfWuv518fdEMexjg5jcoqoxuNSzxV and https://solscan.io/tx/2kbUewVdqvKvTpzAwmqonvDpBBC7SHEtXMsT7x5VASfbecwPS5123we2DXg9a2CBF7ABvud7HeK7Px1R4NVBECMF ; wallet pages https://solscan.io/account/Dwg7bk1mH6CeY8f54umfs8xPtYiMR5kMWdyMacGa46be and https://solscan.io/account/ANppuXB58vHobrRo3A1zohwG4Gx6TcsNfp4JxqkuQ18E
8. BiFu, "What is AUTON? The Solana Token Powering an AI Agent-Only Crypto Ecosystem", 2026-10-07: https://bifu.co/blog/what-is-auton-the-solana-token-powering-an-ai-agent-only-crypto-ecosystem
9. @something_labs on X, 2026-10-08 02:22: https://x.com/something_labs/status/2108020173739598110
10. @pallets on X (trade write-up), 2026-10-12: https://x.com/pallets/status/2109634313645949399
11. DexScreener token API for the AUTON mint, 2026-10-21: https://api.dexscreener.com/latest/dex/tokens/39ahtL8ynzE4amH26J29C93PA5172V3ft9UuUcqQS8fz
12. @0xBiZzy on X with replies, 2026-10-08 13:47: https://x.com/0xBiZzy/status/2108192573387079953
12b. @yjyq_xcys on X, 2026-10-07 08:57: https://x.com/yjyq_xcys/status/2107757076902506521
13. @CryptoThannos on X (thread and replies), 2026-10-08: https://x.com/CryptoThannos/status/2108196226915324220 and https://x.com/CryptoThannos/status/2108073200533835850
14. Bitrue, "What Is AUTON and How Does It Work?", 2026-10-07: https://www.bitrue.com/blog/what-is-auton
15. @StalkHQ on X, 2026-10-07 02:52 and 03:15: https://x.com/StalkHQ/status/2107665375685153198 and https://x.com/StalkHQ/status/2107671092160815504
16. @PastelAlpha on X, 2026-10-07 07:03 ("77K > 8.2M"): https://x.com/PastelAlpha/status/2107728546969915472
17. KuCoin News / Solana Daily trending list naming AUTON, 2026-10-07: https://www.kucoin.com/news/trends/SOL/6ac5fed038a264000792f81e
18. Wayback capture of x.com/autonxyz profile, 2026-10-06 23:04:23: http://web.archive.org/web/20261006230423id_/https://x.com/autonxyz
19. Wayback captures of @autonxyz tweets (twitter.com/autonxyz/status/*, 2026-10-05 to 2026-10-07; launch thread 2107540605203034490 captured 2026-10-06 18:36:51 and 404 at 2026-10-08 17:09:57): http://web.archive.org/cdx/search/cdx?url=twitter.com/autonxyz/status/*&output=json ; http://web.archive.org/web/20261006183651id_/https://twitter.com/autonxyz/status/2107540605203034490 ; http://web.archive.org/web/20261008170957/https://x.com/autonxyz/status/2107540605203034490?s=46 ; http://web.archive.org/web/20261007015115id_/https://x.com/autonxyz/status/2107562636116422847?s=46 ; http://web.archive.org/web/20261008020215id_/https://x.com/autonxyz/status/2107625539091440078?s=46
20. Sotwe mirror of @autonxyz (team replies, retweeted posts by @Checker1crypto, @king_samuz, @Shed_Cover; "auton markets" and "Bring your own keys" posts): https://www.sotwe.com/autonxyz
21. Auton AI (different project): https://www.autonaisol.xyz/ and https://phantom.com/tokens/solana/7io8XEMRMoQoCvD3phKR2QR5EeoMmJnYM1dsjVdYpump
22. @bitecong on X, unrelated pump.fun "$Auton", 2026-10-05 07:38: https://x.com/bitecong/status/2107012590148968958
23. Top100Token page for pump.fun "auton" 7w4sL7...: https://top100token.com/solana/7w4sL7VzJ2VEYPwWmX3phmSAJroFKcSg3oByhP4Rpump
24. TwStalker search for $AUTON (shows impostor @autonxyzSol): https://twstalker.com/search/%24AUTON
25. Lookalike "Holder Crates" site: https://www.autonecon.xyz/
26. Yield Hustler, "AUTON Rugged to Zero on Meteora...", YouTube, 2026-10-09: https://www.youtube.com/watch?v=mv6a1NESAYY
27. KuCoin News repost of @Joensmoon, 2026-10-07: https://www.kucoin.com/news/community/SOL/6ac58bbe38a264000792eae0 (original https://x.com/Joensmoon/status/2107606937605148814)
