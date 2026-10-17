//! LiteSVM tests for token/programs/bridge (SPEC 2.3). In-process, no ports.
//! Build the program first: `cd token && anchor build -p bridge`.

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use bridge::{BridgeError, Config, Deposit, Processed, CHAIN_AGENT, CHAIN_TOKEN, REFUND_DOMAIN, WITHDRAW_DOMAIN};
use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{AccountMeta, Instruction, InstructionError},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::{Transaction, TransactionError},
};

const TOKEN_2022: Pubkey = solana_sdk::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const ATA_PROGRAM: Pubkey = solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const ED25519: Pubkey = solana_sdk::pubkey!("Ed25519SigVerify111111111111111111111111111");
const IX_SYSVAR: Pubkey = solana_sdk::pubkey!("Sysvar1nstructions1111111111111111111111111");
const DECIMALS: u8 = 6;

// ---------- helpers ----------

fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[owner.as_ref(), TOKEN_2022.as_ref(), mint.as_ref()], &ATA_PROGRAM).0
}
fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &bridge::ID).0
}
fn deposit_pda(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"deposit", &nonce.to_le_bytes()], &bridge::ID).0
}
fn processed_pda(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"processed", &[CHAIN_AGENT], &nonce.to_le_bytes()], &bridge::ID).0
}

/// One ed25519 precompile instruction carrying every (signer, message) pair,
/// with all offsets pointing into this instruction (index u16::MAX).
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

fn err_code(e: BridgeError) -> u32 {
    u32::from(e)
}

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    admin: Keypair,
    mint: Pubkey,
    signers: Vec<Keypair>,
}

impl Env {
    fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<litesvm::types::TransactionMetadata, TransactionError> {
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        for k in extra {
            if k.pubkey() != self.payer.pubkey() {
                signers.push(k);
            }
        }
        let tx = Transaction::new_signed_with_payer(ixs, Some(&self.payer.pubkey()), &signers, self.svm.latest_blockhash());
        let r = self.svm.send_transaction(tx).map_err(|f| {
            for l in &f.meta.logs {
                eprintln!("  log: {l}");
            }
            f.err
        });
        self.svm.expire_blockhash();
        r
    }

    fn token_balance(&self, account: &Pubkey) -> u64 {
        let a = self.svm.get_account(account).expect("token account");
        u64::from_le_bytes(a.data[64..72].try_into().unwrap())
    }

    fn config(&self) -> Config {
        let a = self.svm.get_account(&config_pda()).unwrap();
        Config::try_deserialize(&mut a.data.as_slice()).unwrap()
    }

