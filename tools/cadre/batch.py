"""Continuous batching for the Cadre worker (SPEC 4.7 "Batching", SPEC 5.4 "Worker concurrency").

One scheduler thread owns the model. Every step it runs ONE forward over the rows of every
active request (glue.DetGlue.forward_multi): one decode row per request that is generating, plus
a chunk of prompt rows for requests still in prefill. The int32 GEMMs see all those rows stacked
(m grows with the batch); attention reads each request's own KV cache (the decode rows of all
pooled requests in one batched attention). Requests join between steps (admission) and leave the
step after their last token, so a short answer never waits for a long one.

Bit identity: every per-row operation of cadre-glue-v1 is a function of that row alone (integer
GEMM rows, per-row quantization and norms, elementwise binary64), attention reads only the
request's own cache (padding past its length gets weight exactly 0), and integer sums are exact
in any grouping, so a request's A rows, C rows, logits and tokens are the same bits as when it is
served alone (Model.generate). test_batch.py checks this file by file and logits row by row, and
the evaluator's replay (crates/cadre-glue `cadre-replay local`) accepts every batched request.

Each request still owns its rows: a step records which rows are whose, and its A/C rows are split
out of the stacked step tensors (prompt rows, then one row per fed-back token, in order) and
appended to that request's own segment files: exactly what the gateway posts as that ticket's
segments. Nothing about the chain changes.

Memory: the step tensors (A and C of every matrix over the step's rows, 2.7 MB per row on
Llama-3.2-1B, mostly lm_head's C) are held until `flush_rows` rows are retained or a request ends;
then every request's rows go to its files (a writer thread appends them) and the step tensors are
freed. KV caches are preallocated for prompt + max_new positions (a pool of max_batch slots on
CUDA, one cache per request on CPU); `max_live_rows` bounds the sum over admitted requests.

Fairness and backpressure: FIFO admission (the head of the queue is never skipped, so a large
request cannot starve), at most `max_batch` requests active, prefill chunked to `max_step_rows`
rows per step after the decode rows, and at most `max_queue` requests waiting: one more is refused
at once (Busy, HTTP 503 with Retry-After) instead of queueing without bound.
"""
import os, queue, threading, time
from collections import deque

import numpy as np
import torch

import glue as gluemod


class Busy(Exception):
    """The queue is full; the caller answers 503 and the client retries."""


class Job:
    def __init__(self, ids, max_new, out_dir, stop, pad):
        self.ids = list(ids)
        self.max_new = max_new
        self.out_dir = out_dir
        self.stop = [x for x in (stop or []) if x]
        self.pad = pad
        self.need = len(self.ids) + max_new  # KV positions (and live rows) reserved
        self.out = []
        self.fed = 0             # prompt rows already through the model
        self.kv = None
        self.finish = None
        self.error = None
        self.cancelled = False
        self.events = queue.Queue()  # ("token", id) ..., then ("done", None) or ("error", msg)
        self.t_submit = time.time()
        self.t_admit = self.t_first = self.t_done = None
        self.steps = 0
        self.batch_sum = 0
        self.rows_sent = 0       # rows handed to the writer
        self.started_files = set()
        self.last_fed_step = 0   # batcher step at which this request last got prompt rows

    @property
    def rows(self):
        return len(self.ids) + max(len(self.out) - 1, 0)


