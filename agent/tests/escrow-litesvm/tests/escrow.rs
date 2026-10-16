//! LiteSVM tests for agent/programs/escrow (SPEC 3.1). In-process, no ports.
//! Build the program first: `cd agent && anchor build -p escrow`.

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use escrow::{Config, EscrowError, Processed, Withdrawal, CHAIN_TOKEN, DEPOSIT_DOMAIN};
use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{Instruction, InstructionError},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::{Transaction, TransactionError},
};

const ED25519: Pubkey = solana_sdk::pubkey!("Ed25519SigVerify111111111111111111111111111");
const IX_SYSVAR: Pubkey = solana_sdk::pubkey!("Sysvar1nstructions1111111111111111111111111");
/// Whole agent-chain supply: 1B tokens x 1e9 lamports (SPEC 1).
const PRELOAD: u64 = 1_000_000_000_000_000_000;

fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &escrow::ID).0
}
fn escrow_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"escrow"], &escrow::ID).0
}
fn withdrawal_pda(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"withdrawal", &nonce.to_le_bytes()], &escrow::ID).0
}
fn processed_pda(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"processed", &[CHAIN_TOKEN], &nonce.to_le_bytes()], &escrow::ID).0
}

/// One ed25519 precompile instruction carrying every (signer, message) pair.
fn ed25519_ix(entries: &[(&Keypair, &[u8])]) -> Instruction {
    let n = entries.len();
    let mut data = vec![n as u8, 0u8];
    let header = 2 + 14 * n;
    let mut body: Vec<u8> = Vec::new();
    let mut offsets: Vec<u8> = Vec::new();
    for (kp, msg) in entries {
        let sig = kp.sign_message(msg);
        let pk_off = header + body.len();
        body.extend_from_slice(kp.pubkey().as_ref());
        let sig_off = header + body.len();
        body.extend_from_slice(sig.as_ref());
        let msg_off = header + body.len();
        body.extend_from_slice(msg);
        for v in [sig_off as u16, u16::MAX, pk_off as u16, u16::MAX, msg_off as u16, msg.len() as u16, u16::MAX] {
            offsets.extend_from_slice(&v.to_le_bytes());
        }
    }
    data.extend_from_slice(&offsets);
    data.extend_from_slice(&body);
    Instruction { program_id: ED25519, accounts: vec![], data }
}

fn custom(code: u32) -> impl Fn(&TransactionError) -> bool {
    move |e| matches!(e, TransactionError::InstructionError(_, InstructionError::Custom(c)) if *c == code)
}
fn code(e: EscrowError) -> u32 {
    u32::from(e)
}
fn deposit_msg(nonce: u64, recipient: &Pubkey, amount_spl: u64) -> Vec<u8> {
    escrow::bridge_message(DEPOSIT_DOMAIN, nonce, recipient, amount_spl)
}

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    admin: Keypair,
    signers: Vec<Keypair>,
}