    fn create_ata(&mut self, owner: &Pubkey) -> Pubkey {
        let ix = Instruction {
            program_id: ATA_PROGRAM,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(ata(owner, &self.mint), false),
                AccountMeta::new_readonly(*owner, false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_2022, false),
            ],
            data: vec![1], // CreateIdempotent
        };
        self.send(&[ix], &[]).expect("create ata");
        ata(owner, &self.mint)
    }

    fn mint_to(&mut self, dest: &Pubkey, amount: u64) {
        let mut data = vec![7u8]; // MintTo
        data.extend_from_slice(&amount.to_le_bytes());
        let ix = Instruction {
            program_id: TOKEN_2022,
            accounts: vec![
                AccountMeta::new(self.mint, false),
                AccountMeta::new(*dest, false),
                AccountMeta::new_readonly(self.admin.pubkey(), true),
            ],
            data,
        };
        let admin = self.admin.insecure_clone();
        self.send(&[ix], &[&admin]).expect("mint_to");
    }

    fn deposit_ix(&self, sender: &Pubkey, amount: u64, recipient: Pubkey) -> Instruction {
        let nonce = self.config().deposit_nonce;
        Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::DepositIx {
                payer: self.payer.pubkey(),
                sender: *sender,
                config: config_pda(),
                mint: self.mint,
                sender_token: ata(sender, &self.mint),
                vault: ata(&config_pda(), &self.mint),
                deposit: deposit_pda(nonce),
                token_program: TOKEN_2022,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: bridge::instruction::Deposit { amount, agent_recipient: recipient }.data(),
        }
    }

    fn release_ix(&self, nonce: u64, recipient: Pubkey, amount: u64, recipient_token: Pubkey) -> Instruction {
        Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::Release {
                payer: self.payer.pubkey(),
                config: config_pda(),
                mint: self.mint,
                vault: ata(&config_pda(), &self.mint),
                recipient_token,
                processed: processed_pda(nonce),
                instructions: IX_SYSVAR,
                token_program: TOKEN_2022,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: bridge::instruction::Release { nonce, recipient, amount }.data(),
        }
    }

    fn refund_ix(&self, nonce: u64, buyer: Pubkey, buyer_token: Pubkey) -> Instruction {
        Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::RefundDeposit {
                payer: self.payer.pubkey(),
                config: config_pda(),
                mint: self.mint,
                vault: ata(&config_pda(), &self.mint),
                deposit: deposit_pda(nonce),
                buyer_token,
                refund: bridge::refund_record_address(nonce),
                instructions: IX_SYSVAR,
                token_program: TOKEN_2022,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: bridge::instruction::RefundDeposit { nonce, buyer }.data(),
        }
    }

    fn set_signers_ix(&self, admin: &Pubkey, signers: Vec<Pubkey>, threshold: u8) -> Instruction {
        Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::AdminOnly { admin: *admin, config: config_pda() }.to_account_metas(None),
            data: bridge::instruction::SetSigners { signers, threshold }.data(),
        }
    }

    fn vault(&self) -> Pubkey {
        ata(&config_pda(), &self.mint)
    }
}

fn withdraw_msg(nonce: u64, recipient: &Pubkey, amount: u64) -> Vec<u8> {
    bridge::bridge_message(WITHDRAW_DOMAIN, nonce, recipient, amount)
}

/// Fresh SVM, a Token-2022 mint (6 dp, with a MetadataPointer extension like
/// pump's create_v2 mints), bridge initialized with `n_signers` and `threshold`.
fn setup(n_signers: usize, threshold: u8) -> Env {
    let mut svm = LiteSVM::new();
    let so = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/deploy/bridge.so"))
        .expect("build first: cd token && anchor build -p bridge");
    svm.add_program(bridge::ID, &so);
    let payer = Keypair::new();
    let admin = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();

    // Mint: base 82 bytes padded to 165, account type byte, MetadataPointer TLV (2+2+64).
    let mint_kp = Keypair::new();
    let mint = mint_kp.pubkey();
    let mint_len = 165 + 1 + 4 + 64;
    let rent = svm.minimum_balance_for_rent_exemption(mint_len);
    svm.set_account(mint, Account { lamports: rent, data: vec![0; mint_len], owner: TOKEN_2022, executable: false, rent_epoch: 0 })
        .unwrap();
    let mut mp = vec![39u8, 0u8]; // MetadataPointerExtension::Initialize
    mp.extend_from_slice(admin.pubkey().as_ref());
    mp.extend_from_slice(mint.as_ref());
    let init_mp = Instruction { program_id: TOKEN_2022, accounts: vec![AccountMeta::new(mint, false)], data: mp };
    let mut im = vec![20u8, DECIMALS]; // InitializeMint2
    im.extend_from_slice(admin.pubkey().as_ref());
    im.push(0); // no freeze authority
    let init_mint = Instruction { program_id: TOKEN_2022, accounts: vec![AccountMeta::new(mint, false)], data: im };

    let signers: Vec<Keypair> = (0..n_signers).map(|_| Keypair::new()).collect();
    let mut env = Env { svm, payer, admin, mint, signers };
    env.send(&[init_mp, init_mint], &[]).expect("init mint");

    let admin_pk = env.admin.pubkey();
    let init = Instruction {
        program_id: bridge::ID,
        accounts: bridge::accounts::Initialize {
            admin: admin_pk,
            config: config_pda(),
            mint,
            vault: ata(&config_pda(), &mint),
            token_program: TOKEN_2022,
            associated_token_program: ATA_PROGRAM,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: bridge::instruction::Initialize {
            signers: env.signers.iter().map(|k| k.pubkey()).collect(),
            threshold,
        }
        .data(),
    };
    let admin = env.admin.insecure_clone();
    env.send(&[init], &[&admin]).expect("initialize");
    env
}

/// A user deposits `amount` so the vault has something to release.
fn fund_vault(env: &mut Env, amount: u64) -> Keypair {
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 1_000_000_000).unwrap();
    let user_ata = env.create_ata(&user.pubkey());
    env.mint_to(&user_ata, amount);
    let ix = env.deposit_ix(&user.pubkey(), amount, Pubkey::new_unique());
    env.send(&[ix], &[&user]).expect("deposit");
    user
}

