//! Speed schedule (docs/SPEED.md 3.5): the light client follows the chain's halved target
//! block time and doubled K from `speed_height`, and behaves exactly as before until an admin
//! sets a schedule. Build first: `cd token && anchor build -p lightclient`.

use lightclient::client;
use lightclient::{LcError, SpeedSchedule, SPEED_LEN, SPEED_OFFSET};
use lightclient_litesvm_tests::*;
use solana_sdk::signature::{Keypair, Signer};

/// 10 s base target (helpers' params), K 4; the schedule halves the target and doubles K.
const T1: u64 = 10_000;
const T2: u64 = 5_000;
const K1: u32 = 4;
const K2: u32 = 8;

fn small() -> lightclient::Params {
    let mut p = params(K1);
    p.fallback_m = 4;
    p.fallback_n = 8;
    p
}

/// Params with another target time (the chain generator's WTEMA reads it from here).
fn at(p: &lightclient::Params, t: u64) -> lightclient::Params {
    let mut q = *p;
    q.target_block_time_ms = t;
    q
}

/// Extend `from` by `n` blocks of `dt` ms, each mined at the chain's rule for its height:
/// the T1 WTEMA up to and including `s`, the T2 WTEMA above it.
fn extend(p: &lightclient::Params, from: &Blk, n: usize, dt: u64, s: u64, miner: u8) -> Vec<Blk> {
    let mut out: Vec<Blk> = Vec::new();
    for _ in 0..n {
        let parent = out.last().unwrap_or(from).clone();
        let t = if parent.h.height + 1 > s { T2 } else { T1 };
        out.push(child(&at(p, t), &parent, &Work::Fallback, ChildOpts { dt, miner, ..Default::default() }));
    }
    out
}

fn config_len(env: &Env) -> usize {
    env.svm.get_account(&client::config_pda()).unwrap().data.len()
}

fn speed(env: &Env) -> Option<SpeedSchedule> {
    SpeedSchedule::read(&env.svm.get_account(&client::config_pda()).unwrap().data)
}

#[test]
fn set_speed_grows_config_once_and_is_admin_only() {
    let p = small();
    let g = genesis(0x1e100000);
    let mut env = Env::new(p, &g);
    let before = env.config();
    assert_eq!(config_len(&env), SPEED_OFFSET, "an initialized config has no trailing schedule");
    assert_eq!(speed(&env), None);
    // only the admin
    let other = Keypair::new();
    env.svm.airdrop(&other.pubkey(), 1_000_000_000).unwrap();
    let e = env.send(&[client::set_speed(other.pubkey(), 10, T2, K2)], &[&other]).unwrap_err();
    assert!(custom(lc_code(LcError::NotAdmin))(&e), "{e:?}");
    // parameter bounds: K < CANON_LEN - 16, positive target, a height above the tip
    for (h, t, k) in [(10, T2, 0), (10, T2, (lightclient::CANON_LEN - 16) as u32), (10, 0, K2), (0, T2, K2)] {
        let e = env.send(&[client::set_speed(env.payer.pubkey(), h, t, k)], &[]).unwrap_err();
        assert!(custom(lc_code(LcError::BadParams))(&e), "({h}, {t}, {k}): {e:?}");
    }
    let lamports0 = env.svm.get_account(&client::config_pda()).unwrap().lamports;
    let cu = env.send(&[client::set_speed(env.payer.pubkey(), 10, T2, K2)], &[]).unwrap();
    println!("set_speed (first, realloc) CU {cu}");
    let acc = env.svm.get_account(&client::config_pda()).unwrap();
    assert_eq!(acc.data.len(), SPEED_OFFSET + SPEED_LEN);
    let rent_delta = acc.lamports - lamports0;
    println!("set_speed rent added: {rent_delta} lamports for {SPEED_LEN} bytes");
    assert_eq!(acc.lamports, env.svm.minimum_balance_for_rent_exemption(SPEED_OFFSET + SPEED_LEN));
    assert_eq!(speed(&env), Some(SpeedSchedule { speed_height: 10, target_block_time_ms: T2, finality_k: K2 }));
    // the Config itself is unchanged and still deserializes
    let after = env.config();
    assert_eq!((after.tip_height, after.tip_hash, after.params), (before.tip_height, before.tip_hash, before.params));
    // re-set while below the height: no second realloc
    env.send(&[client::set_speed(env.payer.pubkey(), 12, T2, K2)], &[]).unwrap();
    assert_eq!(config_len(&env), SPEED_OFFSET + SPEED_LEN);
    assert_eq!(speed(&env).unwrap().speed_height, 12);
    // other admin instructions keep the schedule (Anchor rewrites only the Config prefix)
    let mut q = p;
    q.relay_reward = 7;
    env.send(&[client::set_params(env.payer.pubkey(), q)], &[]).unwrap();
    assert_eq!(env.config().params.relay_reward, 7);
    assert_eq!(speed(&env).unwrap().speed_height, 12);
}

