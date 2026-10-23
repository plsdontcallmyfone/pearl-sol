"""Compression of real C segment files (int32 LE): ratio and single-core CPU time, raw vs 4-byte shuffle,
zstd (CLI, levels 1 and 3) and zlib (stdlib, level 1). Usage: czbench.py DIR_WITH_c/ ROWS"""
import glob, json, os, subprocess, sys, time, zlib
import numpy as np
d, rows = sys.argv[1], int(sys.argv[2])
files = sorted(glob.glob(os.path.join(d, "c", "*.i32")))
raw = b"".join(open(f, "rb").read() for f in files)
arr = np.frombuffer(raw, dtype="<i4")
t = time.process_time(); sh = arr.view(np.uint8).reshape(-1, 4).T.copy().tobytes(); t_sh = time.process_time() - t
out = {"files": len(files), "rows": rows, "raw_bytes": len(raw), "bytes_per_row": len(raw) // rows, "shuffle_cpu_s": round(t_sh, 3)}
def z(data, lvl, tag):
    p = "/home/pearl/tput/cz.tmp"; open(p, "wb").write(data)
    t = time.time(); c = subprocess.run(["zstd", f"-{lvl}", "-q", "-c", "-T1", p], capture_output=True).stdout; tc = time.time() - t
    open(p + ".zst", "wb").write(c)
    t = time.time(); subprocess.run(["zstd", "-d", "-q", "-c", p + ".zst"], capture_output=True); td = time.time() - t
    os.remove(p); os.remove(p + ".zst")
    out[tag] = {"ratio": round(len(c) / len(raw), 4), "compress_s": round(tc, 3), "decompress_s": round(td, 3),
                "compress_MBps": round(len(raw) / tc / 1e6, 1), "decompress_MBps": round(len(raw) / td / 1e6, 1)}
z(raw, 1, "zstd1_raw"); z(sh, 1, "zstd1_shuffle"); z(sh, 3, "zstd3_shuffle")
t = time.process_time(); c = zlib.compress(sh, 1); tc = time.process_time() - t
out["zlib1_shuffle"] = {"ratio": round(len(c) / len(raw), 4), "compress_s": round(tc, 3), "compress_MBps": round(len(raw) / tc / 1e6, 1)}
print(json.dumps(out))
