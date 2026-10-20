"""Stand-in for tools/cadre/worker.py that forwards to a worker on another machine (a GPU pod).

The gateway (cadre-gateway) talks to its worker over HTTP and then reads the segment files
(out_dir/a/<i>.i8, out_dir/c/<i>.i32) from its own disk. This shim listens where the local
worker would, forwards /health, /tokenize and /generate to the remote worker (usually through
an ssh tunnel), and before it hands back a /generate result it copies that request's out_dir
from the remote machine to the same path here with --sync (a command template, {dir} = the
out_dir), so the gateway finds the files where it asked for them. Streams pass through line by
line; the final line waits for the copy.

Usage:
  python remote_worker.py --upstream http://127.0.0.1:10765 --port 10665 \\
     --sync "rsync -a -e 'ssh -i KEY -p PORT' root@HOST:{dir}/ {dir}/"
The remote worker must be able to write the same absolute out_dir path (it creates it).
"""
import argparse, json, os, shlex, subprocess, sys, time, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--upstream", required=True)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=10665)
    ap.add_argument("--sync", required=True, help="command template; {dir} is the request's out_dir")
    ap.add_argument("--log", default="", help="append one JSON line per /generate (timings) to this file")
    a = ap.parse_args()

    def sync(d):
        t0 = time.time()
        os.makedirs(d, exist_ok=True)
        cmd = a.sync.replace("{dir}", shlex.quote(d))
        r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError(f"sync failed ({r.returncode}): {r.stderr[-400:]}")
        return time.time() - t0

    def log(rec):
        if a.log:
            with open(a.log, "a") as f:
                f.write(json.dumps(rec) + "\n")

    class H(BaseHTTPRequestHandler):
        def log_message(self, *x):
            pass

        def _up(self, method, body=None):
            req = urllib.request.Request(a.upstream + self.path, data=body, method=method, headers={"content-type": "application/json"})
            return urllib.request.urlopen(req, timeout=3600)

        def _reply(self, code, b, ctype="application/json"):
            self.send_response(code)
            self.send_header("content-type", ctype)
            self.send_header("content-length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def do_GET(self):
            try:
                with self._up("GET") as r:
                    b = r.read()
                if self.path == "/health":
                    j = json.loads(b)
                    j["remote"] = a.upstream
                    b = json.dumps(j).encode()
                self._reply(200, b)
            except urllib.error.HTTPError as e:
                self._reply(e.code, e.read())
            except Exception as e:  # noqa: BLE001
                self._reply(502, json.dumps({"error": f"upstream: {e}"}).encode())

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("content-length", 0)))
            try:
                j = json.loads(body or b"{}")
                t0 = time.time()
                r = self._up("POST", body)
                if self.path != "/generate":
                    with r:
                        return self._reply(r.status, r.read())
                out_dir = j.get("out_dir")
                if not j.get("stream"):
                    with r:
                        b = r.read()
                    t_gen = time.time() - t0
                    t_sync = sync(out_dir) if out_dir else 0.0
                    log({"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "stream": False, "generate_s": round(t_gen, 3), "sync_s": round(t_sync, 3),
                         "timing": json.loads(b).get("timing")})
                    return self._reply(200, b)
                self.send_response(200)
                self.send_header("content-type", "application/x-ndjson")
                self.end_headers()
                with r:
                    for line in r:
                        if line.startswith(b'{"done"'):
                            t_gen = time.time() - t0
                            t_sync = sync(out_dir) if out_dir else 0.0
                            log({"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "stream": True, "generate_s": round(t_gen, 3), "sync_s": round(t_sync, 3),
                                 "timing": json.loads(line).get("timing")})
                        self.wfile.write(line)
                        self.wfile.flush()
            except urllib.error.HTTPError as e:
                self._reply(e.code, e.read())
            except Exception as e:  # noqa: BLE001
                self._reply(502, json.dumps({"error": f"{type(e).__name__}: {e}"}).encode())

    srv = ThreadingHTTPServer((a.host, a.port), H)
    print(json.dumps({"msg": "cadre remote worker shim", "url": f"http://{a.host}:{a.port}", "upstream": a.upstream, "pid": os.getpid()}), flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
