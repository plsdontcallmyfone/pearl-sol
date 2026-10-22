//! AdminConfig keys 48-49: the blob data-lifecycle parameters (SPEC 4.5 "Segment data
//! retention"), kept in their own record (state::blobparams). Only node policy reads them
//! (which blobs a node keeps); the executor only stores them.
//!
//! Rules: the sender is the admin; the block height is at or above the chain's
//! `blob_params_upgrade_height` (Cadre devnet 5800; below it the key is refused with the
//! message an older node gives, "unknown config key"); key 49 is reserved; the value is in
//! range. A record equal to the defaults is deleted, so the state matches a chain where no
//! admin ever set one.

use agent_types::Pubkey;
use state::blobparams::{self as bp, BlobParams};
use state::StateRead;

use crate::{bad, BlockExecutor, TxError};

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    pub(crate) async fn admin_blob_config(&mut self, from: &Pubkey, key: u8, value: &[u8]) -> Result<(), TxError> {
        if *from != self.cfg.admin {
            return bad("not the admin");
        }
        if self.env.height < bp::blob_params_upgrade_height(self.env.chain_id) || key > bp::keys::LAST {
            return bad("unknown config key");
        }
        let mut p = match self.ov.get(&bp::blob_params_key()).await? {
            Some(b) => BlobParams::decode(&b).map_err(|e| state::StateError::Corrupt(e.to_string()))?,
            None => BlobParams::default(),
        };
        p.set(key, value).map_err(TxError::Invalid)?;
        let v = if p == BlobParams::default() { None } else { Some(p.encode()) };
        self.ov.put(bp::blob_params_key(), v);
        Ok(())
    }
}
