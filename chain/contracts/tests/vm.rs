//! VM host checks: the example contracts validate, and malicious modules are refused at
//! deploy or stopped deterministically at run time.

use contracts::{Abort, Env, Mode, Reply, Request, Step, Vm, MAX_MEMORY_PAGES};

const MAX: usize = 128 * 1024;

fn env() -> Env {
    Env { chain_id: 7, height: 10, address: [1; 32], caller: [2; 32], origin: [2; 32], value: 0, miner: [3; 32] }
}

fn wat(s: &str) -> Vec<u8> {
    wat::parse_str(s).unwrap()
}

/// Run `entry` serving requests from a tiny in-memory store; returns (step, gas used).
fn run(vm: &Vm, code: &[u8], entry: &str, input: Vec<u8>, gas: u64, mode: Mode) -> (Step, u64) {
    let (module, _) = vm.validate(code, MAX).unwrap();
    let mut f = match vm.frame(&module, env(), mode, input, gas, 0) {
        Ok(f) => f,
        Err(a) => return (Step::Abort(a), gas),
    };
    let mut store = std::collections::BTreeMap::<Vec<u8>, Vec<u8>>::new();
    let mut step = f.start(entry);
    loop {
        match step {
            Step::Request(r) => {
                let reply = match r {
                    Request::StorageRead { key } => Reply::Value(store.get(&key).cloned()),
                    Request::StorageWrite { key, value } => {
                        store.insert(key, value);
                        Reply::Unit
                    }
                    Request::StorageRemove { key } => {
                        store.remove(&key);
                        Reply::Unit
                    }
                    Request::Balance { .. } => Reply::I64(0),
                    Request::Transfer { .. } => Reply::Status(1, vec![]),
                    Request::Call { .. } => Reply::Call { ok: false, data: b"no".to_vec(), gas_used: 0 },
                    Request::NativeRead { .. } => Reply::Value(None),
                    Request::NativeCall { .. } => Reply::Status(1, vec![]),
                };
                step = f.resume(reply);
            }
            other => {
                let used = gas - f.gas_left();
                return (other, used);
            }
        }
    }
}

fn input(method: &str, fields: &[u8]) -> Vec<u8> {
    let mut v = vec![method.len() as u8];
    v.extend_from_slice(method.as_bytes());
    v.extend_from_slice(fields);
    v
}

#[test]
fn examples_validate() {
    let vm = Vm::new();
    for (name, code) in [
        ("counter", &include_bytes!("../testdata/counter.wasm")[..]),
        ("escrow", &include_bytes!("../testdata/escrow.wasm")[..]),
        ("token", &include_bytes!("../testdata/token.wasm")[..]),
        ("bounty", &include_bytes!("../testdata/bounty.wasm")[..]),
        ("testkit", &include_bytes!("../testdata/testkit.wasm")[..]),
    ] {
        let (_, info) = vm.validate(code, MAX).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(info.entries.iter().any(|e| e == "call"), "{name}: {:?}", info.entries);
    }
}

#[test]
fn counter_runs_and_is_deterministic() {
    let vm = Vm::new();
    let code = include_bytes!("../testdata/counter.wasm");
    let (s, g1) = run(&vm, code, "instantiate", 41u64.to_le_bytes().to_vec(), 10_000_000, Mode::Execute);
    assert!(matches!(s, Step::Done { .. }), "{s:?}");
    // same inputs, same gas, every time
    let (_, g2) = run(&vm, code, "instantiate", 41u64.to_le_bytes().to_vec(), 10_000_000, Mode::Execute);
    assert_eq!(g1, g2);
    let (s, _) = run(&vm, code, "call", input("increment", &1u64.to_le_bytes()), 10_000_000, Mode::Execute);
    assert_eq!(s, Step::Done { ret: 1u64.to_le_bytes().to_vec() });
    let (s, _) = run(&vm, code, "call", input("nope", &[]), 10_000_000, Mode::Execute);
    assert_eq!(s, Step::Abort(Abort::Revert(b"unknown method".to_vec())));
}

#[test]
fn query_mode_cannot_write() {
    let vm = Vm::new();
    let code = include_bytes!("../testdata/testkit.wasm");
    let (s, _) = run(&vm, code, "query", input("write", &[]), 10_000_000, Mode::Query);
    assert!(matches!(s, Step::Abort(Abort::Trap(ref m)) if m.contains("state change in a query")), "{s:?}");
}

