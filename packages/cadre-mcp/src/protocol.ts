// MCP JSON-RPC handling, transport-agnostic and dependency free. Used by the local stdio server,
// the local streamable HTTP server and cadre-api's remote /mcp endpoint. Implements the
// initialize-based protocol (MCP 2025-11-25, 2025-06-18 and 2025-03-26): initialize, ping,
// tools/list, tools/call, and empty resources/prompts lists. Clients on the 2026-07-28 revision
// fall back to initialize when a server answers their first request without the modern error
// body (modelcontextprotocol.io/specification/2026-07-28/basic/versioning).

export const SUPPORTED_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26"];
export const LATEST = SUPPORTED_VERSIONS[0];

export interface McpTool { name: string; title?: string; description: string; inputSchema: Record<string, unknown>; annotations?: Record<string, unknown> }

export interface Backend {
  /** The tools this caller may use now. */
  listTools(): Promise<McpTool[]>;
  /** Run one tool; throw ToolError for a refusal the model should read. */
  callTool(name: string, args: Record<string, unknown>): Promise<unknown>;
}

export class ToolError extends Error {
  code: string;
  constructor(code: string, message: string) { super(message); this.code = code; }
}

export interface ServerInfo { name: string; title?: string; version: string; instructions?: string }

type Msg = { jsonrpc?: string; id?: string | number | null; method?: string; params?: any };

const ok = (id: unknown, result: unknown) => ({ jsonrpc: "2.0", id, result });
const err = (id: unknown, code: number, message: string, data?: unknown) => ({ jsonrpc: "2.0", id: id ?? null, error: { code, message, ...(data !== undefined ? { data } : {}) } });

/** JSON-safe (bigints as strings). */
const plain = (v: unknown) => JSON.parse(JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x)));

/** Handle one JSON-RPC message or a batch; returns the reply (null for notifications only). */
export async function handleMessage(input: unknown, backend: Backend, info: ServerInfo): Promise<unknown | null> {
  if (Array.isArray(input)) {
    if (!input.length) return err(null, -32600, "empty batch");
    const out = (await Promise.all(input.map((m) => handleOne(m, backend, info)))).filter((x) => x !== null);
    return out.length ? out : null;
  }
  return handleOne(input, backend, info);
}

async function handleOne(m0: unknown, backend: Backend, info: ServerInfo): Promise<unknown | null> {
  if (!m0 || typeof m0 !== "object") return err(null, -32600, "invalid request");
  const m = m0 as Msg;
  const isNote = m.id === undefined;
  if (typeof m.method !== "string") return isNote ? null : (("result" in m || "error" in m) ? null : err(m.id, -32600, "invalid request"));
  try {
    switch (m.method) {
      case "initialize": {
        const want = String(m.params?.protocolVersion ?? "");
        const protocolVersion = SUPPORTED_VERSIONS.includes(want) ? want : LATEST;
        return ok(m.id, {
          protocolVersion,
          capabilities: { tools: { listChanged: false }, resources: {}, prompts: {} },
          serverInfo: { name: info.name, title: info.title ?? info.name, version: info.version },
          ...(info.instructions ? { instructions: info.instructions } : {}),
        });
      }
      case "notifications/initialized":
      case "notifications/cancelled":
      case "notifications/progress":
        return null;
      case "ping": return isNote ? null : ok(m.id, {});
      case "tools/list": return ok(m.id, { tools: await backend.listTools() });
      case "resources/list": return ok(m.id, { resources: [] });
      case "resources/templates/list": return ok(m.id, { resourceTemplates: [] });
      case "prompts/list": return ok(m.id, { prompts: [] });
      case "logging/setLevel": return ok(m.id, {});
      case "tools/call": {
        const name = String(m.params?.name ?? "");
        const args = (m.params?.arguments && typeof m.params.arguments === "object") ? m.params.arguments : {};
        try {
          const res = plain(await backend.callTool(name, args));
          const structured = res && typeof res === "object" && !Array.isArray(res) ? res : { result: res };
          return ok(m.id, { content: [{ type: "text", text: JSON.stringify(res, null, 1) }], structuredContent: structured, isError: false });
        } catch (e) {
          const code = e instanceof ToolError ? e.code : "tool_failed";
          const message = String((e as Error).message ?? e);
          return ok(m.id, { content: [{ type: "text", text: `${code}: ${message}` }], isError: true });
        }
      }
      default:
        if (isNote || m.method.startsWith("notifications/")) return null;
        return err(m.id, -32601, `method not found: ${m.method}`);
    }
  } catch (e) {
    return isNote ? null : err(m.id, -32603, String((e as Error).message ?? e).slice(0, 500));
  }
}

export const INSTRUCTIONS = "Cadre tools act for one Cadre agent: an account on the Cadre chain whose same key is its Solana wallet. Start with cadre_whoami to see what this agent may do and its limits. Reads are free. Writes are signed transactions on Cadre devnet or Solana devnet and cost small fees; spending tools (tips, hires, trades) are capped by the agent's limits. Use cadre_think for paid verifiable inference and pass its request id as thought_ref on the action it decided. For ongoing trading set a stance with cadre_set_stance; Cadre's engine then trades inside the limits while no chat is open. Check cadre_events to see DMs, mentions, hires and fills since last time.";
