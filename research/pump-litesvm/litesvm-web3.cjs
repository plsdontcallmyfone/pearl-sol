// LiteSVM with the @solana/web3.js API these tests were written against (litesvm 0.3.3).
// litesvm 1.x speaks @solana/kit (string addresses, kit transactions and EncodedAccount). This
// keeps the 0.3.3 wrapper's exact behaviour (PublicKey in, AccountInfo out, web3.js transactions
// sent as their wire bytes) on top of 1.x's native binding, which has the same byte-level methods.
// Why 1.x: 0.3.3's linux-x64 binding corrupted memory (LiteSVM/litesvm#171, the x87 state the
// SBPF JIT left behind; cleared after every transaction since 1.4.0, LiteSVM/litesvm#398).
// Copies of this file: token/tests/{mandate,reserve,storefront,town}/litesvm-web3.cjs,
// research/pump-litesvm/litesvm-web3.cjs; services/{relayer,storefront-x402}/test/litesvm-web3.ts.
'use strict';
const base = require('litesvm');
const { PublicKey, Transaction } = require('@solana/web3.js');

// rentEpoch comes out as a JS number, so u64::MAX (what Agave 4 gives rent-exempt accounts)
// reads back as 2^64; clamp it so an account read with getAccount can be written back.
const U64_MAX = (1n << 64n) - 1n;
function rentEpochU64(e) {
  const v = BigInt(e || 0);
  return v > U64_MAX ? U64_MAX : v;
}

function toAccountInfo(acc) {
  return {
    executable: acc.executable(),
    owner: new PublicKey(acc.owner()),
    lamports: Number(acc.lamports()),
    data: acc.data(),
    rentEpoch: Number(acc.rentEpoch()),
  };
}

class LiteSVM {
  constructor() {
    this.inner = new base.LiteSVM().inner;
    if (!this.inner || typeof this.inner.sendLegacyTransaction !== 'function') {
      throw new Error('litesvm-web3: unexpected litesvm version (no native binding on LiteSVM.inner)');
    }
  }
  minimumBalanceForRentExemption(dataLen) { return this.inner.minimumBalanceForRentExemption(dataLen); }
  getAccount(address) {
    const a = this.inner.getAccount(address.toBytes());
    return a === null ? null : toAccountInfo(a);
  }
  setAccount(address, acc) {
    this.inner.setAccount(address.toBytes(),
      new base.Account(BigInt(acc.lamports), acc.data, acc.owner.toBytes(), acc.executable, rentEpochU64(acc.rentEpoch)));
  }
  getBalance(address) { return this.inner.getBalance(address.toBytes()); }
  latestBlockhash() { return this.inner.latestBlockhash(); }
  airdrop(address, lamports) { return this.inner.airdrop(address.toBytes(), lamports); }
  addProgramFromFile(programId, path) { return this.inner.addProgramFromFile(programId.toBytes(), path); }
  addProgram(programId, bytes) { return this.inner.addProgram(programId.toBytes(), bytes); }
  sendTransaction(tx) {
    const raw = tx.serialize({ requireAllSignatures: true, verifySignatures: this.inner.getSigverify() });
    return tx instanceof Transaction ? this.inner.sendLegacyTransaction(raw) : this.inner.sendVersionedTransaction(raw);
  }
  expireBlockhash() { this.inner.expireBlockhash(); }
  warpToSlot(slot) { this.inner.warpToSlot(slot); }
  getClock() { return this.inner.getClock(); }
  setClock(clock) { this.inner.setClock(clock); }
  getRent() { return this.inner.getRent(); }
  setRent(rent) { this.inner.setRent(rent); }
}

module.exports = {
  LiteSVM,
  Clock: base.Clock,
  FailedTransactionMetadata: base.FailedTransactionMetadata,
  TransactionMetadata: base.TransactionMetadata,
};
