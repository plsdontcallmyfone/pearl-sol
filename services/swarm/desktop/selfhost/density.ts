// Density test for self-hosted house desktops (docs/DESKTOPS-AT-SCALE.md section 5). Starts N
// desktops from selfhost/Containerfile on one box through the self-hosted DesktopHost (the same
// provisioning as production: the Cadre Wallet with a throwaway, never-funded key, the browser host,
// the HLS encoder), lets them sit like production desktops (half Social with the town open, half Work
// with wallet + explorer + market tabs), runs the real vision loop on a few of them, and measures:
//   CPU (cores used by the whole fleet, from the fleet's cgroup slice), memory (slice total, anon),
//   stream frame rate (frames per second of wall time the encoder produced, per desktop),
//   action latency (per vision step: model call, screenshot, mouse/keyboard; whole task).
//
//   node selfhost/density.ts --levels 5,10,15,20 --vision 3 --window 240 --out density.json
//
// Env: DESK_SELFHOST_CMD (podman, or "ssh ... podman" to drive a remote box), DENSITY_HOST_CMD (the
// same ssh prefix without podman, for the slice's cgroup files; empty = local), DESK_SELFHOST_SLICE
// (the slice that caps the fleet, e.g. desk-density.slice with CPUQuota and MemoryMax),
// DESK_SELFHOST_CPUS / DESK_SELFHOST_MEM_MB (per desktop), DESK_WALLET_TGZ, DESK_SITE_URL,
// CADRE_AGENTS_ENV (names the file with OPENROUTER_API_KEY; never printed). `--keep` leaves the
// desktops running at the end; otherwise every desktop this run started is removed.
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createKeyFile } from "../../src/wallet.ts";
import { loadSecrets } from "../src/config.ts";
import { secretB58 } from "../src/controller.ts";
import { Desk, type SandboxLike } from "../src/desk.ts";
import { selfHostApi, selfHostOptsFromEnv } from "../src/selfhost.ts";
import { browseTask, workTask } from "../src/tasks.ts";
import { openRouterChat, runTask, systemPrompt } from "../src/vision.ts";

const argv = process.argv.slice(2);
const opt = (k: string, d: string) => (argv.includes(k) ? argv[argv.indexOf(k) + 1]! : d);
const levels = opt("--levels", "5,10,15,20").split(",").map(Number);
const visionN = Number(opt("--vision", "3"));
const windowS = Number(opt("--window", "240"));
const settleS = Number(opt("--settle", "60"));
const out = opt("--out", "density.json");
const env = process.env;
const site = env.DESK_SITE_URL ?? "https://cadre.64-95-12-116.sslip.io";
const explorer = `${site}/explorer`;
const slice = env.DESK_SELFHOST_SLICE ?? "";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const log = (e: Record<string, unknown>) => console.log(JSON.stringify({ t: new Date().toISOString(), ...e }));

function sh(cmd: string): Promise<string> {
  const prefix = (env.DENSITY_HOST_CMD ?? "").split(/\s+/).filter(Boolean);
  const [bin, ...args] = prefix.length ? [...prefix, cmd] : ["bash", "-c", cmd];
  return new Promise((res) => {
    const p = spawn(bin!, args);
    let o = "";
    p.stdout.on("data", (b) => (o += b));
    p.on("close", () => res(o));
  });
}

async function sliceStat() {
  if (!slice) return null;
  // systemd nests a dashed slice name: desk-density.slice lives at desk.slice/desk-density.slice
  const parts = slice.replace(/\.slice$/, "").split("-");
  const d = `/sys/fs/cgroup/${parts.map((_, i) => `${parts.slice(0, i + 1).join("-")}.slice`).join("/")}`;
  const r = await sh(`grep usage_usec ${d}/cpu.stat; cat ${d}/memory.current; grep -E '^(anon|file) ' ${d}/memory.stat; cat /proc/loadavg; head -1 /proc/stat; date +%s%N`);
  const l = r.trim().split("\n");
  const n = (s: string | undefined) => Number(s?.split(/\s+/)[1] ?? NaN);
  const stat = l[5]!.split(/\s+/).slice(1).map(Number);
  return { usage_usec: n(l[0]), mem: Number(l[1]), anon: n(l[2]), file: n(l[3]), load1: Number(l[4]!.split(" ")[0]), box_busy: stat.reduce((a, b) => a + b, 0) - stat[3]! - stat[4]!, box_total: stat.reduce((a, b) => a + b, 0), ns: Number(l[6]) };
}

/** Times every call a vision step makes on the desktop. */
function timed(s: SandboxLike, t: { shot: number[]; act: number[] }): SandboxLike {
  const wrap = <A extends unknown[], R>(f: (...a: A) => Promise<R>, into: number[]) => async (...a: A) => { const t0 = performance.now(); try { return await f(...a); } finally { into.push(performance.now() - t0); } };
  return {
    ...s,
    screenshot: wrap(s.screenshot.bind(s), t.shot),
    leftClick: wrap(s.leftClick.bind(s), t.act), doubleClick: wrap(s.doubleClick.bind(s), t.act), rightClick: wrap(s.rightClick.bind(s), t.act),
    moveMouse: wrap(s.moveMouse.bind(s), t.act), scroll: wrap(s.scroll.bind(s), t.act), write: wrap(s.write.bind(s), t.act), press: wrap(s.press.bind(s), t.act),
  };
}

