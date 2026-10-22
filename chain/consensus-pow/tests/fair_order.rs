//! Auto-choice job order (consensus_pow::chain::fair_job_order, SPEC 4.5 "Segment order"):
//! least-served ticket first, then oldest. Node policy, not consensus.

use std::collections::HashMap;

use consensus_pow::chain::{fair_job_order, job_progress_ppm};
use state::schema::{JobExt, JobRecord, JobStatus, TicketExt};

fn job(index: u64, cap: u64, max_blocks: u32, mined: u32, ext: Option<JobExt>) -> JobRecord {
    JobRecord {
        job_id: [index as u8; 32],
        poster: [0; 32],
        m: 64,
        n: 64,
        k: 2048,
        config: [0; 52],
        hash_a: [0; 32],
        hash_b: [0; 32],
        data_uri: Vec::new(),
        worker_pay: 0,
        protocol_fee: 0,
        deadline_height: 1000,
        max_blocks,
        evaluators: Vec::new(),
        threshold: 1,
        created_height: 1,
        terms_hash: [0; 32],
        status: JobStatus::Open,
        blocks_mined: mined,
        cap_remaining: cap,
        subsidy_paid: 0,
        deliverer: [0; 32],
        result_hash: [0; 32],
        result_uri: Vec::new(),
        closed_height: 0,
        index,
        header_id: [0; 32],
        ext,
    }
}

fn seg(index: u64, ticket: u8, cap: u64) -> JobRecord {
    job(index, cap, 1, 0, Some(JobExt::Segment { ticket_id: [ticket; 32], model_id: [9; 32], matrix: index as u16 }))
}

fn ticket(allocated: u32, mined: u32) -> TicketExt {
    TicketExt { segments: allocated, blocks_allocated: allocated, blocks_mined: mined, ..Default::default() }
}

/// The devnet case (2026-10-22): a 48-row ticket's segments carry a 33x smaller cap than a
/// 1,600-row round's, so "largest cap first" never chose them.
#[test]
fn small_ticket_is_not_crowded_out_by_big_rounds() {
    let mut tickets = HashMap::new();
    tickets.insert([1u8; 32], ticket(5760, 1200)); // big balance ticket: 20.8% mined
    tickets.insert([2u8; 32], ticket(144, 11)); // small ticket: 7.6% mined
    let mut jobs = vec![seg(100, 1, 33_000), seg(101, 1, 33_000), seg(50, 2, 1_000), seg(51, 2, 1_000)];
    fair_job_order(&mut jobs, &|t| tickets.get(t).copied());
    assert_eq!(jobs.iter().map(|j| j.index).collect::<Vec<_>>(), vec![50, 51, 100, 101]);
    // under the old rule the big round's segments came first
    let mut old = jobs.clone();
    old.sort_by(|a, b| b.cap_remaining.cmp(&a.cap_remaining).then(a.index.cmp(&b.index)));
    assert_eq!(old[0].index, 100);
}

#[test]
fn equal_progress_goes_oldest_first_and_plain_jobs_take_their_turn() {
    let mut tickets = HashMap::new();
    tickets.insert([1u8; 32], ticket(288, 0));
    tickets.insert([2u8; 32], ticket(144, 0));
    // a plain job with 3 of 10 blocks mined (30%), two fresh tickets (0%)
    let mut jobs = vec![job(7, 9_999_999, 10, 3, None), seg(20, 2, 10), seg(12, 1, 10), seg(30, 1, 10)];
    fair_job_order(&mut jobs, &|t| tickets.get(t).copied());
    assert_eq!(jobs.iter().map(|j| j.index).collect::<Vec<_>>(), vec![12, 20, 30, 7]);
    assert_eq!(job_progress_ppm(&jobs[3], None), 300_000);
}

/// Round robin falls out of the rule: each block mined on a ticket raises its share, so the
/// next template prefers the other ticket.
#[test]
fn mining_alternates_between_tickets_of_equal_size() {
    let mut tickets: HashMap<[u8; 32], TicketExt> = HashMap::new();
    tickets.insert([1u8; 32], ticket(4, 0));
    tickets.insert([2u8; 32], ticket(4, 0));
    let mut open = vec![seg(1, 1, 5), seg(2, 1, 5), seg(3, 1, 5), seg(4, 1, 5), seg(5, 2, 5), seg(6, 2, 5), seg(7, 2, 5), seg(8, 2, 5)];
    let mut mined = Vec::new();
    while !open.is_empty() {
        fair_job_order(&mut open, &|t| tickets.get(t).copied());
        let j = open.remove(0);
        let (tid, _, _) = j.segment().unwrap();
        tickets.get_mut(&tid).unwrap().blocks_mined += 1;
        mined.push(j.index);
    }
    assert_eq!(mined, vec![1, 5, 2, 6, 3, 7, 4, 8]);
}
