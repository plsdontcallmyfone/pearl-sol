mod common;
use anchor_lang::{InstructionData, ToAccountMetas};
use common::*;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey, signature::Signer, system_program};

fn register_ix(auth: &Pubkey, name: &str, uri: &str, payout: Pubkey) -> Instruction {
    Instruction {
        program_id: agents::ID,
        accounts: agents::accounts::Register {
            authority: *auth,
            profile: agent_pda(auth),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: agents::instruction::Register { name: name.into(), uri: uri.into(), payout }.data(),
    }
}

#[test]
fn register_update_deregister() {
    let mut env = Env::bare();
    let a = env.user(SOL);
    let payout = Pubkey::new_unique();
    env.send(&[register_ix(&a.pubkey(), "alice", "https://a.example/card.json", payout)], &a, &[])
        .unwrap();
    let p: agents::AgentProfile = env.account(&agent_pda(&a.pubkey()));
    assert_eq!(p.authority, a.pubkey());
    assert_eq!(p.name, "alice");
    assert_eq!(p.payout, payout);

    // second register fails (PDA exists)
    assert!(env
        .send(&[register_ix(&a.pubkey(), "alice2", "", payout)], &a, &[])
        .is_err());

    let new_payout = Pubkey::new_unique();
    let ix = Instruction {
        program_id: agents::ID,
        accounts: agents::accounts::UpdateProfile { authority: a.pubkey(), profile: agent_pda(&a.pubkey()) }
            .to_account_metas(None),
        data: agents::instruction::UpdateProfile { name: None, uri: Some("ipfs://x".into()), payout: Some(new_payout) }
            .data(),
    };
    env.send(&[ix], &a, &[]).unwrap();
    let p: agents::AgentProfile = env.account(&agent_pda(&a.pubkey()));
    assert_eq!(p.name, "alice");
    assert_eq!(p.uri, "ipfs://x");
    assert_eq!(p.payout, new_payout);

    // someone else cannot update alice's profile: their own PDA does not exist
    let b = env.user(SOL);
    let ix = Instruction {
        program_id: agents::ID,
        accounts: agents::accounts::UpdateProfile { authority: b.pubkey(), profile: agent_pda(&a.pubkey()) }
            .to_account_metas(None),
        data: agents::instruction::UpdateProfile { name: Some("mallory".into()), uri: None, payout: None }.data(),
    };
    assert!(env.send(&[ix], &b, &[]).is_err());

    let before = env.balance(&a.pubkey());
    let ix = Instruction {
        program_id: agents::ID,
        accounts: agents::accounts::Deregister { authority: a.pubkey(), profile: agent_pda(&a.pubkey()) }
            .to_account_metas(None),
        data: agents::instruction::Deregister {}.data(),
    };
    env.send(&[ix], &a, &[]).unwrap();
    assert!(!env.exists(&agent_pda(&a.pubkey())));
    assert!(env.balance(&a.pubkey()) > before);
}

#[test]
fn bad_name_rejected() {
    let mut env = Env::bare();
    let a = env.user(SOL);
    let r = env.send(&[register_ix(&a.pubkey(), "", "", a.pubkey())], &a, &[]);
    assert_eq!(err_code(&r), Some(6000));
    let long = "x".repeat(33);
    let r = env.send(&[register_ix(&a.pubkey(), &long, "", a.pubkey())], &a, &[]);
    assert!(r.is_err());
}