class SegmentWriter(threading.Thread):
    """Appends each request's A/C rows to out_dir/a/<i>.i8 and out_dir/c/<i>.i32 (the same files
    and bytes Model.generate writes), off the scheduler thread. A flush carries every matrix's
    stacked A and C (numpy, CPU) of the retained steps and, per request, its row ranges in them."""

    def __init__(self, n):
        super().__init__(daemon=True, name=f"cadre-segment-writer-{n}")
        self.q = queue.Queue()

    def run(self):
        while True:
            mats, per_job = self.q.get()
            for job, ranges, final, pad_rows in per_job:
                try:
                    if job.out_dir is not None and job.error is None:
                        for index, (A, C) in mats.items():
                            for kind, arr in (("a", A), ("c", C)):
                                path = os.path.join(job.out_dir, kind, f"{index}.{'i8' if kind == 'a' else 'i32'}")
                                mode = "ab" if (kind, index) in job.started_files else "wb"
                                job.started_files.add((kind, index))
                                with open(path, mode) as f:
                                    for a, b in ranges:
                                        f.write(memoryview(np.ascontiguousarray(arr[a:b])).cast("B"))
                                    if final and pad_rows:
                                        f.write(np.zeros((pad_rows,) + arr.shape[1:], dtype=arr.dtype).tobytes())
                        if final and not mats:  # nothing retained for this job's last flush
                            for index, (k, n) in job.shapes.items():
                                for kind, width, dt in (("a", k, np.int8), ("c", n, "<i4")):
                                    path = os.path.join(job.out_dir, kind, f"{index}.{'i8' if kind == 'a' else 'i32'}")
                                    mode = "ab" if (kind, index) in job.started_files else "wb"
                                    job.started_files.add((kind, index))
                                    with open(path, mode) as f:
                                        if pad_rows:
                                            f.write(np.zeros((pad_rows, width), dtype=dt).tobytes())
                except Exception as e:  # noqa: BLE001
                    job.error = f"segment write: {type(e).__name__}: {e}"
                if final:
                    job.t_done = job.t_done or time.time()
                    job.events.put(("error", job.error) if job.error else ("done", None))


