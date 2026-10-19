"""Tests for tools/cadre (quantize.py + worker.py) on a tiny synthetic Llama (1 layer, hidden 2048,
intermediate 4096 so the down projection is cut into two k shards, vocab 512).

Run: python test_cadre.py   (needs torch + transformers; the tokenizer files are copied from
CADRE_TOKENIZER_DIR, default ~/cadre-models/llama-3.2-1b-w7a7, or any Llama 3 tokenizer dir)
"""
import json, os, shutil, subprocess, sys, tempfile, unittest

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import worker  # noqa: E402

TOK = os.environ.get("CADRE_TOKENIZER_DIR", os.path.expanduser("~/cadre-models/llama-3.2-1b-w7a7-k2048"))


def tiny_checkpoint(d):
    from transformers import LlamaConfig, LlamaForCausalLM
    torch.manual_seed(0)
    cfg = LlamaConfig(hidden_size=2048, intermediate_size=4096, num_hidden_layers=1, num_attention_heads=32, num_key_value_heads=8,
                      head_dim=64, vocab_size=512, tie_word_embeddings=True, max_position_embeddings=512, rope_theta=10000.0)
    m = LlamaForCausalLM(cfg).to(torch.bfloat16)
    m.save_pretrained(d, safe_serialization=True)
    for f in ("tokenizer.json", "tokenizer_config.json", "special_tokens_map.json"):
        if os.path.exists(os.path.join(TOK, f)):
            shutil.copyfile(os.path.join(TOK, f), os.path.join(d, f))


class Cadre(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix="cadre-test-")
        cls.hf = os.path.join(cls.tmp, "hf")
        cls.out = os.path.join(cls.tmp, "model")
        tiny_checkpoint(cls.hf)
        r = subprocess.run([sys.executable, os.path.join(HERE, "quantize.py"), "--model", cls.hf, "--out", cls.out, "--name", "tiny", "--calib", "none"],
                           capture_output=True, text=True)
        assert r.returncode == 0, r.stderr[-2000:]
        cls.man = json.load(open(os.path.join(cls.out, "manifest.json")))
        cls.model = worker.Model(cls.out)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_manifest_matrices_and_k_shards(self):
        names = [m["name"] for m in self.man["matrices"]]
        self.assertEqual(names, ["layers.0.qkv", "layers.0.o", "layers.0.gate", "layers.0.up", "layers.0.down{0:2048}", "layers.0.down{2048:4096}", "lm_head"])
        for m in self.man["matrices"]:
            self.assertLessEqual(m["k"], 2048, "registered k is at most the light client's 2048")
            q = np.fromfile(os.path.join(self.out, m["weights"]), dtype=np.int8)
            self.assertEqual(q.size, m["k"] * m["n"])
            self.assertTrue(np.abs(q.astype(np.int32)).max() <= 64)
        self.assertEqual(self.man["macs_per_row"], sum(m["k"] * m["n"] for m in self.man["matrices"]))
        self.assertEqual(self.man["unmined"], [])

    def test_k_shards_sum_to_the_whole_product(self):
        # the down projection through its two shards equals one int32 product over the whole k
        x = torch.randn(5, 4096)
        y = self.model.lin("layers.0.down", x, None)
        ks = self.model.by_name["layers.0.down"][(0, 2048)]
        v, s, z = worker.quant_act(x)
        qw = torch.cat([m.qw for m in ks], 1)
        c = torch.from_numpy(v.numpy().astype(np.int64) @ qw.numpy().astype(np.int64).T)
        colsum = qw.to(torch.int64).sum(1).float()
        ref = s * (c.float() - z * colsum) * ks[0].sw
        self.assertTrue(torch.equal(y, ref) or torch.allclose(y, ref, rtol=0, atol=1e-6))

    def test_generation_writes_exact_job_products(self):
        d = os.path.join(self.tmp, "req")
        ids = [1, 2, 3, 4, 5]
        r = self.model.generate(ids, 4, d, pad=False)
        rows = len(ids) + len(r["output_ids"]) - 1
        self.assertEqual(r["rows"], rows)
        self.assertEqual(r["rows_padded"], rows, "pad false keeps exactly the rows")
        for m in self.man["matrices"]:
            a = np.fromfile(os.path.join(d, "a", f"{m['index']}.i8"), dtype=np.int8).reshape(rows, m["k"])
            c = np.fromfile(os.path.join(d, "c", f"{m['index']}.i32"), dtype="<i4").reshape(rows, m["n"])
            q = np.fromfile(os.path.join(self.out, m["weights"]), dtype=np.int8).reshape(m["n"], m["k"])
            self.assertTrue(np.abs(a.astype(np.int32)).max() <= 64, "activations are int7")
            self.assertTrue((a.astype(np.int64) @ q.astype(np.int64).T == c).all(), f"C = A B exactly for {m['name']}")
        # greedy decoding is deterministic, and padding only appends zero rows
        r2 = self.model.generate(ids, 4, os.path.join(self.tmp, "req2"))
        self.assertEqual(r2["output_ids"], r["output_ids"])
        self.assertEqual(r2["rows_padded"] % 4, 0)

    def test_stop_and_streaming_callback(self):
        got = []
        r = self.model.generate([1, 2, 3], 6, None, on_token=lambda t, s: got.append(t))
        self.assertEqual(got, r["output_ids"], "every generated token is streamed")
        self.assertIn(r["finish_reason"], ("length", "stop"))
        self.assertEqual(r["completion_tokens"], len(r["output_ids"]))


if __name__ == "__main__":
    unittest.main(verbosity=2)
