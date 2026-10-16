//! Full M1 share + delivery flow against the real jobs / mining programs, with every
//! service-side transaction built by pearl-chain (raw bytes, no solana-sdk), and every
//! account decoded by pearl-chain's decoders.
//!
//! post_job (real hash_a/hash_b/config) -> decode job -> mine shares -> attester
//! signs -> submit_share -> replay rejected -> stolen share rejected -> submit_result
//! -> attester Freivalds + sign -> complete (W paid) -> epoch advance -> claim.

use anchor_lang::{InstructionData, ToAccountMetas};
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use litesvm::LiteSVM;
use serde_json::json;
use solana_sdk::{
    instruction::Instruction as SdkIx,
    pubkey::Pubkey as SdkPk,
    signature::{Keypair, Signer},
    system_program,
    transaction::{Transaction, VersionedTransaction},
};

use pearl_attester::{Attester, JobSource};
use pearl_chain::accounts::{EpochAccount, JobAccount, JobStatus, MiningConfigAccount};
use pearl_chain::ix::Programs;
use pearl_chain::tx::{build_signed, ed25519_verify_ix};
use pearl_chain::Pubkey;
use pearl_job::freivalds::{c_to_bytes, result_hash};
use pearl_job::{data, MiningConfig, PreparedJob};

const SOL: u64 = 1_000_000_000;
const EASY: u32 = 0x1e040000;

fn so(name: &str) -> String {
    format!("{}/../../agent/target/deploy/{name}.so", env!("CARGO_MANIFEST_DIR"))
}

fn pk(p: &SdkPk) -> Pubkey {
    Pubkey(p.to_bytes())
}

fn spk(p: &Pubkey) -> SdkPk {
    SdkPk::new_from_array(p.0)
}

fn dalek(k: &Keypair) -> SigningKey {
    SigningKey::from_bytes(&k.to_bytes()[..32].try_into().unwrap())
}

fn send_sdk(svm: &mut LiteSVM, ixs: &[SdkIx], payer: &Keypair) {
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &[payer], svm.latest_blockhash());
    svm.send_transaction(tx).map_err(|e| format!("{:?}\n{}", e.err, e.meta.logs.join("\n"))).unwrap();
    svm.expire_blockhash();
}

/// Send a pearl-chain built transaction (raw wire bytes).
fn send_raw(svm: &mut LiteSVM, ixs: &[pearl_chain::tx::Instruction], payer: &SigningKey) -> Result<u64, String> {
    let bh = svm.latest_blockhash().to_bytes();
    let raw = build_signed(ixs, &[payer], bh).map_err(|e| e.to_string())?;
    let tx: VersionedTransaction = bincode::deserialize(&raw).map_err(|e| e.to_string())?;
    let r = svm.send_transaction(tx);
    svm.expire_blockhash();
    r.map(|m| m.compute_units_consumed).map_err(|e| format!("{:?}\n{}", e.err, e.meta.logs.join("\n")))
}

fn raw_account(svm: &LiteSVM, p: &Pubkey) -> Vec<u8> {
    svm.get_account(&spk(p)).expect("account").data
}

