import cProfile, pstats, sys, os, torch
sys.path.insert(0, sys.argv[1])
import glue, worker
torch.set_num_threads(int(sys.argv[3]))
m = worker.Model(sys.argv[2])
n = int(sys.argv[4])
def go():
    kv = glue.KVCache(m.det, n + 8)
    with torch.no_grad():
        m.det.forward_multi([(list(range(1000, 1000 + n)), kv)], lambda i, a, c: None)
go()
pr = cProfile.Profile(); pr.enable(); go(); pr.disable()
st = pstats.Stats(pr); st.sort_stats("tottime").print_stats(18)
