//! Balance-ticket row bound (SPEC 4.5 / 4.6, ECOSYSTEM M1.5).
//!
//! A ticket in balance form (`n = 1`, so `m = R - 1`) may carry up to the config's
//! `max_balance_rows` rows (default 4096, admin-editable from the social upgrade height);
//! every other ticket keeps `R <= MAX_TICKET_ROWS` (4096). Pay, refund and segment caps
//! are unchanged: they are pro rata in u128 (settle_ticket_rows, standing::segment_cap),
//! so a larger R changes no rounding rule.

use agent_types::PostJob;
use state::StateRead;

use crate::BlockExecutor;

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    /// Most rows a ticket with this body may budget.
    pub(crate) fn ticket_row_limit(&self, p: &PostJob) -> u64 {
        let base = pearl_job::standing::MAX_TICKET_ROWS as u64;
        if p.n == 1 {
            base.max(self.cfg.max_balance_rows as u64)
        } else {
            base
        }
    }
}