// ---------- tests ----------

#[test]
fn initialize_records_config_and_vault() {
    let env = setup(1, 1);
    let c = env.config();
    assert_eq!(c.admin, env.admin.pubkey());
    assert_eq!(c.mint, env.mint);
    assert_eq!(c.vault, env.vault());
    assert_eq!(c.token_program, TOKEN_2022);
    assert_eq!(c.threshold, 1);
    assert_eq!(c.signers, vec![env.signers[0].pubkey()]);
    assert_eq!(c.deposit_nonce, 0);
    assert_eq!(env.token_balance(&env.vault()), 0);
}

#[test]
fn deposit_moves_spl_and_writes_records_with_global_nonce() {
    let mut env = setup(1, 1);
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 1_000_000_000).unwrap();
    let user_ata = env.create_ata(&user.pubkey());
    env.mint_to(&user_ata, 5_000_000);

    let r1 = Pubkey::new_unique();
    let r2 = Pubkey::new_unique();
    let ix = env.deposit_ix(&user.pubkey(), 1_234_567, r1);
    let meta = env.send(&[ix], &[&user]).expect("deposit 0");
    assert!(meta.logs.iter().any(|l| l.starts_with("Program data: ")), "DepositEvent emitted");
    let ix = env.deposit_ix(&user.pubkey(), 2_000_000, r2);
    env.send(&[ix], &[&user]).expect("deposit 1");

    assert_eq!(env.token_balance(&env.vault()), 3_234_567);
    assert_eq!(env.token_balance(&user_ata), 5_000_000 - 3_234_567);
    assert_eq!(env.config().deposit_nonce, 2);

    let d0 = Deposit::try_deserialize(&mut env.svm.get_account(&deposit_pda(0)).unwrap().data.as_slice()).unwrap();
    assert_eq!((d0.nonce, d0.sender, d0.recipient, d0.amount), (0, user.pubkey(), r1, 1_234_567));
    let d1 = Deposit::try_deserialize(&mut env.svm.get_account(&deposit_pda(1)).unwrap().data.as_slice()).unwrap();
    assert_eq!((d1.nonce, d1.recipient, d1.amount), (1, r2, 2_000_000));
}

#[test]
fn deposit_rejects_zero() {
    let mut env = setup(1, 1);
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 1_000_000_000).unwrap();
    let user_ata = env.create_ata(&user.pubkey());
    env.mint_to(&user_ata, 10);
    let ix = env.deposit_ix(&user.pubkey(), 0, Pubkey::new_unique());
    let e = env.send(&[ix], &[&user]).unwrap_err();
    assert!(custom(err_code(BridgeError::ZeroAmount))(&e), "{e:?}");
}