impl Env {
    fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<(), TransactionError> {
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        signers.extend(extra.iter().copied().filter(|k| k.pubkey() != self.payer.pubkey()));
        let tx = Transaction::new_signed_with_payer(ixs, Some(&self.payer.pubkey()), &signers, self.svm.latest_blockhash());
        let r = self.svm.send_transaction(tx).map(|_| ()).map_err(|f| {
            for l in &f.meta.logs {
                eprintln!("  log: {l}");
            }
            f.err
        });
        self.svm.expire_blockhash();
        r
    }
    fn bal(&self, k: &Pubkey) -> u64 {
        self.svm.get_balance(k).unwrap_or(0)
    }
    fn config(&self) -> Config {
        Config::try_deserialize(&mut self.svm.get_account(&config_pda()).unwrap().data.as_slice()).unwrap()
    }
    fn unlock_ix(&self, nonce: u64, recipient: Pubkey, amount_spl: u64) -> Instruction {
        Instruction {
            program_id: escrow::ID,
            accounts: escrow::accounts::Unlock {
                payer: self.payer.pubkey(),
                config: config_pda(),
                escrow: escrow_pda(),
                recipient,
                processed: processed_pda(nonce),
                instructions: IX_SYSVAR,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: escrow::instruction::Unlock { nonce, recipient, amount_spl }.data(),
        }
    }
    fn lock_ix(&self, sender: &Pubkey, amount_lamports: u64, solana_recipient: Pubkey) -> Instruction {
        let nonce = self.config().withdraw_nonce;
        Instruction {
            program_id: escrow::ID,
            accounts: escrow::accounts::Lock {
                sender: *sender,
                config: config_pda(),
                escrow: escrow_pda(),
                withdrawal: withdrawal_pda(nonce),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: escrow::instruction::Lock { amount_lamports, solana_recipient }.data(),
        }
    }
    fn set_signers_ix(&self, admin: &Pubkey, signers: Vec<Pubkey>, threshold: u8) -> Instruction {
        Instruction {
            program_id: escrow::ID,
            accounts: escrow::accounts::AdminOnly { admin: *admin, config: config_pda() }.to_account_metas(None),
            data: escrow::instruction::SetSigners { signers, threshold }.data(),
        }
    }
}

/// Fresh SVM with the escrow PDA preloaded (program-owned, no data, 1e18
/// lamports, as `--account` does on localnet) and the config initialized.
fn setup(n_signers: usize, threshold: u8) -> Env {
    let mut svm = LiteSVM::new();
    let so = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/deploy/escrow.so"))
        .expect("build first: cd agent && anchor build -p escrow");
    svm.add_program(escrow::ID, &so);
    svm.set_account(escrow_pda(), Account { lamports: PRELOAD, data: vec![], owner: escrow::ID, executable: false, rent_epoch: 0 })
        .unwrap();
    let payer = Keypair::new();
    let admin = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();
    let signers: Vec<Keypair> = (0..n_signers).map(|_| Keypair::new()).collect();
    let mut env = Env { svm, payer, admin, signers };
    let init = Instruction {
        program_id: escrow::ID,
        accounts: escrow::accounts::Initialize {
            admin: env.admin.pubkey(),
            config: config_pda(),
            escrow: escrow_pda(),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: escrow::instruction::Initialize { signers: env.signers.iter().map(|k| k.pubkey()).collect(), threshold }.data(),
    };
    let admin = env.admin.insecure_clone();
    env.send(&[init], &[&admin]).expect("initialize");
    env
}

#[test]
fn initialize_records_config() {
    let env = setup(1, 1);
    let c = env.config();
    assert_eq!(c.admin, env.admin.pubkey());
    assert_eq!(c.threshold, 1);
    assert_eq!(c.signers, vec![env.signers[0].pubkey()]);
    assert_eq!(c.withdraw_nonce, 0);
    assert_eq!(env.bal(&escrow_pda()), PRELOAD);
}

#[test]
fn unlock_happy_path_then_replay_is_rejected() {
    let mut env = setup(1, 1);
    let s0 = env.signers[0].insecure_clone();
    let recipient = Pubkey::new_unique(); // a brand-new account
    let amount_spl = 2_500_000u64; // 2.5 tokens
    let msg = deposit_msg(0, &recipient, amount_spl);
    env.send(&[ed25519_ix(&[(&s0, &msg)]), env.unlock_ix(0, recipient, amount_spl)], &[]).expect("unlock");
    assert_eq!(env.bal(&recipient), amount_spl * 1000);
    assert_eq!(env.bal(&escrow_pda()), PRELOAD - amount_spl * 1000);
    let p = Processed::try_deserialize(&mut env.svm.get_account(&processed_pda(0)).unwrap().data.as_slice()).unwrap();
    assert_eq!((p.source_chain, p.nonce, p.recipient, p.amount), (CHAIN_TOKEN, 0, recipient, amount_spl));

    let e = env.send(&[ed25519_ix(&[(&s0, &msg)]), env.unlock_ix(0, recipient, amount_spl)], &[]).unwrap_err();
    assert!(custom(0)(&e), "replay must fail on the existing Processed PDA: {e:?}");
    assert_eq!(env.bal(&recipient), amount_spl * 1000);
    assert_eq!(env.bal(&escrow_pda()), PRELOAD - amount_spl * 1000);
}

#[test]
fn unlock_bad_signatures_fail() {
    let mut env = setup(1, 1);
    let s0 = env.signers[0].insecure_clone();
    let recipient = Pubkey::new_unique();
    let msg = deposit_msg(0, &recipient, 1_000_000);
    let nonsig = custom(code(EscrowError::NotEnoughSignatures));

    // no ed25519 instruction at all
    let e = env.send(&[env.unlock_ix(0, recipient, 1_000_000)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // corrupted signature: the precompile fails the tx
    let mut ed = ed25519_ix(&[(&s0, &msg)]);
    ed.data[2 + 14 + 32 + 5] ^= 0x01;
    assert!(env.send(&[ed, env.unlock_ix(0, recipient, 1_000_000)], &[]).is_err());
    // a key that is not a configured signer
    let x = Keypair::new();
    let e = env.send(&[ed25519_ix(&[(&x, &msg)]), env.unlock_ix(0, recipient, 1_000_000)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // signed amount differs from the unlock amount
    let e = env.send(&[ed25519_ix(&[(&s0, &msg)]), env.unlock_ix(0, recipient, 1_000_001)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // signed recipient differs
    let other = Pubkey::new_unique();
    let e = env.send(&[ed25519_ix(&[(&s0, &msg)]), env.unlock_ix(0, other, 1_000_000)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // a withdraw-domain message is not a deposit message
    let wmsg = escrow::bridge_message(escrow::WITHDRAW_DOMAIN, 0, &recipient, 1_000_000);
    let e = env.send(&[ed25519_ix(&[(&s0, &wmsg)]), env.unlock_ix(0, recipient, 1_000_000)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");

    assert_eq!(env.bal(&escrow_pda()), PRELOAD);
    assert!(env.svm.get_account(&processed_pda(0)).is_none());
}

#[test]
fn unlock_threshold_two_of_three() {
    let mut env = setup(3, 2);
    let [a, b, c] = [0, 1, 2].map(|i| env.signers[i].insecure_clone());
    let recipient = Pubkey::new_unique();
    let msg = deposit_msg(5, &recipient, 3_000_000);
    let nonsig = custom(code(EscrowError::NotEnoughSignatures));

    let e = env.send(&[ed25519_ix(&[(&b, &msg)]), env.unlock_ix(5, recipient, 3_000_000)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    let e = env.send(&[ed25519_ix(&[(&b, &msg), (&b, &msg)]), env.unlock_ix(5, recipient, 3_000_000)], &[]).unwrap_err();
    assert!(nonsig(&e), "same signer twice counts once: {e:?}");

    env.send(&[ed25519_ix(&[(&a, &msg)]), ed25519_ix(&[(&c, &msg)]), env.unlock_ix(5, recipient, 3_000_000)], &[])
        .expect("2-of-3");
    assert_eq!(env.bal(&recipient), 3_000_000_000);
}

#[test]
fn lock_writes_withdrawal_and_enforces_multiple_of_1000() {
    let mut env = setup(1, 1);
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let sol_recipient = Pubkey::new_unique();

    let e = env.send(&[env.lock_ix(&user.pubkey(), 1_500, sol_recipient)], &[&user]).unwrap_err();
    assert!(custom(code(EscrowError::NotMultipleOf1000))(&e), "{e:?}");
    let e = env.send(&[env.lock_ix(&user.pubkey(), 999, sol_recipient)], &[&user]).unwrap_err();
    assert!(custom(code(EscrowError::NotMultipleOf1000))(&e), "{e:?}");
    let e = env.send(&[env.lock_ix(&user.pubkey(), 0, sol_recipient)], &[&user]).unwrap_err();
    assert!(custom(code(EscrowError::ZeroAmount))(&e), "{e:?}");
    assert_eq!(env.config().withdraw_nonce, 0);

    let before = env.bal(&user.pubkey());
    env.send(&[env.lock_ix(&user.pubkey(), 2_000_000_000, sol_recipient)], &[&user]).expect("lock 0");
    env.send(&[env.lock_ix(&user.pubkey(), 3_000, sol_recipient)], &[&user]).expect("lock 1");
    assert_eq!(env.bal(&escrow_pda()), PRELOAD + 2_000_003_000);
    let rent = env.svm.minimum_balance_for_rent_exemption(8 + 8 + 32 + 32 + 8 + 8 + 1);
    assert_eq!(before - env.bal(&user.pubkey()), 2_000_003_000 + 2 * rent);
    assert_eq!(env.config().withdraw_nonce, 2);

    let w0 = Withdrawal::try_deserialize(&mut env.svm.get_account(&withdrawal_pda(0)).unwrap().data.as_slice()).unwrap();
    assert_eq!((w0.nonce, w0.sender, w0.recipient, w0.amount), (0, user.pubkey(), sol_recipient, 2_000_000));
    let w1 = Withdrawal::try_deserialize(&mut env.svm.get_account(&withdrawal_pda(1)).unwrap().data.as_slice()).unwrap();
    assert_eq!((w1.nonce, w1.amount), (1, 3));
}

#[test]
fn round_trip_keeps_escrow_accounting() {
    // unlock 7 tokens to a user, user locks 3 back: escrow = PRELOAD - 4 tokens.
    let mut env = setup(1, 1);
    let s0 = env.signers[0].insecure_clone();
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 1_000_000_000).unwrap();
    let msg = deposit_msg(0, &user.pubkey(), 7_000_000);
    env.send(&[ed25519_ix(&[(&s0, &msg)]), env.unlock_ix(0, user.pubkey(), 7_000_000)], &[]).expect("unlock");
    env.send(&[env.lock_ix(&user.pubkey(), 3_000_000_000, Pubkey::new_unique())], &[&user]).expect("lock");
    assert_eq!(env.bal(&escrow_pda()), PRELOAD - 4_000_000_000);
}

#[test]
fn admin_edits_signers_and_admin() {
    let mut env = setup(1, 1);
    let admin = env.admin.insecure_clone();
    let old = env.signers[0].insecure_clone();
    let new_signer = Keypair::new();

    let mallory = Keypair::new();
    env.svm.airdrop(&mallory.pubkey(), 1_000_000_000).unwrap();
    let e = env.send(&[env.set_signers_ix(&mallory.pubkey(), vec![mallory.pubkey()], 1)], &[&mallory]).unwrap_err();
    assert!(custom(code(EscrowError::NotAdmin))(&e), "{e:?}");
    let e = env.send(&[env.set_signers_ix(&admin.pubkey(), vec![new_signer.pubkey()], 0)], &[&admin]).unwrap_err();
    assert!(custom(code(EscrowError::BadThreshold))(&e), "{e:?}");
    let too_many: Vec<Pubkey> = (0..11).map(|_| Pubkey::new_unique()).collect();
    let e = env.send(&[env.set_signers_ix(&admin.pubkey(), too_many, 1)], &[&admin]).unwrap_err();
    assert!(custom(code(EscrowError::BadSignerSet))(&e), "{e:?}");

    env.send(&[env.set_signers_ix(&admin.pubkey(), vec![new_signer.pubkey()], 1)], &[&admin]).expect("set signers");
    let r = Pubkey::new_unique();
    let msg = deposit_msg(0, &r, 1_000);
    let e = env.send(&[ed25519_ix(&[(&old, &msg)]), env.unlock_ix(0, r, 1_000)], &[]).unwrap_err();
    assert!(custom(code(EscrowError::NotEnoughSignatures))(&e), "{e:?}");
    env.send(&[ed25519_ix(&[(&new_signer, &msg)]), env.unlock_ix(0, r, 1_000)], &[]).expect("new signer works");

    let next = Keypair::new();
    let ix = Instruction {
        program_id: escrow::ID,
        accounts: escrow::accounts::AdminOnly { admin: admin.pubkey(), config: config_pda() }.to_account_metas(None),
        data: escrow::instruction::SetAdmin { new_admin: next.pubkey() }.data(),
    };
    env.send(&[ix], &[&admin]).expect("set admin");
    assert_eq!(env.config().admin, next.pubkey());
    let e = env.send(&[env.set_signers_ix(&admin.pubkey(), vec![old.pubkey()], 1)], &[&admin]).unwrap_err();
    assert!(custom(code(EscrowError::NotAdmin))(&e), "{e:?}");
}