#[test]
fn headers_follow_the_schedule_at_and_after_speed_height() {
    let p = small();
    let g = genesis(0x1e100000);
    let s = 3u64;
    // 8 s solve times: under T1 the target hardens, under T2 it eases, so the rules differ
    let blks = extend(&p, &g, 6, 8_000, s, 0x4d);
    let mut env = Env::new(p, &g);
    // without a schedule, block S+1 (mined at T2) is refused once the chain gets there
    for b in &blks[..3] {
        env.relay(b).unwrap();
    }
    let e = env.submit(&blks[3]).unwrap_err();
    assert!(custom(lc_code(LcError::WrongTarget))(&e), "{e:?}");
    // the schedule cannot be set at or below the tip (S = 3 = tip): refused
    let e = env.send(&[client::set_speed(env.payer.pubkey(), s, T2, K2)], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::BadParams))(&e), "{e:?}");

    // fresh light client with the schedule set before the chain reaches S
    let mut env = Env::new(p, &g);
    env.send(&[client::set_speed(env.payer.pubkey(), s, T2, K2)], &[]).unwrap();
    // header S judged by the old rule: a T2-mined header at height S is refused
    let wrong_s = child(&at(&p, T2), &blks[1], &Work::Fallback, ChildOpts { dt: 8_000, miner: 0x01, ..Default::default() });
    assert_eq!(wrong_s.h.height, s);
    assert_ne!(wrong_s.h.target_bits, blks[2].h.target_bits);
    for b in &blks[..2] {
        env.relay(b).unwrap();
    }
    let e = env.submit(&wrong_s).unwrap_err();
    assert!(custom(lc_code(LcError::WrongTarget))(&e), "{e:?}");
    env.relay(&blks[2]).unwrap();
    // S+1 judged by T2: a T1-mined header is refused, the T2 one verifies
    let wrong_s1 = child(&at(&p, T1), &blks[2], &Work::Fallback, ChildOpts { dt: 8_000, miner: 0x02, ..Default::default() });
    assert_ne!(wrong_s1.h.target_bits, blks[3].h.target_bits);
    let e = env.submit(&wrong_s1).unwrap_err();
    assert!(custom(lc_code(LcError::WrongTarget))(&e), "{e:?}");
    for b in &blks[3..] {
        env.relay(b).unwrap();
        assert_eq!(env.config().tip_hash, b.hash);
    }
    // no longer changeable once the tip passed S
    let e = env.send(&[client::set_speed(env.payer.pubkey(), 100, T2, K2)], &[]).unwrap_err();
    assert!(custom(lc_code(LcError::BadParams))(&e), "{e:?}");
}

