//! cadre-glue-v1 integer tables and primitives (SPEC 4.7). The Python reference prints the same
//! values (tools/cadre/test_cadre.py test_glue_tables; tools/cadre/test_replay.py compares the
//! sha256 of every table on the real model: 757b2c06...).

use cadre_glue::*;

#[test]
fn exp2_table_endpoints_and_accuracy() {
    let e = exp2_table();
    assert_eq!((e[0], e[4096], e.len()), (1 << 30, 1 << 29, 4097));
    for i in [1usize, 777, 2048, 4095] {
        let want = 2f64.powf(-(i as f64) / 4096.0);
        assert!((e[i] as f64 / (1u64 << 30) as f64 - want).abs() < 2e-9, "E2[{i}]");
    }
}

#[test]
fn rope_table_matches_cos_sin() {
    let f = [1.0f32, 0.25, 3e-7];
    let mut t = RopeTable::new(&f).unwrap();
    t.ensure(4096);
    assert_eq!(t.cos[0], vec![1 << 30; 3]);
    for p in [1usize, 100, 4095] {
        for (i, &x) in f.iter().enumerate() {
            let a = p as f64 * x as f64;
            assert!((t.cos[p][i] as f64 / (1u64 << 30) as f64 - a.cos()).abs() < 3e-9);
            assert!((t.sin[p][i] as f64 / (1u64 << 30) as f64 - a.sin()).abs() < 3e-9);
        }
    }
}

#[test]
fn constants() {
    assert!((softmax_l(64) as f64 / (1u64 << 24) as f64 - std::f64::consts::LOG2_E / 8.0).abs() < 1e-7);
    assert!((LOG2E_Q24 as f64 / (1u64 << 24) as f64 - std::f64::consts::LOG2_E).abs() < 1e-7);
    assert!(f32_to_q60(0.3).is_ok());
    assert!(f32_to_q60(1.5).is_err());
    assert_eq!(rne_clamp(2.5, 10.0), 2);
    assert_eq!(rne_clamp(3.5, 10.0), 4);
    assert_eq!(rne_clamp(-2.5, 10.0), -2);
    assert_eq!(rne_clamp(1e300, 7.0), 7);
}

#[test]
fn gemm_is_exact() {
    let (m, k, n) = (5usize, 64usize, 33usize);
    let a: Vec<i8> = (0..m * k).map(|i| ((i * 37 % 129) as i64 - 64) as i8).collect();
    let bt: Vec<i8> = (0..n * k).map(|i| ((i * 91 % 129) as i64 - 64) as i8).collect();
    let c = gemm_bt(&a, m, k, &bt, n);
    for i in 0..m {
        for j in 0..n {
            let want: i32 = (0..k).map(|l| a[i * k + l] as i32 * bt[j * k + l] as i32).sum();
            assert_eq!(c[i * n + j], want);
        }
    }
}
