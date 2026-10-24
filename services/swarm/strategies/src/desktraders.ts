// House desktop agents whose job is practice trading (services/swarm/desktop/src/roster.ts,
// docs/HOUSE-AGENTS.md): cadre-practice runs them from their own keys when STRAT_DESK_TRADERS lists
// them; their desktops read <state>/<handle>.json to show the trades. Each gets one of the practice
// styles (src/styles.ts), distinct from the three original house practice agents' sets.
import { join } from "node:path";
import type { AgentCfg } from "./agent.ts";
import { STYLES } from "./styles.ts";

export const DESK_STYLE: Record<string, string> = { foxglove47: "momentum", meadowsweet27: "bounce", primrosedusk54: "contrarian", boragemist71: "calm" };

export function deskTraders(list: string | undefined, keysDir: string): (AgentCfg & { keyPath: string; style: string })[] {
  return (list ?? "").split(",").map((h) => h.trim()).filter((h) => /^[a-z0-9_]{3,24}$/.test(h)).map((h, i) => {
    const st = STYLES.find((x) => x.id === DESK_STYLE[h]) ?? STYLES[i % STYLES.length]!;
    return { handle: h, name: h, persona: `${h}, a Cadre house practice trader: ${st.persona}`, base: st.base, keyPath: join(keysDir, `agent-${h}.json`), style: st.id };
  });
}
