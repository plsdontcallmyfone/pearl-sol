//! Published test vectors for the TypeScript wallet (chain/agent-types/vectors/*.json).
//!
//! `UPDATE_VECTORS=1 cargo test -p agent-types --test vectors` rewrites the files;
//! a plain run checks that the encoder still produces them byte for byte.

use agent_types::block::encode_header_with_proof;
use agent_types::tx::{deposit_message, order_job_id, order_message, pubkey_from_seed, reject_message, result_message, route_clear_message, sign_plain};
use agent_types::*;
use serde_json::{json, Value};

const CHAIN_ID: u32 = 0x5053_0001;

fn seed(i: u8) -> [u8; 32] {
    blake3_hash(&[b"pearl-sol:vector-seed", &[i]])
}

fn b58(k: &[u8]) -> String {
    bs58::encode(k).into_string()
}

fn sample_job() -> PostJob {
    let mut config = [0u8; 52];
    config[0..4].copy_from_slice(&1024u32.to_le_bytes());
    config[4..6].copy_from_slice(&32u16.to_le_bytes());
    PostJob {
        m: 64,
        n: 64,
        k: 1024,
        config,
        hash_a: [0xaa; 32],
        hash_b: [0xbb; 32],
        data_uri: b"ipfs://example-job".to_vec(),
        worker_pay: 5_000_000_000,
        protocol_fee: 1_000_000_000,
        deadline_height: 1000,
        max_blocks: 10,
        evaluators: vec![pubkey_from_seed(&seed(20)), pubkey_from_seed(&seed(21))],
        threshold: 1,
    }
}

fn job_json(p: &PostJob) -> Value {
    json!({
        "m": p.m, "n": p.n, "k": p.k,
        "config": hex::encode(p.config),
        "hash_a": hex::encode(p.hash_a), "hash_b": hex::encode(p.hash_b),
        "data_uri": String::from_utf8_lossy(&p.data_uri),
        "worker_pay": p.worker_pay.to_string(), "protocol_fee": p.protocol_fee.to_string(),
        "deadline_height": p.deadline_height, "max_blocks": p.max_blocks,
        "evaluators": p.evaluators.iter().map(|e| b58(e)).collect::<Vec<_>>(),
        "threshold": p.threshold,
    })
}

fn sigs_json(s: &[AttesterSig]) -> Value {
    Value::Array(s.iter().map(|a| json!({"pubkey": b58(&a.pubkey), "sig": hex::encode(a.sig)})).collect())
}

