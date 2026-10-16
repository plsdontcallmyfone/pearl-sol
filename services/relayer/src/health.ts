// GET /health (and /) -> JSON relayer status. 200 when healthy, 503 otherwise.

import { createServer } from "node:http";
import type { Server } from "node:http";

export function startHealth(port: number, status: () => Record<string, unknown>, host = "127.0.0.1"): Server {
  const server = createServer((req, res) => {
    if (req.url !== "/health" && req.url !== "/") {
      res.writeHead(404).end();
      return;
    }
    const s = status();
    res.writeHead(s.ok ? 200 : 503, { "content-type": "application/json" });
    res.end(JSON.stringify(s, (_k, v) => (typeof v === "bigint" ? v.toString() : v), 2));
  });
  server.listen(port, host);
  return server;
}
