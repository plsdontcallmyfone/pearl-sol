//! RejectResult (SPEC 4.1): the job's evaluators reject its current delivery.
//!
//! Rules:
//! - the job is Delivered (plain job or inference ticket; a segment has no delivery);
//! - the tx's `deliverer` and `result_hash` are the job's current ones;
//! - `threshold` distinct valid signatures from the job's evaluators over
//!   `b"pearl-sol:reject:v1" || job_id || deliverer || result_hash` (raw ed25519, the
//!   CompleteJob rule under another domain);
//! - effect: the job goes back to Open (same deadline, W stays escrowed, mining goes on),
//!   deliverer / result are cleared, a `Reject` record is appended for the job and the
//!   deliverer's `RejectStats` grows. On a plain job the rejected deliverer may not deliver
//!   to that job again (SubmitResult refuses it), so one key cannot hold a job open by
//!   resubmitting; a ticket's server may (only it can deliver a ticket).
//! - any account may send it (it pays the fee); the signatures are the authority.

use agent_types::tx::reject_message;
use agent_types::{AttesterSig, Hash, Pubkey};
use state::reject::{self, Reject, RejectStats};
use state::schema::JobStatus;
use state::StateRead;

use crate::{bad, check_sigs, BlockExecutor, TxError};

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    async fn reject_count(&self, job_id: &Hash) -> Result<u32, TxError> {
        Ok(match self.ov.get(&reject::reject_count_key(job_id)).await? {
            Some(b) => reject::decode_count(&b).map_err(|e| state::StateError::Corrupt(e.to_string()))?,
            None => 0,
        })
    }

    /// Has `who` had a delivery to `job_id` rejected?
    pub(crate) async fn was_rejected(&self, job_id: &Hash, who: &Pubkey) -> Result<bool, TxError> {
        let n = self.reject_count(job_id).await?;
        for i in 0..n {
            if let Some(b) = self.ov.get(&reject::reject_key(job_id, i)).await? {
                let r = Reject::decode(&b).map_err(|e| state::StateError::Corrupt(e.to_string()))?;
                if r.deliverer == *who {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub(crate) async fn reject_result(
        &mut self,
        h: u64,
        job_id: &Hash,
        deliverer: &Pubkey,
        result_hash: &Hash,
        sigs: &[AttesterSig],
    ) -> Result<(), TxError> {
        let Some(mut j) = self.ov.job(job_id).await? else { return bad("unknown job") };
        if j.segment().is_some() {
            return bad("a segment has no delivery to reject");
        }
        if j.status != JobStatus::Delivered {
            return bad("job is not delivered");
        }
        if j.deliverer != *deliverer || j.result_hash != *result_hash {
            return bad("deliverer / result_hash are not the job's current delivery");
        }
        let msg = reject_message(job_id, deliverer, result_hash);
        check_sigs(sigs, &j.evaluators, j.threshold, &msg)?;

        let index = self.reject_count(job_id).await?;
        let rec = Reject {
            deliverer: *deliverer,
            result_hash: *result_hash,
            height: h,
            protocol_fee: j.protocol_fee,
            signers: sigs.iter().map(|s| s.pubkey).collect(),
        };
        self.ov.put(reject::reject_key(job_id, index), Some(rec.encode()));
        self.ov.put(reject::reject_count_key(job_id), Some(reject::encode_count(index + 1)));

        let sk = reject::reject_stats_key(deliverer);
        let mut st = match self.ov.get(&sk).await? {
            Some(b) => RejectStats::decode(&b).map_err(|e| state::StateError::Corrupt(e.to_string()))?,
            None => RejectStats { first_height: h, ..Default::default() },
        };
        st.rejected += 1;
        st.rejected_fee = st.rejected_fee.saturating_add(j.protocol_fee);
        st.last_height = h;
        self.ov.put(sk, Some(st.encode()));

        j.status = JobStatus::Open;
        j.deliverer = [0; 32];
        j.result_hash = [0; 32];
        j.result_uri = Vec::new();
        self.ov.put_job(&j);
        Ok(())
    }
}