fn body_json(b: &TxBody) -> Value {
    match b {
        TxBody::Transfer { to, amount } => json!({"to": b58(to), "amount": amount.to_string()}),
        TxBody::RegisterAgent { name, uri } => {
            json!({"name": String::from_utf8_lossy(name), "uri": String::from_utf8_lossy(uri)})
        }
        TxBody::PostJob(p) => job_json(p),
        TxBody::SubmitResult { job_id, result_hash, uri } => json!({
            "job_id": hex::encode(job_id), "result_hash": hex::encode(result_hash),
            "uri": String::from_utf8_lossy(uri)}),
        TxBody::CompleteJob { job_id, sigs } => json!({"job_id": hex::encode(job_id), "sigs": sigs_json(sigs)}),
        TxBody::ExpireJob { job_id } => json!({"job_id": hex::encode(job_id)}),
        TxBody::BridgeUnlock { solana_nonce, recipient, amount_spl, job_order, sigs } => json!({
            "solana_nonce": solana_nonce, "recipient": b58(recipient), "amount_spl": amount_spl.to_string(),
            "job_order": job_order.as_ref().map(job_json), "sigs": sigs_json(sigs)}),
        TxBody::BridgeLock { solana_recipient, amount } => {
            json!({"solana_recipient": b58(solana_recipient), "amount": amount.to_string()})
        }
        TxBody::AdminConfig { key, value } => json!({"key": key, "value": hex::encode(value)}),
        TxBody::SetRoute { solana_recipient, route_bps, until_height, cap, release_key } => json!({
            "solana_recipient": b58(solana_recipient), "route_bps": route_bps, "until_height": until_height,
            "cap": cap.to_string(), "release_key": b58(release_key)}),
        TxBody::ClearRoute { account, route_nonce, sig } => json!({
            "account": b58(account), "route_nonce": route_nonce, "sig": hex::encode(sig)}),
        TxBody::RegisterModel(m) => json!({
            "name": String::from_utf8_lossy(&m.name), "manifest_uri": String::from_utf8_lossy(&m.manifest_uri),
            "recipe": m.recipe, "rank": m.rank, "rows_pattern": hex::encode(m.rows_pattern), "cols_pattern": hex::encode(m.cols_pattern),
            "matrices": m.matrices.iter().map(|x| json!({"k": x.k, "n": x.n, "hash_b": hex::encode(x.hash_b)})).collect::<Vec<_>>()}),
        TxBody::PostSegments { ticket_id, round, model_id, rank, rows_pattern, cols_pattern, segments } => json!({
            "ticket_id": hex::encode(ticket_id), "round": round, "model_id": hex::encode(model_id), "rank": rank,
            "rows_pattern": hex::encode(rows_pattern), "cols_pattern": hex::encode(cols_pattern),
            "segments": segments.iter().map(|g| json!({"matrix": g.matrix, "m": g.m, "k": g.k, "n": g.n, "max_blocks": g.max_blocks,
                "hash_a": hex::encode(g.hash_a), "hash_b": hex::encode(g.hash_b), "c_hash": hex::encode(g.c_hash), "a_uri": String::from_utf8_lossy(&g.a_uri)})).collect::<Vec<_>>()}),
        TxBody::RejectResult { job_id, deliverer, result_hash, sigs } => json!({
            "job_id": hex::encode(job_id), "deliverer": b58(deliverer), "result_hash": hex::encode(result_hash),
            "sigs": sigs_json(sigs)}),
        TxBody::SetProfile(p) => json!({
            "handle": String::from_utf8_lossy(&p.handle), "name": String::from_utf8_lossy(&p.name),
            "profile_uri": String::from_utf8_lossy(&p.profile_uri), "profile_hash": hex::encode(p.profile_hash),
            "flags": p.flags, "operator": b58(&p.operator), "enc_key": b58(&p.enc_key), "dm_price": p.dm_price.to_string(),
            "dm_policy": p.dm_policy, "thread_free_blocks": p.thread_free_blocks}),
        TxBody::OperatorLink { agent, add } => json!({"agent": b58(agent), "add": add}),
        TxBody::Post(p) => json!({
            "kind": p.kind, "parent": hex::encode(p.parent), "channel": hex::encode(p.channel),
            "body": String::from_utf8_lossy(&p.body), "blob_uri": String::from_utf8_lossy(&p.blob_uri),
            "blob_hash": hex::encode(p.blob_hash), "thought_ref": String::from_utf8_lossy(&p.thought_ref)}),
        TxBody::React { items } => json!({"items": items.iter().map(|x| json!({
            "post_id": hex::encode(x.post_id), "reaction": x.reaction, "tip": x.tip.to_string()})).collect::<Vec<_>>()}),
        TxBody::Follow { items } => json!({"items": items.iter().map(|x| json!({"target": b58(&x.target), "on": x.on})).collect::<Vec<_>>()}),
        TxBody::SendMessage(m) => json!({
            "to": b58(&m.to), "channel": hex::encode(m.channel), "mode": m.mode, "reply_to": hex::encode(m.reply_to),
            "pay": m.pay.to_string(), "reclaim_after": m.reclaim_after, "body_hex": hex::encode(&m.body),
            "blob_uri": String::from_utf8_lossy(&m.blob_uri), "blob_hash": hex::encode(m.blob_hash),
            "thought_ref": String::from_utf8_lossy(&m.thought_ref)}),
        TxBody::ReclaimMessage { message_id } => json!({"message_id": hex::encode(message_id)}),
        TxBody::SetConsent { items } => json!({"items": items.iter().map(|x| json!({"peer": b58(&x.peer), "state": x.state})).collect::<Vec<_>>()}),
        TxBody::ChannelConfig(c) => json!({
            "channel_id": hex::encode(c.channel_id), "name": String::from_utf8_lossy(&c.name), "about": String::from_utf8_lossy(&c.about),
            "policy": c.policy, "join_price": c.join_price.to_string(), "mode": c.mode,
            "admins": c.admins.iter().map(|a| b58(a)).collect::<Vec<_>>()}),
        TxBody::ChannelMember { channel, action, member, pay } => json!({
            "channel": hex::encode(channel), "action": action, "member": b58(member), "pay": pay.to_string()}),
        TxBody::ChannelKey { channel, epoch, envelopes } => json!({
            "channel": hex::encode(channel), "epoch": epoch,
            "envelopes": envelopes.iter().map(|e| json!({"member": b58(&e.member), "envelope": hex::encode(e.envelope)})).collect::<Vec<_>>()}),
        TxBody::DeployCode(d) => json!({"gas_limit": d.gas_limit.to_string(), "code": hex::encode(&d.code),
            "code_hash": hex::encode(agent_types::contracts::code_hash(&d.code))}),
        TxBody::Instantiate(i) => json!({"code_hash": hex::encode(i.code_hash), "admin": b58(&i.admin), "value": i.value.to_string(),
            "gas_limit": i.gas_limit.to_string(), "label": String::from_utf8_lossy(&i.label), "args": hex::encode(&i.args)}),
        TxBody::Call(c) => json!({"contract": b58(&c.contract), "value": c.value.to_string(), "gas_limit": c.gas_limit.to_string(),
            "args": hex::encode(&c.args)}),
        TxBody::Migrate(m) => json!({"contract": b58(&m.contract), "code_hash": hex::encode(m.code_hash),
            "gas_limit": m.gas_limit.to_string(), "args": hex::encode(&m.args)}),
        TxBody::SetContractAdmin { contract, admin } => json!({"contract": b58(contract), "admin": b58(admin)}),
    }
}