class Batcher(threading.Thread):
    def __init__(self, model, max_batch=16, max_queue=64, max_step_rows=256, max_live_rows=8192, flush_rows=128, kv_pool=None, pool_positions=2048, writers=4,
                 prefill_order="short", starve_steps=8):
        super().__init__(daemon=True, name="cadre-batcher")
        assert model.det is not None, "continuous batching needs cadre-glue-v1 (the fp32 glue is not batch-invariant)"
        assert max_batch >= 1 and max_step_rows >= max_batch, "max_step_rows must cover one decode row per active request"
        self.m = model
        self.max_batch, self.max_queue, self.max_step_rows = max_batch, max_queue, max_step_rows
        self.max_live_rows, self.flush_rows = max_live_rows, flush_rows
        assert prefill_order in ("short", "fifo"), prefill_order
        self.order, self.starve_steps = prefill_order, max(1, starve_steps)
        if kv_pool is None:
            kv_pool = model.dev.type == "cuda"
        self.pool = gluemod.KVPool(model.det, max_batch, pool_positions) if kv_pool and max_batch > 1 else None
        self.cv = threading.Condition()
        self.waiting = deque()
        self.active = []
        self.live_rows = 0
        # writers: a request always goes to the same one (its files stay in order), requests
        # spread over them (disk writes of 2.7 MB per row are the limit at high batch on a GPU)
        self.writers = [SegmentWriter(i) for i in range(writers)]
        self.eos = model.eos_ids()
        self.shapes = {m.index: (m.k, m.n) for m in model.mats}
        self.retained = []        # steps whose A/C are not flushed: (spans [(job, lo, hi)], {index: (A, C)}, rows)
        self.retained_rows = 0
        self.stats = {"steps": 0, "rows": 0, "tokens": 0, "served": 0, "refused_busy": 0, "errors": 0, "busy_s": 0.0, "max_active": 0, "flushes": 0,
                      "prompt_rows": 0, "prefix_rows": 0}
        self.recent = deque(maxlen=64)  # last prompts, for the prefix_rows measurement
        self.t_start = time.time()
        self.logits_log = None
        for w in self.writers:
            w.start()
        self.start()

    # ------------------------------------------------------------------ client side

    def submit(self, ids, max_new, out_dir=None, stop=None, pad=True):
        job = Job(ids, max_new, out_dir, stop, pad)
        job.shapes = self.shapes
        with self.cv:
            self.n_submitted = getattr(self, "n_submitted", 0) + 1
            job.writer = self.n_submitted % len(self.writers)
        if job.need > gluemod.T_MAX:
            raise ValueError(f"prompt + max_new_tokens is {job.need} positions, above T_MAX {gluemod.T_MAX}")
        if job.need > self.max_live_rows:
            raise ValueError(f"prompt + max_new_tokens is {job.need} rows, above this worker's max_live_rows {self.max_live_rows}")
        if out_dir is not None:
            os.makedirs(os.path.join(out_dir, "a"), exist_ok=True)
            os.makedirs(os.path.join(out_dir, "c"), exist_ok=True)
        self.note_prefix(job.ids)
        with self.cv:
            if len(self.waiting) >= self.max_queue:
                self.stats["refused_busy"] += 1
                raise Busy(f"{len(self.waiting)} requests waiting (max_queue {self.max_queue}), {len(self.active)} active")
            self.waiting.append(job)
            self.cv.notify()
        return job

    def note_prefix(self, ids):
        """Measurement only (nothing is reused): the longest prefix this prompt shares with one of
        the last 64 prompts. A prefix cache could serve those rows' A, C and KV from the earlier
        request (they are the same bits: every row depends only on the rows before it), so
        prefix_rows / prompt_rows in /health is the share of prompt rows such a cache would save."""
        best = 0
        for prev in self.recent:
            n = min(len(prev), len(ids) - 1)  # the last prompt row always runs (its logits pick the first token)
            i = 0
            while i < n and prev[i] == ids[i]:
                i += 1
            best = max(best, i)
        with self.cv:
            self.stats["prompt_rows"] += len(ids)
            self.stats["prefix_rows"] += best
            self.recent.append(list(ids))

    def cancel(self, job):
        job.cancelled = True

    def health(self):
        with self.cv:
            up = max(time.time() - self.t_start, 1e-9)
            return {"mode": "continuous", "max_batch": self.max_batch, "max_queue": self.max_queue, "max_step_rows": self.max_step_rows,
                    "max_live_rows": self.max_live_rows, "flush_rows": self.flush_rows, "kv_pool": self.pool is not None,
                    "prefill_order": self.order, "threads": torch.get_num_threads(),
                    "active": len(self.active), "waiting": len(self.waiting), "live_rows": self.live_rows, **self.stats,
                    "busy_frac": round(self.stats["busy_s"] / up, 3)}

    # ------------------------------------------------------------------ scheduler

    def admit(self):
        """FIFO: admit from the head while a slot and the live-row budget allow; never skip it."""
        while self.waiting and len(self.active) < self.max_batch:
            j = self.waiting[0]
            if j.cancelled:
                self.waiting.popleft()
                j.events.put(("error", "cancelled"))
                continue
            if self.active and self.live_rows + j.need > self.max_live_rows:
                break
            self.waiting.popleft()
            j.t_admit = time.time()
            j.last_fed_step = self.stats["steps"]
            slot = self.pool.take() if self.pool is not None and j.need <= self.pool.cap_t else None
            j.kv = slot if slot is not None else gluemod.KVCache(self.m.det, j.need)
            self.live_rows += j.need
            self.active.append(j)
        self.stats["max_active"] = max(self.stats["max_active"], len(self.active))

    def plan(self):
        """One step's parts: a decode row for every generating request, then prompt chunks (FIFO)
        up to max_step_rows."""
        parts = []
        budget = self.max_step_rows
        for j in self.active:
            if j.fed == len(j.ids) and j.out:
                parts.append((j, [j.out[-1]]))
                budget -= 1
        for j in self.prefill_order():
            if budget <= 0:
                break
            n = min(len(j.ids) - j.fed, budget)
            parts.append((j, j.ids[j.fed:j.fed + n]))
            budget -= n
            j.last_fed_step = self.stats["steps"]
        return parts

    def prefill_order(self):
        """Requests still in prefill, in the order they get this step's prompt rows.
        "fifo": admission order. "short" (default): fewest prompt rows left first, so a short
        thought admitted next to a long report reaches its first token after its own prefill,
        not after the report's (shortest remaining work first minimises mean wait); a request
        that got no prompt rows for `starve_steps` steps goes first, so a long prompt still
        advances while short ones keep arriving. Order changes which rows share a step, never a
        request's bits (every row is computed as if alone; test_batch.py, golden.py)."""
        pre = [j for j in self.active if j.fed < len(j.ids)]
        if self.order == "fifo" or len(pre) < 2:
            return pre
        now = self.stats["steps"]
        starved = [j for j in pre if now - j.last_fed_step >= self.starve_steps]
        rest = sorted((j for j in pre if j not in starved), key=lambda j: len(j.ids) - j.fed)
        return starved + rest

    def retire(self, j):
        self.active.remove(j)
        self.live_rows -= j.need
        if isinstance(j.kv, gluemod.KVSlot):
            self.pool.give(j.kv)
        j.kv = None

    def flush(self, finals):
        """Hand every retained row to the writer (per request, in order) and free the step
        tensors. finals: requests whose last rows these are (padding and the done event follow)."""
        mats = {}
        if self.retained:
            for index in self.shapes:
                A = torch.cat([st[1][index][0] for st in self.retained]) if len(self.retained) > 1 else self.retained[0][1][index][0]
                C = torch.cat([st[1][index][1] for st in self.retained]) if len(self.retained) > 1 else self.retained[0][1][index][1]
                mats[index] = (A.cpu().numpy(), C.cpu().numpy().astype("<i4", copy=False))
        ranges = {}
        off = 0
        for spans, _, nrows in self.retained:
            for j, lo, hi in spans:
                ranges.setdefault(j, []).append((off + lo, off + hi))
            off += nrows
        per_job = []
        period = len(self.m.man["rows_pattern"])
        for j in set(ranges) | set(finals):
            rs = ranges.get(j, [])
            j.rows_sent += sum(b - a for a, b in rs)
            final = j in finals
            pad_rows = 0
            if final and j.error is None:
                assert j.out_dir is None or j.rows_sent == j.rows, (j.rows_sent, j.rows)
                if j.pad:
                    pad_rows = -(-j.rows // period) * period - j.rows
            per_job.append((j, rs, final, pad_rows))
        self.retained = []
        self.retained_rows = 0
        self.stats["flushes"] += 1
        mats = mats if any(j.out_dir for j, *_ in per_job) else {}
        by_w = {}
        for item in per_job:
            by_w.setdefault(item[0].writer, []).append(item)
        for w, items in by_w.items():
            self.writers[w].q.put((mats, items))

    def step(self):
        parts = self.plan()
        if not parts:
            return
        t0 = time.time()
        spans, lo = [], 0
        for j, ids in parts:
            spans.append((j, lo, lo + len(ids)))
            lo += len(ids)
        rec = {}
        keep = any(j.out_dir is not None for j, _ in parts)

        def cap(index, vs, part):
            if keep:
                rec[index] = (vs, part)

        with self.m.lock, torch.no_grad():
            lasts = self.m.det.forward_multi([(ids, j.kv) for j, ids in parts], cap)
            toks = gluemod.pick_rows(lasts)
            if self.logits_log is not None:  # tests: every token's binary64 logits row, per request
                rows = lasts.cpu().numpy()
                for i, (j, ids) in enumerate(parts):
                    if j.fed + len(ids) >= len(j.ids):
                        self.logits_log.setdefault(id(j), []).append(rows[i].tobytes())
        dt = time.time() - t0
        if keep:
            self.retained.append(([(j, a, b) for j, a, b in spans if j.out_dir is not None], rec, lo))
            self.retained_rows += lo
        self.stats["steps"] += 1
        self.stats["rows"] += lo
        self.stats["busy_s"] += dt
        tok = self.m.tokenizer()
        finals = []
        for (j, ids), nxt in zip(parts, toks):
            j.steps += 1
            j.batch_sum += len(parts)
            if j.fed < len(j.ids):
                j.fed += len(ids)
                if j.fed < len(j.ids):
                    continue  # mid-prompt chunk: no token yet
            # same order of checks as Model.generate: stop strings, eos, length
            j.out.append(nxt)
            self.stats["tokens"] += 1
            if j.t_first is None:
                j.t_first = time.time()
            j.events.put(("token", nxt))
            if j.stop and any(x in tok.decode(j.out, skip_special_tokens=True) for x in j.stop):
                j.finish = "stop"
            elif nxt in self.eos:
                j.finish = "stop"
            elif len(j.out) >= j.max_new:
                j.finish = "length"
            if j.finish:
                # the last token is never fed back: its row was not computed, rows = prompt + out - 1
                j.t_done = time.time()
                self.retire(j)
                self.stats["served"] += 1
                finals.append(j)
        if finals or self.retained_rows >= self.flush_rows:
            self.flush(finals)

    def fail(self, jobs, msg):
        for j in jobs:
            if j in self.active:
                self.retire(j)
            j.error = msg
        self.flush(list(jobs))

    def run(self):
        while True:
            with self.cv:
                while not self.waiting and not self.active:
                    self.cv.wait()
                gone = [j for j in self.active if j.cancelled]
                self.admit()
            if gone:
                self.fail(gone, "cancelled")
            try:
                self.step()
            except Exception as e:  # noqa: BLE001  one bad step fails the requests in it, not the worker
                self.stats["errors"] += 1
                self.retained = []
                self.retained_rows = 0
                self.fail(list(self.active), f"{type(e).__name__}: {e}")


def result(model, job):
    """The /generate reply for a finished job: the same fields Model.generate returns."""
    tok = model.tokenizer()
    text = tok.decode(job.out, skip_special_tokens=True)
    for x in job.stop:
        if x in text:
            text = text[: text.index(x)]
    rows = job.rows
    period = len(model.man["rows_pattern"])
    padded = -(-rows // period) * period if job.pad else rows
    mats = []
    for m in model.mats:
        rec = {"index": m.index, "name": m.name, "m": padded, "k": m.k, "n": m.n}
        if job.out_dir:
            rec["a"] = os.path.join(job.out_dir, "a", f"{m.index}.i8")
            rec["c"] = os.path.join(job.out_dir, "c", f"{m.index}.i32")
        mats.append(rec)
    n = len(job.out)
    decode_s = (job.t_done - job.t_first) if job.t_first and job.t_done else 0.0
    macs = sum(m.k * m.n for m in model.mats)
    return {
        "output_ids": list(job.out), "text": text, "rows": rows, "rows_padded": padded, "prompt_tokens": len(job.ids),
        "completion_tokens": n, "finish_reason": job.finish, "glue": model.glue, "matrices": mats,
        "timing": {"queue_s": round(job.t_admit - job.t_submit, 3) if job.t_admit else None,
                   "prefill_s": round(job.t_first - job.t_admit, 3) if job.t_first else None,
                   "first_token_s": round(job.t_first - job.t_submit, 3) if job.t_first else None,
                   "decode_s": round(decode_s, 3),
                   "decode_tokens_per_s": round((n - 1) / decode_s, 3) if n > 1 and decode_s > 0 else None,
                   "total_s": round(job.t_done - job.t_submit, 3) if job.t_done else None,
                   "batch_mean": round(job.batch_sum / job.steps, 2) if job.steps else None,
                   "steps": job.steps, "mined_macs": macs * rows, "threads": torch.get_num_threads(), "device": str(model.dev),
                   "mode": "continuous"},
    }


def wait(model, job, on_token=None, timeout=None):
    """Block until the job ends; on_token(id, text delta) per token (the same deltas the
    unbatched worker streams). Returns result() or raises RuntimeError."""
    tok = model.tokenizer()
    out, text = [], ""
    while True:
        kind, v = job.events.get(timeout=timeout)
        if kind == "token":
            out.append(v)
            if on_token is not None:
                full = tok.decode(out, skip_special_tokens=True)
                on_token(v, full[len(text):])
                text = full
        elif kind == "done":
            return result(model, job)
        else:
            raise RuntimeError(v)