#[test]
fn final_floor_uses_the_new_k_from_speed_height() {
    let p = small();
    let g = genesis(0x1e100000);
    let s = 6u64;
    let blks = extend(&p, &g, 16, 10_000, s, 0x4d);
    let mut env = Env::new(p, &g);
    env.send(&[client::set_speed(env.payer.pubkey(), s, T2, K2)], &[]).unwrap();
    let mut floors = Vec::new();
    for b in &blks {
        env.relay(b).unwrap();
        floors.push((b.h.height, env.config().final_floor));
    }
    println!("tip -> final_floor: {floors:?}");
    for (tip, floor) in floors {
        let want = if tip < s {
            tip.saturating_sub(K1 as u64)
        } else {
            // monotone: the floor reached under K1 (s - 1 - K1) holds until tip - K2 passes it
            tip.saturating_sub(K2 as u64).max((s - 1).saturating_sub(K1 as u64))
        };
        assert_eq!(floor, want, "tip {tip}");
    }
    // a header at the floor cannot be marked final above it, below it can
    let c = env.config();
    assert_eq!(c.final_floor, 16 - K2 as u64);
    assert!(custom(lc_code(LcError::NotFinal))(&env.mark_final(&blks[(c.final_floor) as usize].hash).unwrap_err()));
    env.mark_final(&blks[(c.final_floor - 1) as usize].hash).unwrap();
}

#[test]
fn reorg_across_speed_height_uses_both_rules() {
    let p = small();
    let g = genesis(0x1e100000);
    let s = 4u64;
    let main = extend(&p, &g, 6, 10_000, s, 0x4d);
    let mut env = Env::new(p, &g);
    env.send(&[client::set_speed(env.payer.pubkey(), s, T2, K2)], &[]).unwrap();
    for b in &main {
        env.relay(b).unwrap();
    }
    assert_eq!(env.config().tip_height, 6);
    // a heavier branch from height 2: heights 3, 4 (= S, old rule) and 5..8 (new rule)
    let side = extend(&p, &main[1], 6, 9_000, s, 0x60);
    for b in &side {
        env.relay(b).unwrap();
    }
    let new_tip = side.last().unwrap();
    let mut path: Vec<[u8; 32]> = side[..side.len() - 1].iter().rev().map(|b| b.hash).collect();
    path.push(main[1].hash);
    let c0 = env.config();
    if c0.tip_hash != new_tip.hash {
        env.send(&[client::reorg(&new_tip.hash, &path)], &[]).unwrap();
    }
    let c = env.config();
    assert_eq!((c.tip_hash, c.tip_height), (new_tip.hash, 8));
    // K1 below S (the highest tip under it was 3: floor 0), K2 from S: 8 - 8 = 0
    assert_eq!(c.final_floor, 8u64.saturating_sub(K2 as u64).max((s - 1).saturating_sub(K1 as u64)));
}

#[test]
fn submit_header_cu_with_and_without_a_schedule() {
    let p = small();
    let g = genesis(0x1e100000);
    let blks = extend(&p, &g, 4, 10_000, 2, 0x4d);
    let mut env = Env::new(p, &g);
    let plain = extend(&p, &g, 4, 10_000, u64::MAX, 0x4d);
    let cu_plain: Vec<u64> = plain.iter().map(|b| {
        let cu = env.submit(b).unwrap();
        env.upload(b).unwrap();
        let plan = env.plan(b);
        env.steps(b, &plan).unwrap();
        cu
    }).collect();
    let mut env = Env::new(p, &g);
    env.send(&[client::set_speed(env.payer.pubkey(), 2, T2, K2)], &[]).unwrap();
    let cu_sched: Vec<u64> = blks.iter().map(|b| {
        let cu = env.submit(b).unwrap();
        env.upload(b).unwrap();
        let plan = env.plan(b);
        env.steps(b, &plan).unwrap();
        cu
    }).collect();
    println!("submit_header CU, no schedule: {cu_plain:?}; with schedule (S = 2): {cu_sched:?}");
    let max = cu_plain.iter().chain(&cu_sched).max().unwrap();
    assert!(*max < 200_000, "submit_header stays small: {max}");
    for (l, c) in &env.cu_log {
        assert!(*c < 1_400_000, "{l} over the limit");
    }
}
