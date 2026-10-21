// Builds the E2B template `cadre-desktop` for the house desktops: E2B's own `desktop` template
// (Ubuntu 22.04, XFCE, Xvfb, xdotool, ffmpeg) plus Node 22, Playwright's Chromium (the only
// Chromium build that still loads an unpacked extension with --load-extension) with its system
// libraries, and the screensaver / power manager / lock removed so the stream never blanks.
// Sized small on purpose: the stock `desktop` template is 8 vCPU / 8 GiB (read on a real sandbox
// 2026-10-21), which bills 0.000148 USD/s; the house desktops run at DESK_VCPU / DESK_MEM_MB.
//
//   node src/template.ts [--name cadre-desktop] [--vcpu 1] [--mem 1024] [--check]
//
// --check starts one sandbox from the built template, prints nproc, memory and missing tools, kills it.
// The E2B key is read from the file named by E2B_ENV_FILE (never printed).
import { loadSecrets, usdPerSecond } from "./config.ts";

const argv = process.argv.slice(2);
const opt = (k: string, d: string) => (argv.includes(k) ? argv[argv.indexOf(k) + 1]! : d);
const name = opt("--name", "cadre-desktop");
const vcpu = Number(opt("--vcpu", "1"));
const memMb = Number(opt("--mem", "1024"));
const sec = loadSecrets();
if (!sec.e2bKey) throw new Error("no E2B_API_KEY (E2B_ENV_FILE)");
process.env.E2B_API_KEY = sec.e2bKey;
const { Template, Sandbox } = (await import("@e2b/desktop")) as any;

if (!argv.includes("--check-only")) {
  const t = Template()
    .fromTemplate("desktop")
    .runCmd(
      [
        "rm -f /etc/xdg/autostart/xfce4-screensaver.desktop /etc/xdg/autostart/xfce4-power-manager.desktop /etc/xdg/autostart/light-locker.desktop /etc/xdg/autostart/xscreensaver.desktop",
        "curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null",
        "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nodejs >/dev/null",
        "mkdir -p /opt/deskhost /opt/ms-playwright",
        "cd /opt/deskhost && npm init -y >/dev/null && npm i --silent playwright-core@1.56.1",
        "cd /opt/deskhost && PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright npx -y playwright@1.56.1 install --with-deps chromium",
        "chmod -R a+rX /opt/ms-playwright /opt/deskhost",
        "rm -rf /var/lib/apt/lists/*",
      ],
      { user: "root" },
    )
    .setEnvs({ PLAYWRIGHT_BROWSERS_PATH: "/opt/ms-playwright", NODE_PATH: "/opt/deskhost/node_modules" });
  const t0 = Date.now();
  const info = await Template.build(t, name, { cpuCount: vcpu, memoryMB: memMb, onBuildLogs: (l: any) => process.stdout.write(`${String(l?.message ?? l).slice(0, 200)}\n`) });
  console.log(`built ${name} (${info?.templateId ?? "?"}) in ${((Date.now() - t0) / 1000).toFixed(0)} s: ${vcpu} vCPU, ${memMb} MiB, ${usdPerSecond(vcpu, memMb / 1024).toFixed(7)} USD/s`);
}
if (argv.includes("--check") || argv.includes("--check-only")) {
  const t0 = Date.now();
  const s = await Sandbox.create(name, { timeoutMs: 180_000, resolution: [1280, 800], metadata: { cadre_desktop: "template-check" } });
  try {
    const r = await s.commands.run("nproc; free -m | sed -n 2p; node -v; ls /opt/ms-playwright; for t in xdotool ffmpeg node; do command -v $t >/dev/null || echo missing $t; done", { timeoutMs: 60_000 });
    console.log(r.stdout.trim());
  } finally {
    await s.kill();
    console.log(`check sandbox up ${((Date.now() - t0) / 1000).toFixed(1)} s in all`);
  }
}
