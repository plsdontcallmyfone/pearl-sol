// feed-bot: dry run by default (nothing sent), live only with FEED_LIVE=1 + targets + credentials,
// first run takes no backlog, hourly cap holds events for later, the cursor survives a restart,
// texts fit X's 280 characters and carry the proof link. No network: fetch is faked.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FeedBot, blockers, configFromEnv, format, type FeedEvent } from "../src/bot.ts";

const ev = (i: number, over: Partial<FeedEvent> = {}): FeedEvent => ({ id: `e${i}`, type: "new_agent", at: 1000 + i, title: `New agent @a${i} joined Cadre`, house: false, links: { explorer: `https://explorer.test/tx/${i}` }, ...over });

function fakeApi(list: () => FeedEvent[]) {
  const calls: string[] = [];
  const f = (async (u: any) => {
    calls.push(String(u));
    return new Response(JSON.stringify({ events: list() }), { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
  return { f, calls };
}

const cfg = (env: Record<string, string>) => configFromEnv({ HOME: mkdtempSync(join(tmpdir(), "feedbot-")), ...env });

test("dry run is the default: no target, no credentials, nothing posted", async () => {
  const c = cfg({ FEED_BACKFILL: "2" });
  assert.deepEqual(blockers(c).length > 0, true);
  let events = [ev(3), ev(2), ev(1)];
  const api = fakeApi(() => events);
  const sent: string[] = [];
  const logs: string[] = [];
  const bot = new FeedBot(c, { fetchFn: api.f, post: async (_t, x) => { sent.push(x); }, log: (m) => logs.push(m) });
  const r = await bot.tick();
  assert.equal(r.dry, 2, "first run backfills only FEED_BACKFILL");
  assert.equal(sent.length, 0);
  assert.ok(logs.every((l) => l.startsWith("[dry-run]")));
  events = [ev(4), ...events];
  const r2 = await bot.tick();
  assert.equal(r2.new, 1);
  assert.equal(sent.length, 0);
});

test("live needs all three: FEED_LIVE=1, a target and its credentials", async () => {
  assert.ok(blockers(cfg({ FEED_LIVE: "1" })).includes("FEED_TARGETS is empty"));
  assert.ok(blockers(cfg({ FEED_LIVE: "1", FEED_TARGETS: "telegram" })).some((b) => b.startsWith("telegram needs")));
  assert.ok(blockers(cfg({ FEED_TARGETS: "telegram", FEED_TELEGRAM_TOKEN: "t", FEED_TELEGRAM_CHAT: "c" })).includes("FEED_LIVE is not 1 (dry run)"));
  assert.deepEqual(blockers(cfg({ FEED_LIVE: "1", FEED_TARGETS: "telegram,x", FEED_TELEGRAM_TOKEN: "t", FEED_TELEGRAM_CHAT: "c", FEED_X_TOKEN: "x" })), []);
  assert.throws(() => cfg({ FEED_TARGETS: "discord" }), /unknown target/);
});

test("live with fake endpoints: posts each new event once, oldest first, under the hourly cap; restart keeps the cursor", async () => {
  const env = { FEED_LIVE: "1", FEED_TARGETS: "telegram,x", FEED_TELEGRAM_TOKEN: "tok", FEED_TELEGRAM_CHAT: "@chan", FEED_X_TOKEN: "xt", FEED_MAX_PER_HOUR: "2", FEED_TELEGRAM_API: "https://tg.test", FEED_X_API: "https://x.test" };
  const c = cfg(env);
  let events: FeedEvent[] = [ev(1)];
  const posts: { url: string; body: any; auth: string | null }[] = [];
  const f = (async (u: any, init?: any) => {
    const url = String(u);
    if (url.includes("/v1/events")) return new Response(JSON.stringify({ events }), { status: 200 });
    posts.push({ url, body: JSON.parse(init.body), auth: init.headers?.authorization ?? null });
    return new Response("{}", { status: 200 });
  }) as typeof fetch;
  const bot = new FeedBot(c, { fetchFn: f, log: () => {} });
  assert.equal((await bot.tick(10_000)).new, 0, "first run: no backlog");
  events = [ev(4), ev(3), ev(2), ...events];
  const r = await bot.tick(20_000);
  assert.equal(r.posted, 2);
  assert.equal(r.held, 1);
  assert.equal(posts.length, 4, "2 events x 2 targets");
  assert.equal(posts[0].url, "https://tg.test/bottok/sendMessage");
  assert.equal(posts[0].body.chat_id, "@chan");
  assert.match(posts[0].body.text, /@a2 joined Cadre\nhttps:\/\/explorer\.test\/tx\/2/);
  assert.equal(posts[1].url, "https://x.test/2/tweets");
  assert.equal(posts[1].auth, "Bearer xt");
  const again = new FeedBot(configFromEnv({ ...env, HOME: "" , FEED_STATE: c.stateFile }), { fetchFn: f, log: () => {} });
  const r2 = await again.tick(20_000 + 3_600_001);
  assert.equal(r2.posted, 1, "the held event goes out after the hour, the posted ones never again");
  assert.match(posts[posts.length - 1].body.text, /@a4/);
  assert.ok(!readFileSync(c.stateFile, "utf8").includes("tok"), "credentials never reach the state file");
});

test("format: 280 characters with the link kept, house marked", () => {
  const long = ev(1, { title: "x".repeat(400), house: true });
  const t = format(long, { siteUrl: null });
  assert.ok(t.length <= 280);
  assert.ok(t.endsWith("https://explorer.test/tx/1"));
  assert.match(format(ev(2, { house: true }), { siteUrl: null }), /\[house\]/);
  assert.equal(format(ev(3, { links: { api: "/v1/market/hires/3" } }), { siteUrl: "https://api.test" }).split("\n")[1], "https://api.test/v1/market/hires/3");
});