#[test]
fn share_and_delivery_flow_with_pearl_chain_transactions() {
    let programs = Programs::default();
    assert_eq!(programs.jobs.0, jobs::ID.to_bytes());
    assert_eq!(programs.mining.0, mining::ID.to_bytes());

    let mut svm = LiteSVM::new();
    svm.add_program_from_file(jobs::ID, so("jobs")).unwrap();
    svm.add_program_from_file(mining::ID, so("mining")).unwrap();
    svm.warp_to_slot(1_000);
    let admin = Keypair::new();
    let poster = Keypair::new();
    let miner_kp = Keypair::new();
    let thief_kp = Keypair::new();
    for k in [&admin, &poster, &miner_kp, &thief_kp] {
        svm.airdrop(&k.pubkey(), 100 * SOL).unwrap();
    }
    let attester_key = SigningKey::from_bytes(&[9u8; 32]);
    let attester_pk = SdkPk::new_from_array(attester_key.verifying_key().to_bytes());

    // --- setup with the programs' own Anchor types ---
    let mcfg = SdkPk::new_from_array(programs.mining_config().0);
    let pool = SdkPk::new_from_array(programs.mining_pool().0);
    let ix = SdkIx {
        program_id: mining::ID,
        accounts: mining::accounts::Initialize {
            admin: admin.pubkey(),
            config: mcfg,
            mining_pool: pool,
            epoch: spk(&programs.epoch(0)),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: mining::instruction::Initialize {
            params: mining::ConfigParams {
                attesters: Some(vec![attester_pk]),
                epoch_slots: Some(1_000),
                max_nonce: Some(64),
                ..Default::default()
            },
        }
        .data(),
    };
    send_sdk(&mut svm, &[ix], &admin);
    let ix = SdkIx {
        program_id: jobs::ID,
        accounts: jobs::accounts::InitializeConfig {
            admin: admin.pubkey(),
            config: spk(&programs.jobs_config()),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: jobs::instruction::InitializeConfig {
            params: jobs::ConfigParams { evaluators: Some(vec![attester_pk]), ..Default::default() },
        }
        .data(),
    };
    send_sdk(&mut svm, &[ix], &admin);
    // seed the pool so the epoch-1 budget is non-zero
    let mut pa = svm.get_account(&pool).unwrap();
    pa.lamports += 1_000 * SOL;
    svm.set_account(pool, pa).unwrap();

    // --- a real job: matrices, header, commitments ---
    let (m, n, k) = (64u32, 64u32, 1024u32);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("e2e-job");
    let (a, b) = data::random_matrices(m as usize, n as usize, k as usize, Some(5));
    let manifest = data::save(&dir, m, n, k, &a, &b).unwrap();
    let job_id = 7u64;
    let job_pda = SdkPk::find_program_address(&[b"job", poster.pubkey().as_ref(), &job_id.to_le_bytes()], &jobs::ID).0;
    let config = MiningConfig::localnet_default();
    let created_slot = 1_000u64;
    let mut params = jobs::PostJobParams {
        job_id,
        hash_a: [0; 32],
        hash_b: [0; 32],
        config: config.to_bytes(),
        m,
        n,
        k,
        data_uri: format!("file://{}", manifest.display()),
        worker_pay: 2 * SOL,
        protocol_fee: SOL / 2,
        created_slot,
        deadline_slot: created_slot + 5_000,
        share_target: EASY,
    };
    let terms = jobs::terms_hash(&poster.pubkey(), &params);
    let header = pearl_job::JobHeader { job_pubkey: job_pda.to_bytes(), terms_hash: terms, created_slot, target_bits: EASY };
    let key = pearl_job::job_key(&header, &config);
    let (ha, hb) = pearl_job::commit(&key, &a, &b, k as usize, n as usize);
    params.hash_a = ha;
    params.hash_b = hb;
    let ix = SdkIx {
        program_id: jobs::ID,
        accounts: jobs::accounts::PostJob {
            poster: poster.pubkey(),
            config: spk(&programs.jobs_config()),
            job: job_pda,
            mining_pool: pool,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: jobs::instruction::PostJob { p: params }.data(),
    };
    send_sdk(&mut svm, &[ix], &poster);

    // --- decode with pearl-chain ---
    let job_addr = pk(&job_pda);
    let ja = JobAccount::decode(job_addr, &raw_account(&svm, &job_addr)).unwrap();
    assert_eq!(ja.status, JobStatus::Open);
    assert_eq!(ja.header, header.to_bytes(), "program header == pearl-job header");
    let mc = MiningConfigAccount::decode(&raw_account(&svm, &programs.mining_config())).unwrap();
    assert_eq!((mc.max_nonce, mc.current_epoch, mc.attesters.len()), (64, 0, 1));
    let ep = EpochAccount::decode(&raw_account(&svm, &programs.epoch(0))).unwrap();
    assert_eq!(ep.index, 0);
    let spec = ja.to_spec(mc.max_nonce as u64).unwrap();
    let prep = PreparedJob::new(spec.to_job().unwrap(), &a, &b).expect("on-chain commitments match the data");

    // --- mine, attest, submit ---
    let miner = dalek(&miner_kp);
    let miner_pk = pearl_chain::key::pubkey_of(&miner);
    let att = Attester { key: attester_key.clone(), source: JobSource::Request, freivalds_rounds: 2 };
    let mut accepted = 0u64;
    let mut first_sig = None;
    for nonce in 0..16u64 {
        let Some(p) = prep.mine_nonce(&miner_pk.0, nonce, EASY) else { continue };
        let (st, v) = att.route(
            "POST",
            "/share",
            json!({"job": job_addr.to_b58(), "miner": miner_pk.to_b58(),
                   "proof_b64": base64::engine::general_purpose::STANDARD.encode(p.to_bytes()), "job_spec": spec})
            .to_string()
            .as_bytes(),
        );
        assert_eq!(st, 200, "{v}");
        let sig: [u8; 64] = hex::decode(v["signature_hex"].as_str().unwrap()).unwrap().try_into().unwrap();
        let msg = pearl_job::messages::share_message(&job_addr.0, &miner_pk.0, nonce);
        let ixs = [ed25519_verify_ix(&attester_key.verifying_key().to_bytes(), &sig, &msg),
                   programs.submit_share_ix(miner_pk, job_addr, 0, nonce)];
        let cu = send_raw(&mut svm, &ixs, &miner).unwrap();
        println!("submit_share nonce {nonce}: {cu} CU");
        accepted += 1;
        if first_sig.is_none() {
            first_sig = Some((nonce, sig));
        }
    }
    assert!(accepted >= 3, "only {accepted} shares in 16 nonces");
    let shares_pda = Pubkey(SdkPk::find_program_address(
        &[b"shares", &0u64.to_le_bytes(), &job_pda.to_bytes(), &miner_pk.0],
        &mining::ID,
    ).0.to_bytes());
    let sh = raw_account(&svm, &shares_pda);
    let count = u64::from_le_bytes(sh[8 + 32 + 32 + 8..8 + 32 + 32 + 16].try_into().unwrap());
    assert_eq!(count, accepted);

    // replay of an accepted share fails
    let (n0, s0) = first_sig.unwrap();
    let msg = pearl_job::messages::share_message(&job_addr.0, &miner_pk.0, n0);
    let ixs = [ed25519_verify_ix(&attester_key.verifying_key().to_bytes(), &s0, &msg),
               programs.submit_share_ix(miner_pk, job_addr, 0, n0)];
    assert!(send_raw(&mut svm, &ixs, &miner).is_err());
    // a thief cannot submit the miner's signed share as their own
    let thief = dalek(&thief_kp);
    let thief_pk = pearl_chain::key::pubkey_of(&thief);
    let ixs = [ed25519_verify_ix(&attester_key.verifying_key().to_bytes(), &s0, &msg),
               programs.submit_share_ix(thief_pk, job_addr, 0, n0)];
    assert!(send_raw(&mut svm, &ixs, &thief).is_err());

    // --- deliver C = AB ---
    let c = prep.product();
    let rh = result_hash(&prep.job_key, &c);
    let cpath = dir.join("C.i32");
    data::write_i32(&cpath, &c).unwrap();
    let uri = format!("file://{}", cpath.display());
    send_raw(&mut svm, &[programs.submit_result_ix(miner_pk, job_addr, rh, &uri)], &miner).unwrap();
    let ja = JobAccount::decode(job_addr, &raw_account(&svm, &job_addr)).unwrap();
    assert_eq!((ja.status, ja.deliverer, ja.result_hash, ja.result_uri.as_str()), (JobStatus::Delivered, miner_pk, rh, uri.as_str()));
    let (st, v) = att.route(
        "POST",
        "/result",
        json!({"job": job_addr.to_b58(), "deliverer": miner_pk.to_b58(),
               "result_b64": base64::engine::general_purpose::STANDARD.encode(c_to_bytes(&c)), "job_spec": spec})
        .to_string()
        .as_bytes(),
    );
    assert_eq!(st, 200, "{v}");
    let sig: [u8; 64] = hex::decode(v["signature_hex"].as_str().unwrap()).unwrap().try_into().unwrap();
    let before = svm.get_balance(&miner_kp.pubkey()).unwrap();
    let msg = pearl_job::messages::result_message(&job_addr.0, &miner_pk.0, &rh);
    let ixs = [ed25519_verify_ix(&attester_key.verifying_key().to_bytes(), &sig, &msg), programs.complete_ix(job_addr, miner_pk)];
    send_raw(&mut svm, &ixs, &miner).unwrap();
    let after = svm.get_balance(&miner_kp.pubkey()).unwrap();
    // fee: 5000 for the tx signature + 5000 for the ed25519 precompile signature
    assert_eq!(after + 10_000, before + 2 * SOL, "W paid to the deliverer, minus fees");
    let ja = JobAccount::decode(job_addr, &raw_account(&svm, &job_addr)).unwrap();
    assert_eq!(ja.status, JobStatus::Completed);

    // --- epoch close, advance, claim ---
    svm.warp_to_slot(ep.end_slot);
    send_raw(&mut svm, &[programs.advance_epoch_ix(miner_pk, 0)], &miner).unwrap();
    let mc = MiningConfigAccount::decode(&raw_account(&svm, &programs.mining_config())).unwrap();
    assert_eq!(mc.current_epoch, 1);
    let before = svm.get_balance(&miner_kp.pubkey()).unwrap();
    send_raw(&mut svm, &[programs.claim_ix(miner_pk, job_addr, 0)], &miner).unwrap();
    let after = svm.get_balance(&miner_kp.pubkey()).unwrap();
    println!("claimed {} lamports (net of fee, plus Shares rent back) for {accepted} shares", after as i128 - before as i128);
    assert!(after > before, "claim paid the miner");
}