const stats = (x: number[]) => {
  if (!x.length) return null;
  const s = [...x].sort((a, b) => a - b);
  const q = (p: number) => s[Math.min(s.length - 1, Math.floor(p * s.length))]!;
  return { n: s.length, mean: Math.round(s.reduce((a, b) => a + b, 0) / s.length), p50: Math.round(q(0.5)), p90: Math.round(q(0.9)), max: Math.round(s[s.length - 1]!) };
};

async function main() {
  const sec = loadSecrets();
  if (visionN > 0 && !sec.openrouterKey) throw new Error("no OPENROUTER_API_KEY (CADRE_AGENTS_ENV -> PROVIDERS_ENV_FILE)");
  const chatRaw = sec.openrouterKey ? openRouterChat(sec.openrouterKey) : null;
  const modelMs: number[] = [];
  const chat = chatRaw ? async (b: Record<string, unknown>) => { const t0 = performance.now(); try { return await chatRaw(b); } finally { modelMs.push(performance.now() - t0); } } : null;
  const host = selfHostApi(selfHostOptsFromEnv());
  const keys = mkdtempSync(join(tmpdir(), "desk-density-keys-"));
  const desks: Desk[] = [];
  const results: any[] = [];
  let visionUsd = 0;
  const walletTgz = env.DESK_WALLET_TGZ ?? "wallet.tgz";
  try {
    for (const n of levels) {
      while (desks.length < n) {
        const i = desks.length;
        const handle = `density_${String(i).padStart(2, "0")}`;
        const role = i % 2 === 0 ? "social" : "work";
        const kf = join(keys, `${handle}.json`);
        const w = createKeyFile(kf); // throwaway devnet key, never funded
        const d = new Desk(handle, role, w.address, host, { template: "selfhost", resolution: [1280, 800], keepaliveS: 900, sessionMaxS: 86_400, siteUrl: site, explorerUrl: explorer, walletTgz, log: (e) => log(e) });
        const t0 = Date.now();
        await d.ensure(() => secretB58(kf), () => Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2));
        log({ event: "desk_up", handle, role, s: (Date.now() - t0) / 1000 });
        desks.push(d);
      }
      log({ event: "level", desktops: n, settle_s: settleS });
      await sleep(settleS * 1000);
      const seq0 = await Promise.all(desks.map((d) => d.sbx!.commands.run("grep -m1 MEDIA-SEQUENCE /tmp/stream/live.m3u8 | cut -d: -f2").then((r) => Number(r.stdout.trim()))));
      const s0 = await sliceStat();
      const t0 = Date.now();
      const memSamples: number[] = [];
      const anonSamples: number[] = [];
      const shot: number[] = [], act: number[] = [], taskS: number[] = [], stepS: number[] = [];
      modelMs.length = 0;
      let tasks = 0, ok = 0;
      const deadline = t0 + windowS * 1000;
      const sampler = (async () => { while (Date.now() < deadline) { const s = await sliceStat(); if (s) { memSamples.push(s.mem); anonSamples.push(s.anon); } await sleep(15_000); } })();
      // the same calls the vision loop makes, timed ON the box (no ssh hop): podman exec + ImageMagick
      // screenshot, podman exec + xdotool, on the vision desktops, every 20 s through the window
      const localShot: number[] = [], localAct: number[] = [];
      const names = desks.slice(0, Math.max(1, Math.min(visionN, desks.length))).map((d) => d.id!);
      const podman = (env.DESK_SELFHOST_CMD ?? "podman").split(/\s+/).pop()!;
      const local = (async () => {
        while (Date.now() < deadline - 20_000) {
          for (const nm of names) {
            const r = await sh(`a=$(date +%s%N); ${podman} exec ${nm} import -window root -display :0 png:- >/dev/null; b=$(date +%s%N); ${podman} exec -e DISPLAY=:0 ${nm} xdotool mousemove 640 400; c=$(date +%s%N); echo $(( (b-a)/1000000 )) $(( (c-b)/1000000 ))`);
            const [x, y] = r.trim().split(" ").map(Number);
            if (Number.isFinite(x)) localShot.push(x!);
            if (Number.isFinite(y)) localAct.push(y!);
          }
          await sleep(20_000);
        }
      })();
      const vision = chat ? desks.slice(0, Math.min(visionN, desks.length)).map(async (d, k) => {
        let pick = k;
        while (Date.now() < deadline - 30_000) {
          const t = d.role === "work" ? workTask(site, explorer, d.address, pick++, { walletUrl: null }) : browseTask(site, pick++, [{ handle: "courier_rex" }]);
          const ts = Date.now();
          const r = await runTask(timed(d.sbx!, { shot, act }), chat, { model: env.DESK_VISION_MODEL ?? "anthropic/claude-haiku-5.5", system: systemPrompt("Density Probe", "density_probe", 1280, 800), task: t.text, openUrl: t.url, maxSteps: 10, maxUsd: 0.03 });
          tasks++; if (r.ok) ok++; visionUsd += r.usd;
          taskS.push((Date.now() - ts));
          if (r.steps) stepS.push((Date.now() - ts) / r.steps);
        }
      }) : [];
      await Promise.all([sampler, local, ...vision]);
      const s1 = await sliceStat();
      const wall = (Date.now() - t0) / 1000;
      // frame rate: frames in the newest finished segments / their duration, and how fast segments came
      const fps = await Promise.all(desks.map(async (d, i) => {
        const r = await d.sbx!.commands.run(`cd /tmp/stream && seq=$(grep -m1 MEDIA-SEQUENCE live.m3u8 | cut -d: -f2); for f in $(ls -t s*.ts | sed -n 2,4p); do echo "$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=nw=1:nk=1 $f | head -1) $(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 $f)"; done; echo seq $seq`, { timeoutMs: 30_000 });
        const lines = r.stdout.trim().split("\n");
        const seq = Number(lines.find((l) => l.startsWith("seq"))?.split(" ")[1]);
        let frames = 0, dur = 0;
        for (const l of lines.filter((x) => !x.startsWith("seq"))) { const [a, b] = l.trim().split(/[\s,]+/).map(Number); if (Number.isFinite(a) && Number.isFinite(b)) { frames += a!; dur += b!; } }
        const segFps = dur > 0 ? frames / dur : null;
        const realtime = Number.isFinite(seq) && Number.isFinite(seq0[i]!) ? ((seq - seq0[i]!) * 2) / wall : null;
        return { segFps, realtime, eff: segFps !== null && realtime !== null ? segFps * Math.min(1, realtime) : null };
      }));
      const podStats = JSON.parse((await sh(`${(env.DESK_SELFHOST_CMD ?? "podman").split(/\s+/).pop()} stats --no-stream --format json`)) || "[]");
      const effs = fps.map((f) => f.eff).filter((x): x is number => x !== null);
      const row = {
        desktops: n,
        window_s: Math.round(wall),
        cpu_cores_used: s0 && s1 ? Number(((s1.usage_usec - s0.usage_usec) / ((s1.ns - s0.ns) / 1000)).toFixed(3)) : null,
        cpu_per_desktop: s0 && s1 ? Number(((s1.usage_usec - s0.usage_usec) / ((s1.ns - s0.ns) / 1000) / n).toFixed(3)) : null,
        box_cpu_busy_pct: s0 && s1 ? Number(((100 * (s1.box_busy - s0.box_busy)) / (s1.box_total - s0.box_total)).toFixed(1)) : null,
        load1: s1?.load1 ?? null,
        mem_mib_mean: memSamples.length ? Math.round(memSamples.reduce((a, b) => a + b, 0) / memSamples.length / 2 ** 20) : null,
        mem_mib_max: memSamples.length ? Math.round(Math.max(...memSamples) / 2 ** 20) : null,
        anon_mib_mean: anonSamples.length ? Math.round(anonSamples.reduce((a, b) => a + b, 0) / anonSamples.length / 2 ** 20) : null,
        mem_mib_per_desktop: memSamples.length ? Math.round(Math.max(...memSamples) / 2 ** 20 / n) : null,
        fps_target: Number(env.DESK_FPS ?? 4),
        fps_effective_mean: effs.length ? Number((effs.reduce((a, b) => a + b, 0) / effs.length).toFixed(2)) : null,
        fps_effective_min: effs.length ? Number(Math.min(...effs).toFixed(2)) : null,
        on_box: { screenshot_ms: stats(localShot), action_ms: stats(localAct) },
        vision: { desktops: Math.min(visionN, n), tasks, ok, model_ms: stats(modelMs), screenshot_ms: stats(shot), action_ms: stats(act), step_ms: stats(stepS), task_ms: stats(taskS), note: "screenshot/action/step/task times include the driver's ssh hop to the box; on_box has the same calls without it" },
        per_container: podStats.map((p: any) => ({ name: p.name ?? p.Name, cpu: p.cpu_percent ?? p.CPU, mem: p.mem_usage ?? p.MemUsage })),
      };
      results.push(row);
      log({ event: "measured", ...row, per_container: undefined });
      writeFileSync(out, JSON.stringify({ at: new Date().toISOString(), slice, per_desktop: selfHostOptsFromEnv(), vision_usd: Number(visionUsd.toFixed(4)), results }, null, 1));
    }
  } finally {
    if (!argv.includes("--keep")) for (const d of desks) await d.kill().catch(() => {});
    rmSync(keys, { recursive: true, force: true });
    log({ event: "done", removed: argv.includes("--keep") ? 0 : desks.length, vision_usd: Number(visionUsd.toFixed(4)), out });
  }
}

main().catch((e) => { console.error(JSON.stringify({ event: "fatal", error: String(e?.message ?? e).slice(0, 400) })); process.exit(1); });
