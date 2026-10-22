// node src/main.ts            run the bot (dry run unless FEED_LIVE=1 + FEED_TARGETS + credentials)
// node src/main.ts --once     one poll, then exit (what a cron or a check runs)
// node src/main.ts --status   print the config (credentials shown only as set / not set) and exit
import { FeedBot, blockers, configFromEnv } from "./bot.ts";

const c = configFromEnv();
const status = {
  api: c.api, live: c.live, targets: c.targets, types: c.types, organic_only: c.organicOnly, poll_s: c.pollMs / 1000, max_per_hour: c.maxPerHour,
  state: c.stateFile, outbox: c.outbox, telegram: { token: c.telegram.token ? "set" : "not set", chat: c.telegram.chat ? "set" : "not set" }, x: { token: c.x.token ? "set" : "not set" },
  mode: blockers(c).length ? "dry-run" : "LIVE", dry_run_because: blockers(c),
};
if (process.argv.includes("--status")) {
  console.log(JSON.stringify(status, null, 2));
  process.exit(0);
}
console.log(JSON.stringify({ msg: "cadre feed-bot", ...status }));
const bot = new FeedBot(c);
const once = process.argv.includes("--once");
let stop = false;
process.on("SIGTERM", () => (stop = true));
process.on("SIGINT", () => (stop = true));
for (;;) {
  try {
    const r = await bot.tick();
    console.log(JSON.stringify({ msg: "tick", ...r }));
  } catch (e) {
    console.log(JSON.stringify({ msg: "tick failed", error: (e as Error).message.slice(0, 200) }));
  }
  if (once || stop) break;
  await new Promise((r) => setTimeout(r, c.pollMs));
  if (stop) break;
}
