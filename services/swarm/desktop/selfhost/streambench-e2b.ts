// The same stream settings bench as selfhost/streambench.sh, on one E2B desktop of the production size
// (template cadre-desktop, 1 vCPU / 1 GiB): creates a sandbox, installs x11vnc for the VNC
// comparison (inside the sandbox only; nothing is exposed), runs bench-inner.sh, times the
// controller's pull path (files.read of the playlist, a segment and the still, as the controller does
// for a viewer), prints one JSON line per setting, kills the sandbox. Costs a few cents of E2B time.
//   node selfhost/streambench-e2b.ts "4:350:2:2 10:600:1:1 15:800:1:1 24:1200:1:1 30:1500:1:1"
// The E2B key is read from the file CADRE_AGENTS_ENV names (never printed).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { loadSecrets } from "../src/config.ts";
import { sdkApi } from "../src/desk.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const settings = process.argv[2] ?? "4:350:2:2 10:600:1:1 15:800:1:1 24:1200:1:1 30:1500:1:1";
const site = process.argv[3] ?? "https://cadre.64-95-12-116.sslip.io";
const sec = loadSecrets();
if (!sec.e2bKey) throw new Error("no E2B_API_KEY");
const api = await sdkApi(sec.e2bKey);
const t0 = Date.now();
const sbx = await api.create(process.env.DESK_TEMPLATE ?? "cadre-desktop", { timeoutMs: 1_800_000, resolution: [1280, 800], metadata: { cadre_bench: "stream" }, lifecycle: { onTimeout: "kill" } });
const ms = (t: number) => Math.round(performance.now() - t);
try {
  console.log(JSON.stringify({ event: "created", s: (Date.now() - t0) / 1000 }));
  for (const f of ["../sandbox/stream.sh", "latency-probe.sh", "rfbwatch.py", "rfbbytes.py", "bench-inner.sh"]) await sbx.files.write(`/tmp/${f.split("/").pop()}`, readFileSync(join(HERE, f), "utf8"));
  const inst = await sbx.commands.run("((command -v x11vnc && command -v display && command -v python3) || (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq x11vnc imagemagick python3 >/dev/null)) >/dev/null 2>&1; chmod 755 /tmp/*.sh; command -v x11vnc python3 ffprobe convert display xdotool | wc -l", { user: "root", timeoutMs: 300_000 } as any).catch((e: any) => e?.result ?? { stdout: String(e) });
  console.log(JSON.stringify({ event: "tools", found: inst.stdout.trim() }));
  for (const s of settings.split(/\s+/).filter(Boolean)) {
    const r = await sbx.commands.run(`bash /tmp/bench-inner.sh "${s}" ${site}`, { timeoutMs: 600_000 } as any).catch((e: any) => e?.result ?? { stdout: "", stderr: String(e) });
    // the controller's pull path for a viewer, right after the setting ran (encoder still up is not needed: files remain)
    const pull: Record<string, number[]> = { playlist: [], segment: [], still: [] };
    for (let i = 0; i < 5; i++) {
      let t = performance.now();
      const list = Buffer.from(await sbx.files.read("/tmp/stream/live.m3u8", { format: "bytes" })).toString();
      pull.playlist!.push(ms(t));
      const seg = list.split("\n").filter((l) => /^s\d{6}\.ts$/.test(l)).pop();
      if (seg) { t = performance.now(); await sbx.files.read(`/tmp/stream/${seg}`, { format: "bytes" }); pull.segment!.push(ms(t)); }
      t = performance.now(); await sbx.files.read("/tmp/stream/thumb.jpg", { format: "bytes" }); pull.still!.push(ms(t));
    }
    const med = (v: number[]) => [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)] ?? null;
    for (const line of r.stdout.trim().split("\n").filter((l: string) => l.startsWith("{"))) {
      console.log(JSON.stringify({ host: "e2b", ...JSON.parse(line), pull_ms_median: { playlist: med(pull.playlist!), segment: med(pull.segment!), still: med(pull.still!) } }));
    }
    if (!r.stdout.includes("{")) console.log(JSON.stringify({ host: "e2b", setting: s, error: String(r.stderr ?? "").slice(0, 300) }));
  }
} finally {
  await api.kill(sbx.sandboxId);
  console.log(JSON.stringify({ event: "killed", minutes: Number(((Date.now() - t0) / 60000).toFixed(1)), e2b_usd_list: Number((((Date.now() - t0) / 1000) * 0.0000185).toFixed(4)) }));
}