#[test]
fn malicious_modules_refused_at_deploy() {
    let vm = Vm::new();
    let cases: &[(&str, &str)] = &[
        ("floats", r#"(module (memory (export "memory") 1) (func (export "call") (drop (f32.add (f32.const 1) (f32.const 2)))))"#),
        ("float param", r#"(module (memory (export "memory") 1) (func (export "call")) (func (param f64)))"#),
        ("unknown import", r#"(module (import "cadre" "fs_open" (func)) (memory (export "memory") 1) (func (export "call")))"#),
        ("foreign module", r#"(module (import "env" "abort" (func)) (memory (export "memory") 1) (func (export "call")))"#),
        ("bad signature", r#"(module (import "cadre" "input_len" (func (param i32))) (memory (export "memory") 1) (func (export "call")))"#),
        ("imported memory", r#"(module (import "cadre" "mem" (memory 1)) (func (export "call")))"#),
        ("start function", r#"(module (memory (export "memory") 1) (func $s) (start $s) (func (export "call")))"#),
        ("no memory", r#"(module (func (export "call")))"#),
        ("no entry", r#"(module (memory (export "memory") 1) (func (export "main")))"#),
        ("entry with params", r#"(module (memory (export "memory") 1) (func (export "call") (param i32)))"#),
        ("huge memory", r#"(module (memory (export "memory") 65) (func (export "call")))"#),
    ];
    for (name, src) in cases {
        assert!(vm.validate(&wat(src), MAX).is_err(), "{name} was accepted");
    }
    assert!(vm.validate(b"not wasm", MAX).is_err());
    let ok = wat(r#"(module (memory (export "memory") 1) (func (export "call")))"#);
    assert!(vm.validate(&ok, MAX).is_ok());
    assert!(vm.validate(&ok, ok.len() - 1).is_err(), "size limit");
}

#[test]
fn runtime_attacks_stop_deterministically() {
    let vm = Vm::new();
    // infinite loop: out of gas, all gas used
    let lp = wat(r#"(module (memory (export "memory") 1) (func (export "call") (loop $l (br $l))))"#);
    let (s, used) = run(&vm, &lp, "call", vec![], 1_000_000, Mode::Execute);
    assert_eq!(s, Step::Abort(Abort::OutOfGas));
    assert_eq!(used, 1_000_000);
    // deep recursion: a stack overflow trap, not a host crash
    let rec = wat(r#"(module (memory (export "memory") 1) (func $f (call $f)) (func (export "call") (call $f)))"#);
    let (s, _) = run(&vm, &rec, "call", vec![], 100_000_000, Mode::Execute);
    assert_eq!(s, Step::Abort(Abort::Trap("trap: stack overflow".into())));
    // memory.grow past the cap fails (returns -1) instead of allocating
    let grow = wat(&format!(
        r#"(module (memory (export "memory") 1)
             (func (export "call") (if (i32.ne (memory.grow (i32.const {})) (i32.const -1)) (then unreachable))))"#,
        MAX_MEMORY_PAGES
    ));
    let (s, _) = run(&vm, &grow, "call", vec![], 1_000_000, Mode::Execute);
    assert!(matches!(s, Step::Done { .. }), "{s:?}");
    // out-of-bounds host pointer: a trap with a fixed message
    let oob = wat(r#"(module (import "cadre" "input_read" (func $r (param i32))) (memory (export "memory") 1)
             (func (export "call") (call $r (i32.const 70000))))"#);
    let (s, _) = run(&vm, &oob, "call", vec![1; 10], 1_000_000, Mode::Execute);
    assert_eq!(s, Step::Abort(Abort::Trap("host call: memory write out of bounds".into())));
    // a huge storage key is refused by the host
    let bigkey = wat(r#"(module (import "cadre" "storage_read" (func $r (param i32 i32) (result i32))) (memory (export "memory") 1)
             (func (export "call") (drop (call $r (i32.const 0) (i32.const 4096)))))"#);
    let (s, _) = run(&vm, &bigkey, "call", vec![], 1_000_000, Mode::Execute);
    assert_eq!(s, Step::Abort(Abort::Trap("host call: length out of range".into())));
    // a host call with too little gas left stops with out of gas
    let w = wat(r#"(module (import "cadre" "storage_write" (func $w (param i32 i32 i32 i32))) (memory (export "memory") 1)
             (func (export "call") (call $w (i32.const 0) (i32.const 8) (i32.const 0) (i32.const 8))))"#);
    let (s, used) = run(&vm, &w, "call", vec![], 1_000, Mode::Execute);
    assert_eq!(s, Step::Abort(Abort::OutOfGas));
    assert_eq!(used, 1_000);
    // emitting more than the per-tx event cap
    let ev = wat(r#"(module (import "cadre" "emit" (func $e (param i32 i32 i32 i32))) (memory (export "memory") 1)
             (func (export "call") (local $i i32) (loop $l (call $e (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0))
               (local.set $i (i32.add (local.get $i) (i32.const 1))) (br_if $l (i32.lt_u (local.get $i) (i32.const 100))))))"#);
    let (s, _) = run(&vm, &ev, "call", vec![], 100_000_000, Mode::Execute);
    assert_eq!(s, Step::Abort(Abort::Trap("host call: too many events in one tx".into())));
}

/// Gas per wall time, for the schedule (docs/CONTRACTS.md "Measured"). Run with
/// `cargo test --release -p contracts --test vm -- --ignored --nocapture measure`.
#[test]
#[ignore]
fn measure() {
    let vm = Vm::new();
    let code = include_bytes!("../testdata/testkit.wasm");
    let gas = 1_000_000_000u64;
    let t = std::time::Instant::now();
    let (s, used) = run(&vm, code, "call", input("burn", &5_000_000u64.to_le_bytes()), gas, Mode::Execute);
    let el = t.elapsed();
    println!("burn: {s:?} used {used} gas in {el:?}: {:.2} ns/gas", el.as_nanos() as f64 / used as f64);
    let lp = wat(r#"(module (memory (export "memory") 1) (func (export "call") (loop $l (br $l))))"#);
    let t = std::time::Instant::now();
    let (_, used) = run(&vm, &lp, "call", vec![], 500_000_000, Mode::Execute);
    let el = t.elapsed();
    println!("tight loop: {used} gas in {el:?}: {:.2} ns/gas", el.as_nanos() as f64 / used as f64);
    // other worst-case candidates: 64-bit division, bulk memory, indirect calls, host calls
    let cases = [
        ("i64.div_u loop", r#"(module (memory (export "memory") 1) (func (export "call") (local $x i64) (local.set $x (i64.const -1))
            (loop $l (local.set $x (i64.add (i64.div_u (local.get $x) (i64.const 3)) (i64.const -1))) (br $l))))"#),
        ("memory.fill 4 MiB loop", r#"(module (memory (export "memory") 64) (func (export "call")
            (loop $l (memory.fill (i32.const 0) (i32.const 7) (i32.const 4194304)) (br $l))))"#),
        ("memory.copy 2 MiB loop", r#"(module (memory (export "memory") 64) (func (export "call")
            (loop $l (memory.copy (i32.const 0) (i32.const 2097152) (i32.const 2097152)) (br $l))))"#),
        ("call_indirect loop", r#"(module (memory (export "memory") 1) (type $t (func)) (table 1 funcref) (elem (i32.const 0) $f) (func $f)
            (func (export "call") (loop $l (call_indirect (type $t) (i32.const 0)) (br $l))))"#),
        ("gas_left host call loop", r#"(module (import "cadre" "gas_left" (func $g (result i64))) (memory (export "memory") 1)
            (func (export "call") (loop $l (drop (call $g)) (br $l))))"#),
        ("blake3 4 KiB host call loop", r#"(module (import "cadre" "blake3" (func $h (param i32 i32 i32))) (memory (export "memory") 1)
            (func (export "call") (loop $l (call $h (i32.const 0) (i32.const 4096) (i32.const 8192)) (br $l))))"#),
    ];
    for (name, src) in cases {
        let m = wat(src);
        let t = std::time::Instant::now();
        let (s, used) = run(&vm, &m, "call", vec![], 200_000_000, Mode::Execute);
        let el = t.elapsed();
        println!("{name}: {s:?} {used} gas in {el:?}: {:.2} ns/gas", el.as_nanos() as f64 / used as f64);
    }
    let t = std::time::Instant::now();
    for _ in 0..100 {
        vm.validate(code, MAX).unwrap();
    }
    let el = t.elapsed() / 100;
    println!("validate {} bytes: {el:?} = {:.1} ns/byte", code.len(), el.as_nanos() as f64 / code.len() as f64);
    let (module, _) = vm.validate(code, MAX).unwrap();
    let t = std::time::Instant::now();
    for _ in 0..1000 {
        let _ = vm.frame(&module, env(), Mode::Execute, vec![], 1_000_000, 0).ok().unwrap();
    }
    let el = t.elapsed() / 1000;
    println!("instantiate cached module of {} bytes: {el:?} = {:.1} ns/byte", code.len(), el.as_nanos() as f64 / code.len() as f64);
}