#[test]
fn release_happy_path_then_replay_is_rejected() {
    let mut env = setup(1, 1);
    fund_vault(&mut env, 10_000_000);
    let recipient = Pubkey::new_unique();
    let rta = env.create_ata(&recipient);

    let msg = withdraw_msg(7, &recipient, 4_000_000);
    let s0 = env.signers[0].insecure_clone();
    let ixs = [ed25519_ix(&[(&s0, &msg)]), env.release_ix(7, recipient, 4_000_000, rta)];
    env.send(&ixs, &[]).expect("release");
    assert_eq!(env.token_balance(&rta), 4_000_000);
    assert_eq!(env.token_balance(&env.vault()), 6_000_000);
    let p = Processed::try_deserialize(&mut env.svm.get_account(&processed_pda(7)).unwrap().data.as_slice()).unwrap();
    assert_eq!((p.source_chain, p.nonce, p.recipient, p.amount), (CHAIN_AGENT, 7, recipient, 4_000_000));

    // Same signed message again (fresh blockhash, so not a duplicate tx): replay PDA blocks it.
    let ixs = [ed25519_ix(&[(&s0, &msg)]), env.release_ix(7, recipient, 4_000_000, rta)];
    let e = env.send(&ixs, &[]).unwrap_err();
    assert!(custom(0)(&e), "replay must fail on the existing Processed PDA (system 'already in use'): {e:?}");
    assert_eq!(env.token_balance(&rta), 4_000_000);
    assert_eq!(env.token_balance(&env.vault()), 6_000_000);
}

#[test]
fn release_without_signature_fails() {
    let mut env = setup(1, 1);
    fund_vault(&mut env, 1_000_000);
    let recipient = Pubkey::new_unique();
    let rta = env.create_ata(&recipient);
    let e = env.send(&[env.release_ix(0, recipient, 1_000, rta)], &[]).unwrap_err();
    assert!(custom(err_code(BridgeError::NotEnoughSignatures))(&e), "{e:?}");
}

#[test]
fn release_with_corrupted_signature_fails_in_precompile() {
    let mut env = setup(1, 1);
    fund_vault(&mut env, 1_000_000);
    let recipient = Pubkey::new_unique();
    let rta = env.create_ata(&recipient);
    let msg = withdraw_msg(0, &recipient, 1_000);
    let s0 = env.signers[0].insecure_clone();
    let mut ed = ed25519_ix(&[(&s0, &msg)]);
    let sig_off = 2 + 14 + 32;
    ed.data[sig_off] ^= 0xff;
    let r = env.send(&[ed, env.release_ix(0, recipient, 1_000, rta)], &[]);
    assert!(r.is_err(), "bad signature must fail the tx");
    assert!(env.svm.get_account(&processed_pda(0)).is_none());
}

#[test]
fn release_signed_by_non_signer_fails() {
    let mut env = setup(1, 1);
    fund_vault(&mut env, 1_000_000);
    let recipient = Pubkey::new_unique();
    let rta = env.create_ata(&recipient);
    let stranger = Keypair::new();
    let msg = withdraw_msg(0, &recipient, 1_000);
    let e = env.send(&[ed25519_ix(&[(&stranger, &msg)]), env.release_ix(0, recipient, 1_000, rta)], &[]).unwrap_err();
    assert!(custom(err_code(BridgeError::NotEnoughSignatures))(&e), "{e:?}");
}