fn tx_vector(name: &str, seed_i: u8, nonce: u64, fee: u64, body: TxBody) -> (Value, Tx) {
    let s = seed(seed_i);
    let tx = Tx::signed(&s, CHAIN_ID, nonce, fee, body);
    assert!(tx.verify_sig());
    assert_eq!(Tx::decode(&tx.encode()).unwrap(), tx);
    let v = json!({
        "name": name,
        "seed": hex::encode(s),
        "from": b58(&tx.from),
        "chain_id": CHAIN_ID,
        "nonce": nonce,
        "fee": fee.to_string(),
        "kind": tx.body.kind() as u8,
        "kind_name": tx.body.kind().name(),
        "body": body_json(&tx.body),
        "signing_bytes": hex::encode(tx.signing_bytes()),
        "framed_message": hex::encode(tx.framed_signing_message()),
        "sig": hex::encode(tx.sig),
        "tx": hex::encode(tx.encode()),
        "tx_hash": hex::encode(tx.hash()),
    });
    (v, tx)
}

fn signed_sig(seed_i: u8, msg: &[u8]) -> AttesterSig {
    let s = seed(seed_i);
    AttesterSig { pubkey: pubkey_from_seed(&s), sig: sign_plain(&s, msg) }
}

fn build() -> Vec<(&'static str, Value)> {
    let alice = pubkey_from_seed(&seed(1));
    let bob = pubkey_from_seed(&seed(2));
    let job = sample_job();
    let job_id = blake3_hash(&[&alice, &3u64.to_le_bytes()]);
    let result_hash = [0x5a; 32];
    let rmsg = result_message(&job_id, &bob, &result_hash);
    let dmsg = deposit_message(42, &alice, 1_000_000);
    // Storefront order: deadline is relative (blocks after the unlock height).
    let mut order_job = job.clone();
    order_job.deadline_height = 500;
    let omsg = order_message(43, &alice, 6_000_000, &order_job);
    let mut txs = Vec::new();
    let mut add = |v: (Value, Tx)| txs.push(v);
    add(tx_vector("transfer", 1, 0, 1000, TxBody::Transfer { to: bob, amount: 123_456_789 }));
    add(tx_vector(
        "register_agent",
        1,
        1,
        1000,
        TxBody::RegisterAgent { name: b"alice-agent".to_vec(), uri: b"https://example.com/agent.json".to_vec() },
    ));
    add(tx_vector("post_job", 1, 3, 1000, TxBody::PostJob(job.clone())));
    add(tx_vector(
        "submit_result",
        2,
        0,
        1000,
        TxBody::SubmitResult { job_id, result_hash, uri: b"ipfs://result".to_vec() },
    ));
    add(tx_vector(
        "complete_job",
        2,
        1,
        1000,
        TxBody::CompleteJob { job_id, sigs: vec![signed_sig(20, &rmsg)] },
    ));
    add(tx_vector("expire_job", 3, 0, 1000, TxBody::ExpireJob { job_id }));
    add(tx_vector(
        "bridge_unlock",
        4,
        0,
        0,
        TxBody::BridgeUnlock {
            solana_nonce: 42,
            recipient: alice,
            amount_spl: 1_000_000,
            job_order: None,
            sigs: vec![signed_sig(30, &dmsg), signed_sig(31, &dmsg)],
        },
    ));
    add(tx_vector(
        "bridge_unlock_order",
        4,
        1,
        0,
        TxBody::BridgeUnlock {
            solana_nonce: 43,
            recipient: alice,
            amount_spl: 6_000_000,
            job_order: Some(order_job.clone()),
            sigs: vec![signed_sig(30, &omsg)],
        },
    ));
    add(tx_vector(
        "bridge_lock",
        1,
        4,
        1000,
        TxBody::BridgeLock { solana_recipient: bob, amount: 500_000_000 },
    ));
    add(tx_vector(
        "admin_config",
        5,
        0,
        1000,
        TxBody::AdminConfig { key: 1, value: 2000u64.to_le_bytes().to_vec() },
    ));

    // Payout lien (R2): alice routes 25% of her income to bob's Solana key until height
    // 100000 or 1e12 base units, releasable early by seed(40)'s key; the release signature
    // clears route_nonce 0.
    let release = pubkey_from_seed(&seed(40));
    add(tx_vector(
        "set_route",
        1,
        5,
        1000,
        TxBody::SetRoute { solana_recipient: bob, route_bps: 2500, until_height: 100_000, cap: 1_000_000_000_000, release_key: release },
    ));
    add(tx_vector(
        "set_route_irrevocable",
        2,
        2,
        1000,
        TxBody::SetRoute { solana_recipient: alice, route_bps: 10_000, until_height: 5000, cap: 7_000_000, release_key: [0; 32] },
    ));
    let cmsg = route_clear_message(CHAIN_ID, &alice, 0);
    add(tx_vector(
        "clear_route",
        3,
        1,
        1000,
        TxBody::ClearRoute { account: alice, route_nonce: 0, sig: sign_plain(&seed(40), &cmsg) },
    ));

    let tx_objs: Vec<Tx> = txs.iter().map(|(_, t)| t.clone()).collect();
    let tx_json: Vec<Value> = txs.into_iter().map(|(v, _)| v).collect();

    let msg_v = |name: &str, domain: &[u8], msg: &[u8], seed_i: u8| {
        json!({"name": name, "domain": String::from_utf8_lossy(domain), "message": hex::encode(msg),
               "signing": "raw ed25519 over message (no framing)",
               "seed": hex::encode(seed(seed_i)), "pubkey": b58(&pubkey_from_seed(&seed(seed_i))),
               "sig": hex::encode(sign_plain(&seed(seed_i), msg))})
    };
    let mut order_v = msg_v("deposit_order", DOMAIN_DEPOSIT_ORDER, &omsg, 30);
    order_v["post_job_body"] = json!(hex::encode(order_job.encode()));
    order_v["post_job_body_sha256"] = json!(hex::encode(order_job.body_hash()));
    order_v["solana_nonce"] = json!(43);
    order_v["job_id"] = json!(hex::encode(order_job_id(43)));
    order_v["job_id_rule"] = json!("sha256(b\"pearl-sol:order-job:v1\" || solana_nonce u64 LE)");
    order_v["deadline_rule"] = json!("relative: deadline_height = unlock block height + body.deadline_height");
    let messages = json!([
        msg_v("deposit", DOMAIN_DEPOSIT, &dmsg, 30),
        order_v,
        msg_v("result", DOMAIN_RESULT, &rmsg, 20),
        {"name": "job_id", "poster": b58(&alice), "poster_nonce": 3, "job_id": hex::encode(job_id),
         "rule": "blake3(poster || nonce u64 LE)"},
        {"name": "reserved_addresses", "escrow": b58(&escrow_address()), "pool": b58(&pool_address())},
        msg_v("route_clear", DOMAIN_ROUTE_CLEAR, &cmsg, 40),
    ]);

    let proof = b"example proof bytes".to_vec();
    // Coinbase receipt (R1): the last tx_root leaf. Fees = the two txs' fees.
    let coinbase = Coinbase { height: 12, miner: bob, fees: 2000, subsidy: 800_000_000, job_id, mature_height: 42 };
    let header = Header {
        version: 1,
        height: 12,
        prev_hash: blake3_hash(&[b"parent"]),
        timestamp_ms: 1_760_000_000_000,
        target_bits: 0x207fffff,
        job_id,
        miner: bob,
        tx_root: block::tx_root_with(&tx_objs[..2], Some(&coinbase)),
        state_root: [0x11; 32],
        withdraw_root: [0x22; 32],
        proof_hash: agent_types::block::proof_hash(&proof),
        nonce: 77,
    };
    let block = Block { header, proof: proof.clone(), txs: tx_objs[..2].to_vec(), coinbase: Some(coinbase), contracts: None };
    assert_eq!(block.check_commitments(), Ok(()));
    assert_eq!(Block::decode(&block.encode()).unwrap(), block);
    let header_v = json!({
        "fields": {
            "version": header.version, "height": header.height, "prev_hash": hex::encode(header.prev_hash),
            "timestamp_ms": header.timestamp_ms, "target_bits": header.target_bits,
            "job_id": hex::encode(header.job_id), "miner": b58(&header.miner),
            "tx_root": hex::encode(header.tx_root), "state_root": hex::encode(header.state_root),
            "withdraw_root": hex::encode(header.withdraw_root), "proof_hash": hex::encode(header.proof_hash),
            "nonce": header.nonce,
        },
        "header": hex::encode(header.encode()),
        "block_hash": hex::encode(header.hash()),
        "pow_digest": hex::encode(header.pow_digest()),
        "header_with_proof": hex::encode(encode_header_with_proof(&header, &proof)),
        "block": hex::encode(block.encode()),
        "block_tx_hashes": tx_objs[..2].iter().map(|t| hex::encode(t.hash())).collect::<Vec<_>>(),
        "tx_paths": (0..2).map(|i| hex::encode(block::leaf_path(&tx_objs[..2], Some(&coinbase), i))).collect::<Vec<_>>(),
        "coinbase": {
            "fields": {"height": coinbase.height, "miner": b58(&coinbase.miner), "fees": coinbase.fees.to_string(),
                       "subsidy": coinbase.subsidy.to_string(), "job_id": hex::encode(coinbase.job_id),
                       "mature_height": coinbase.mature_height},
            "record": hex::encode(coinbase.encode()),
            "receipt_hash": hex::encode(coinbase.hash()),
            "leaf_index": 2,
            "path": hex::encode(block::leaf_path(&tx_objs[..2], Some(&coinbase), 2)),
            "rule": "record = 0x00 || height u64 || miner || fees u64 || subsidy u64 || job_id || mature_height u64 (97 bytes); last tx_root leaf = blake3(0x00 || blake3(record))",
        },
    });
    // Standing jobs (SPEC 4.5) live in their own file so wallets that do not speak them
    // keep passing tx.json.
    let rows = [0, 1, 0, 0, 3, 0];
    let cols = [0, 1, 0, 0, 7, 0];
    let standing = vec![
        tx_vector(
            "register_model",
            1,
            6,
            1000,
            TxBody::RegisterModel(RegisterModel {
                name: b"llama-3.2-1b-w7a7".to_vec(),
                manifest_uri: format!("blob:{}", hex::encode([0x44u8; 32])).into_bytes(),
                recipe: 1,
                rank: 128,
                rows_pattern: rows,
                cols_pattern: cols,
                matrices: vec![
                    MatrixSpec { k: 2048, n: 3072, hash_b: [0x61; 32] },
                    MatrixSpec { k: 8192, n: 2048, hash_b: [0x62; 32] },
                ],
            }),
        )
        .0,
        tx_vector(
            "post_segments",
            2,
            3,
            1000,
            TxBody::PostSegments {
                ticket_id: order_job_id(44),
                round: 0,
                model_id: [0x45; 32],
                rank: 128,
                rows_pattern: rows,
                cols_pattern: cols,
                segments: vec![Segment {
                    matrix: 1,
                    m: 12,
                    k: 8192,
                    n: 2048,
                    max_blocks: 1,
                    hash_a: [0x71; 32],
                    hash_b: [0x62; 32],
                    c_hash: [0x72; 32],
                    a_uri: format!("blob:{}", hex::encode([0x73u8; 32])).into_bytes(),
                }],
            },
        )
        .0,
    ];
    // RejectResult (kind 13) in its own file, like standing.json: evaluators 20 and 21
    // reject bob's delivery of the sample job (threshold 2 shown; the sample job has 1).
    let jmsg = reject_message(&job_id, &bob, &result_hash);
    let reject = json!({
        "txs": [tx_vector(
            "reject_result",
            3,
            2,
            1000,
            TxBody::RejectResult { job_id, deliverer: bob, result_hash, sigs: vec![signed_sig(20, &jmsg), signed_sig(21, &jmsg)] },
        )
        .0],
        "messages": [msg_v("reject", DOMAIN_REJECT, &jmsg, 20)],
    });
    // Social kinds (SPEC 4.6) in social.json: a profile with a blob, an operator link, a post,
    // a reply with a thought_ref and a blob, a batched React with a tip, a batched Follow.
    let profile_blob = br#"{"format":"cadre-profile-v1","bio":"keeps the lamp lit"}"#;
    let profile_id = agent_types::blake3_hash(&[profile_blob]);
    let alice_pk = pubkey_from_seed(&seed(1));
    let first_post = agent_types::social::post_id(&alice_pk, 5);
    let reply_blob = [0x74u8; 32];
    let social = json!({
        "profile_blob": String::from_utf8_lossy(profile_blob),
        "post_ids": [
            {"from": b58(&alice_pk), "nonce": 5, "post_id": hex::encode(first_post)},
            {"from": b58(&bob), "nonce": 0, "post_id": hex::encode(agent_types::social::post_id(&bob, 0))},
        ],
        "txs": [
            tx_vector("set_profile", 1, 3, 1000, TxBody::SetProfile(agent_types::SetProfile {
                handle: b"lamp_keeper".to_vec(), name: "Lamp Keeper".as_bytes().to_vec(),
                profile_uri: format!("blob:{}", hex::encode(profile_id)).into_bytes(), profile_hash: profile_id,
                flags: 1 | 2, operator: pubkey_from_seed(&seed(9)), enc_key: [0; 32], dm_price: 5_000_000, dm_policy: 1,
                thread_free_blocks: 1440 })).0,
            tx_vector("operator_link", 9, 0, 1000, TxBody::OperatorLink { agent: alice_pk, add: true }).0,
            tx_vector("post", 1, 5, 1000, TxBody::Post(agent_types::Post {
                kind: 0, parent: [0; 32], channel: [0; 32], body: "The fog came in at block 3400.".as_bytes().to_vec(),
                blob_uri: Vec::new(), blob_hash: [0; 32], thought_ref: b"req_62fc4ba094b44e8eca9d331f".to_vec() })).0,
            tx_vector("reply", 2, 0, 2000, TxBody::Post(agent_types::Post {
                kind: 1, parent: first_post, channel: [0; 32], body: "Saw it from the harbour, caf\u{e9} still open.".as_bytes().to_vec(),
                blob_uri: format!("blob:{}", hex::encode(reply_blob)).into_bytes(), blob_hash: reply_blob, thought_ref: Vec::new() })).0,
            tx_vector("react", 2, 1, 1000, TxBody::React { items: vec![
                agent_types::Reaction { post_id: first_post, reaction: 0, tip: 0 },
                agent_types::Reaction { post_id: first_post, reaction: 4, tip: 250_000_000 }] }).0,
            tx_vector("follow", 2, 2, 1000, TxBody::Follow { items: vec![
                agent_types::FollowEdge { target: alice_pk, on: true },
                agent_types::FollowEdge { target: pubkey_from_seed(&seed(9)), on: false }] }).0,
        ],
    });
    // DM and channel kinds (SPEC 4.10) in dm.json: a plain DM with escrowed pay, a sealed DM
    // (framing only here; real envelopes are in dm-crypto.json), a reply that releases it, a
    // reclaim, a batched SetConsent, a sealed channel, an invite, a join, a key epoch, a
    // sealed channel message.
    use agent_types::dm::{self as dmt, Consent, KeyEnvelope, SendMessage};
    let carol = pubkey_from_seed(&seed(4));
    let paid_msg = dmt::message_id(&alice_pk, 6);
    let chan = dmt::channel_id(&alice_pk, 8);
    let mut sealed_body = vec![1u8, 1];
    sealed_body.extend([0x11u8; 32]);
    sealed_body.extend([0x22u8; 24]);
    sealed_body.extend(21u16.to_le_bytes());
    sealed_body.extend([0x33u8; 21]);
    let mut chan_body = vec![2u8];
    chan_body.extend(1u32.to_le_bytes());
    chan_body.extend([0x44u8; 24]);
    chan_body.extend([0x55u8; 30]);
    let dm_v = json!({
        "message_ids": [{"from": b58(&alice_pk), "nonce": 6, "message_id": hex::encode(paid_msg)}],
        "channel_ids": [{"creator": b58(&alice_pk), "nonce": 8, "channel_id": hex::encode(chan)}],
        "txs": [
            tx_vector("dm_plain_escrow", 1, 6, 1000, TxBody::SendMessage(SendMessage {
                to: bob, channel: [0; 32], mode: 0, reply_to: [0; 32], pay: 5_000_000, reclaim_after: 360,
                body: "Are you taking questions about the fog?".as_bytes().to_vec(), blob_uri: Vec::new(), blob_hash: [0; 32], thought_ref: Vec::new() })).0,
            tx_vector("dm_sealed", 1, 7, 1000, TxBody::SendMessage(SendMessage {
                to: bob, channel: [0; 32], mode: 1, reply_to: [0; 32], pay: 0, reclaim_after: 0,
                body: sealed_body, blob_uri: Vec::new(), blob_hash: [0; 32], thought_ref: Vec::new() })).0,
            tx_vector("dm_reply_releases", 2, 3, 1000, TxBody::SendMessage(SendMessage {
                to: alice_pk, channel: [0; 32], mode: 0, reply_to: paid_msg, pay: 0, reclaim_after: 0,
                body: "Yes. The fog lifts at dawn.".as_bytes().to_vec(), blob_uri: Vec::new(), blob_hash: [0; 32],
                thought_ref: b"req_ec0095707b8eb3677bc847de".to_vec() })).0,
            tx_vector("reclaim", 1, 9, 1000, TxBody::ReclaimMessage { message_id: paid_msg }).0,
            tx_vector("set_consent", 2, 4, 1000, TxBody::SetConsent { items: vec![
                Consent { peer: alice_pk, state: 1 }, Consent { peer: carol, state: 2 }, Consent { peer: pubkey_from_seed(&seed(9)), state: 0 }] }).0,
            tx_vector("channel_create", 1, 8, 1000, TxBody::ChannelConfig(agent_types::ChannelConfig {
                channel_id: [0; 32], name: "Market desk".as_bytes().to_vec(), about: "Prices, bids and gossip.".as_bytes().to_vec(),
                policy: 2, join_price: 1_000_000, mode: 1, admins: vec![bob] })).0,
            tx_vector("channel_invite", 1, 10, 1000, TxBody::ChannelMember { channel: chan, action: 2, member: carol, pay: 0 }).0,
            tx_vector("channel_join_paid", 5, 0, 1000, TxBody::ChannelMember { channel: chan, action: 0, member: [0; 32], pay: 1_000_000 }).0,
            tx_vector("channel_key", 1, 11, 1000, TxBody::ChannelKey { channel: chan, epoch: 1, envelopes: vec![
                KeyEnvelope { member: alice_pk, envelope: [0x66; 104] }, KeyEnvelope { member: bob, envelope: [0x77; 104] }] }).0,
            tx_vector("channel_message_sealed", 2, 5, 1000, TxBody::SendMessage(SendMessage {
                to: [0; 32], channel: chan, mode: 1, reply_to: [0; 32], pay: 0, reclaim_after: 0,
                body: chan_body, blob_uri: Vec::new(), blob_hash: [0; 32], thought_ref: Vec::new() })).0,
        ],
    });
    // Contract kinds (docs/CONTRACTS.md) in contracts.json: the five tx kinds, the contract
    // address rule, an event topic, ABI-encoded call args, and a contracts record with a
    // receipt, an event and their paths.
    use agent_types::contracts as ac;
    let code = b"\0asm\x01\0\0\0".to_vec();
    let ch = ac::code_hash(&code);
    let addr = ac::contract_address(&alice_pk, 7);
    // ABI: method "increment" (u8 len) then u64 5
    let mut args = vec![9u8];
    args.extend_from_slice(b"increment");
    args.extend_from_slice(&5u64.to_le_bytes());
    let call_tx = tx_vector("call", 2, 3, 2_000_000, TxBody::Call(ac::Call { contract: addr, value: 1_000_000_000, gas_limit: 1_000_000, args: args.clone() }));
    let ev = ac::Event { contract: addr, tx_hash: call_tx.1.hash(), index: 0, topics: vec![ac::event_topic(b"Incremented")], data: 5u64.to_le_bytes().to_vec() };
    let rc = ac::Receipt { tx_hash: call_tx.1.hash(), kind: 32, status: ac::Status::Ok, gas_limit: 1_000_000, gas_used: 412_345,
        fee_charged: 1_500_000, contract: addr, code_hash: ch, event_count: 1, ret: 5u64.to_le_bytes().to_vec(), error: Vec::new() };
    let rec = ac::ContractsRecord::build(42, std::slice::from_ref(&rc), std::slice::from_ref(&ev));
    let cb = agent_types::Coinbase { height: 42, miner: bob, fees: 1_500_000, subsidy: 0, job_id: [0; 32], mature_height: 72 };
    let txs1 = std::slice::from_ref(&call_tx.1);
    let block_root = block::tx_root_full(txs1, Some(&rec), Some(&cb));
    let contracts = json!({
        "block": {"txs": [hex::encode(call_tx.1.encode())], "coinbase": hex::encode(cb.encode()), "tx_root": hex::encode(block_root),
            "record_leaf_index": 1, "record_path": hex::encode(block::leaf_path_full(txs1, Some(&rec), Some(&cb), 1)),
            "coinbase_path": hex::encode(block::leaf_path_full(txs1, Some(&rec), Some(&cb), 2)),
            "event_path": hex::encode(ac::path_bytes(&[ev.hash()], 0))},
        "address_rule": {"creator": b58(&alice_pk), "nonce": 7, "address": b58(&addr)},
        "event_topic": {"name": "Incremented", "topic": hex::encode(ac::event_topic(b"Incremented"))},
        "call_args": {"method": "increment", "fields": [{"u64": "5"}], "args": hex::encode(&args)},
        "event": {"bytes": hex::encode(ev.encode()), "hash": hex::encode(ev.hash())},
        "receipt": {"bytes": hex::encode(rc.encode()), "hash": hex::encode(rc.hash())},
        "record": {"bytes": hex::encode(rec.encode()), "hash": hex::encode(rec.hash()), "events_root": hex::encode(rec.events_root),
            "receipts_root": hex::encode(rec.receipts_root)},
        "txs": [
            tx_vector("deploy_code", 1, 6, 2_000_000, TxBody::DeployCode(ac::DeployCode { gas_limit: 1_000_000, code: code.clone() })).0,
            tx_vector("instantiate", 1, 7, 2_000_000, TxBody::Instantiate(ac::Instantiate { code_hash: ch, admin: alice_pk, value: 0,
                gas_limit: 2_000_000, label: b"counter".to_vec(), args: 10u64.to_le_bytes().to_vec() })).0,
            call_tx.0,
            tx_vector("migrate", 1, 8, 2_000_000, TxBody::Migrate(ac::Migrate { contract: addr, code_hash: ch, gas_limit: 500_000, args: Vec::new() })).0,
            tx_vector("set_contract_admin", 1, 9, 1000, TxBody::SetContractAdmin { contract: addr, admin: [0; 32] }).0,
        ],
    });
    vec![
        ("dm.json", dm_v),
        ("contracts.json", contracts),
        ("social.json", social),
        ("tx.json", Value::Array(tx_json)),
        ("reject.json", reject),
        ("messages.json", messages),
        ("header.json", header_v),
        ("standing.json", Value::Array(standing)),
    ]
}

#[test]
fn vectors_match_files() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors");
    for (name, v) in build() {
        let text = serde_json::to_string_pretty(&v).unwrap() + "\n";
        let path = dir.join(name);
        if std::env::var("UPDATE_VECTORS").is_ok() || !path.exists() {
            std::fs::write(&path, &text).unwrap();
        } else {
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert_eq!(on_disk, text, "{name} drifted; rerun with UPDATE_VECTORS=1 if intended");
        }
    }
}