#[test]
fn release_with_mismatched_amount_recipient_or_nonce_fails() {
    let mut env = setup(1, 1);
    fund_vault(&mut env, 1_000_000);
    let recipient = Pubkey::new_unique();
    let rta = env.create_ata(&recipient);
    let s0 = env.signers[0].insecure_clone();
    let signed = withdraw_msg(0, &recipient, 1_000);
    let nonsig = custom(err_code(BridgeError::NotEnoughSignatures));

    // amount differs from what was signed
    let e = env.send(&[ed25519_ix(&[(&s0, &signed)]), env.release_ix(0, recipient, 999_999, rta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // nonce differs
    let e = env.send(&[ed25519_ix(&[(&s0, &signed)]), env.release_ix(1, recipient, 1_000, rta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // deposit-domain message is not a withdraw message
    let dep = bridge::bridge_message(bridge::DEPOSIT_DOMAIN, 0, &recipient, 1_000);
    let e = env.send(&[ed25519_ix(&[(&s0, &dep)]), env.release_ix(0, recipient, 1_000, rta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // token account owned by someone other than the signed recipient
    let other = Pubkey::new_unique();
    let ota = env.create_ata(&other);
    let e = env.send(&[ed25519_ix(&[(&s0, &signed)]), env.release_ix(0, recipient, 1_000, ota)], &[]).unwrap_err();
    assert!(custom(err_code(BridgeError::WrongRecipient))(&e), "{e:?}");

    assert_eq!(env.token_balance(&env.vault()), 1_000_000);
}

#[test]
fn threshold_two_of_three() {
    let mut env = setup(3, 2);
    fund_vault(&mut env, 10_000_000);
    let recipient = Pubkey::new_unique();
    let rta = env.create_ata(&recipient);
    let [a, b, c] = [0, 1, 2].map(|i| env.signers[i].insecure_clone());
    let msg = withdraw_msg(0, &recipient, 1_000_000);
    let nonsig = custom(err_code(BridgeError::NotEnoughSignatures));

    // one signature: below threshold
    let e = env.send(&[ed25519_ix(&[(&a, &msg)]), env.release_ix(0, recipient, 1_000_000, rta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // the same signer twice counts once
    let e = env.send(&[ed25519_ix(&[(&a, &msg), (&a, &msg)]), env.release_ix(0, recipient, 1_000_000, rta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // one valid signer plus a stranger
    let x = Keypair::new();
    let e = env.send(&[ed25519_ix(&[(&a, &msg), (&x, &msg)]), env.release_ix(0, recipient, 1_000_000, rta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");

    // two signers in one precompile instruction
    env.send(&[ed25519_ix(&[(&a, &msg), (&c, &msg)]), env.release_ix(0, recipient, 1_000_000, rta)], &[]).expect("2-of-3 in one ix");
    // two signers in two separate precompile instructions
    let msg1 = withdraw_msg(1, &recipient, 2_000_000);
    env.send(
        &[ed25519_ix(&[(&b, &msg1)]), ed25519_ix(&[(&c, &msg1)]), env.release_ix(1, recipient, 2_000_000, rta)],
        &[],
    )
    .expect("2-of-3 across ixs");
    assert_eq!(env.token_balance(&rta), 3_000_000);
    assert_eq!(env.token_balance(&env.vault()), 7_000_000);
}

#[test]
fn admin_edits_signers_threshold_and_admin() {
    let mut env = setup(1, 1);
    let old = env.signers[0].insecure_clone();
    let new_a = Keypair::new();
    let new_b = Keypair::new();
    let admin = env.admin.insecure_clone();

    // non-admin rejected
    let mallory = Keypair::new();
    env.svm.airdrop(&mallory.pubkey(), 1_000_000_000).unwrap();
    let e = env.send(&[env.set_signers_ix(&mallory.pubkey(), vec![mallory.pubkey()], 1)], &[&mallory]).unwrap_err();
    assert!(custom(err_code(BridgeError::NotAdmin))(&e), "{e:?}");

    // invalid sets rejected
    let e = env.send(&[env.set_signers_ix(&admin.pubkey(), vec![new_a.pubkey()], 2)], &[&admin]).unwrap_err();
    assert!(custom(err_code(BridgeError::BadThreshold))(&e), "{e:?}");
    let e = env.send(&[env.set_signers_ix(&admin.pubkey(), vec![new_a.pubkey(), new_a.pubkey()], 1)], &[&admin]).unwrap_err();
    assert!(custom(err_code(BridgeError::BadSignerSet))(&e), "{e:?}");
    let e = env.send(&[env.set_signers_ix(&admin.pubkey(), vec![], 0)], &[&admin]).unwrap_err();
    assert!(custom(err_code(BridgeError::BadSignerSet))(&e), "{e:?}");

    env.send(&[env.set_signers_ix(&admin.pubkey(), vec![new_a.pubkey(), new_b.pubkey()], 1)], &[&admin]).expect("set signers");
    let c = env.config();
    assert_eq!(c.signers, vec![new_a.pubkey(), new_b.pubkey()]);
    assert_eq!(c.threshold, 1);

    // old signer no longer accepted, new one is
    fund_vault(&mut env, 1_000_000);
    let recipient = Pubkey::new_unique();
    let rta = env.create_ata(&recipient);
    let msg = withdraw_msg(0, &recipient, 500);
    let e = env.send(&[ed25519_ix(&[(&old, &msg)]), env.release_ix(0, recipient, 500, rta)], &[]).unwrap_err();
    assert!(custom(err_code(BridgeError::NotEnoughSignatures))(&e), "{e:?}");
    env.send(&[ed25519_ix(&[(&new_b, &msg)]), env.release_ix(0, recipient, 500, rta)], &[]).expect("new signer works");

    // hand admin over; old admin then loses rights
    let next_admin = Keypair::new();
    let ix = Instruction {
        program_id: bridge::ID,
        accounts: bridge::accounts::AdminOnly { admin: admin.pubkey(), config: config_pda() }.to_account_metas(None),
        data: bridge::instruction::SetAdmin { new_admin: next_admin.pubkey() }.data(),
    };
    env.send(&[ix], &[&admin]).expect("set admin");
    assert_eq!(env.config().admin, next_admin.pubkey());
    let e = env.send(&[env.set_signers_ix(&admin.pubkey(), vec![old.pubkey()], 1)], &[&admin]).unwrap_err();
    assert!(custom(err_code(BridgeError::NotAdmin))(&e), "{e:?}");
}

fn refund_msg(nonce: u64, buyer: &Pubkey, amount: u64) -> Vec<u8> {
    bridge::bridge_message(REFUND_DOMAIN, nonce, buyer, amount)
}

#[test]
fn refund_deposit_returns_the_deposit_once_and_writes_the_refund_record() {
    let mut env = setup(1, 1);
    let user = fund_vault(&mut env, 3_000_000); // deposit 0
    let uta = ata(&user.pubkey(), &env.mint);
    assert_eq!(env.token_balance(&uta), 0);
    let s0 = env.signers[0].insecure_clone();
    let msg = refund_msg(0, &user.pubkey(), 3_000_000);
    assert_eq!(&msg[..19], b"pearl-sol:refund:v1");
    assert_eq!(msg.len(), 19 + 8 + 32 + 8);
    let meta = env.send(&[ed25519_ix(&[(&s0, &msg)]), env.refund_ix(0, user.pubkey(), uta)], &[]).expect("refund");
    println!("refund_deposit: {} CU", meta.compute_units_consumed);
    assert_eq!(env.token_balance(&uta), 3_000_000);
    assert_eq!(env.token_balance(&env.vault()), 0);
    let rec = bridge::refund_record_address(0);
    assert_eq!(rec, Pubkey::find_program_address(&[b"processed", &[CHAIN_TOKEN], &0u64.to_le_bytes()], &bridge::ID).0);
    let p = Processed::try_deserialize(&mut env.svm.get_account(&rec).unwrap().data.as_slice()).unwrap();
    assert_eq!((p.source_chain, p.nonce, p.recipient, p.amount), (CHAIN_TOKEN, 0, user.pubkey(), 3_000_000));
    // the withdrawal replay space (CHAIN_AGENT, nonce 0) is untouched
    assert!(env.svm.get_account(&processed_pda(0)).is_none());

    // a second refund of the same deposit fails on the existing record
    fund_vault(&mut env, 3_000_000); // refill the vault (deposit 1)
    let e = env.send(&[ed25519_ix(&[(&s0, &msg)]), env.refund_ix(0, user.pubkey(), uta)], &[]).unwrap_err();
    assert!(custom(0)(&e), "second refund must fail on the existing record: {e:?}");
    assert_eq!(env.token_balance(&uta), 3_000_000);
}

#[test]
fn refund_deposit_needs_signers_over_the_exact_message() {
    let mut env = setup(3, 2);
    let user = fund_vault(&mut env, 2_000_000);
    let uta = ata(&user.pubkey(), &env.mint);
    let [a, b] = [0, 1].map(|i| env.signers[i].insecure_clone());
    let nonsig = custom(err_code(BridgeError::NotEnoughSignatures));
    let good = refund_msg(0, &user.pubkey(), 2_000_000);
    // no signature, one of two, wrong amount, wrong domain (a deposit message)
    let e = env.send(&[env.refund_ix(0, user.pubkey(), uta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    let e = env.send(&[ed25519_ix(&[(&a, &good)]), env.refund_ix(0, user.pubkey(), uta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    let wrong_amt = refund_msg(0, &user.pubkey(), 1_000_000);
    let e = env.send(&[ed25519_ix(&[(&a, &wrong_amt), (&b, &wrong_amt)]), env.refund_ix(0, user.pubkey(), uta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    let dep = bridge::bridge_message(bridge::DEPOSIT_DOMAIN, 0, &user.pubkey(), 2_000_000);
    let e = env.send(&[ed25519_ix(&[(&a, &dep), (&b, &dep)]), env.refund_ix(0, user.pubkey(), uta)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // signed for the user, paid to someone else's token account
    let other = Pubkey::new_unique();
    let ota = env.create_ata(&other);
    let e = env.send(&[ed25519_ix(&[(&a, &good), (&b, &good)]), env.refund_ix(0, user.pubkey(), ota)], &[]).unwrap_err();
    assert!(custom(err_code(BridgeError::WrongRecipient))(&e), "{e:?}");
    // signed for another buyer (the token account matches that buyer): only with signatures over it
    let e = env.send(&[ed25519_ix(&[(&a, &good), (&b, &good)]), env.refund_ix(0, other, ota)], &[]).unwrap_err();
    assert!(nonsig(&e), "{e:?}");
    // a refund of a nonce with no deposit record fails
    let msg1 = refund_msg(1, &user.pubkey(), 2_000_000);
    let e = env.send(&[ed25519_ix(&[(&a, &msg1), (&b, &msg1)]), env.refund_ix(1, user.pubkey(), uta)], &[]).unwrap_err();
    assert!(matches!(e, TransactionError::InstructionError(..)), "{e:?}");
    assert_eq!(env.token_balance(&env.vault()), 2_000_000);
    // 2 of 3: refunded
    env.send(&[ed25519_ix(&[(&a, &good), (&b, &good)]), env.refund_ix(0, user.pubkey(), uta)], &[]).expect("2-of-3 refund");
    assert_eq!(env.token_balance(&uta), 2_000_000);
    assert_eq!(env.token_balance(&env.vault()), 0);
}

#[test]
fn refund_works_with_the_signer_release_path_off() {
    let mut env = setup(1, 1);
    let user = fund_vault(&mut env, 1_000_000);
    let uta = ata(&user.pubkey(), &env.mint);
    let admin = env.admin.insecure_clone();
    let ix = Instruction {
        program_id: bridge::ID,
        accounts: bridge::accounts::AdminOnly { admin: admin.pubkey(), config: config_pda() }.to_account_metas(None),
        data: bridge::instruction::SetReleaseModes { modes: bridge::MODE_PROOF, light_client: Pubkey::new_unique() }.data(),
    };
    env.send(&[ix], &[&admin]).expect("proof only");
    let s0 = env.signers[0].insecure_clone();
    let msg = refund_msg(0, &user.pubkey(), 1_000_000);
    env.send(&[ed25519_ix(&[(&s0, &msg)]), env.refund_ix(0, user.pubkey(), uta)], &[]).expect("refund is inbound: always attested");
    assert_eq!(env.token_balance(&uta), 1_000_000);
}
