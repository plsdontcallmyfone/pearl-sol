//! Reward math (SPEC 3.5) as pure functions. All floors; nothing here can pay out
//! more than it was given.
//!
//! - Epoch budget `E = pool_free x emission_bps / 10_000`, snapshotted at epoch start.
//! - Per job per epoch `S = min(E x P_j / sum_P, cap_remaining_j)`, where
//!   `cap_remaining_j = alpha x P_j - subsidy already paid to job j` over its whole
//!   life (not per epoch). That lifetime cap is what makes self-dealing lose.
//! - Within a job, S is split by share count; the last claimer takes the rounding
//!   remainder so the claims add up to exactly S.

pub const BPS: u64 = 10_000;

pub fn epoch_budget(pool_free: u64, emission_bps: u16) -> u64 {
    ((pool_free as u128 * emission_bps as u128) / BPS as u128) as u64
}

/// Lifetime subsidy cap for a job: alpha x P.
pub fn job_cap(protocol_fee: u64, alpha_bps: u16) -> u64 {
    ((protocol_fee as u128 * alpha_bps as u128) / BPS as u128) as u64
}

/// E_share for one job: E split across active jobs by their fee P.
pub fn fee_share(budget: u64, fee_j: u64, total_fee: u64) -> u64 {
    if total_fee == 0 {
        return 0;
    }
    ((budget as u128 * fee_j as u128) / total_fee as u128) as u64
}

/// S_job = min(E_share, remaining lifetime cap).
pub fn job_subsidy(budget: u64, fee_j: u64, total_fee: u64, cap_remaining: u64) -> u64 {
    fee_share(budget, fee_j, total_fee).min(cap_remaining)
}

/// One miner's cut of a job's epoch subsidy.
/// `claimed_before` / `paid_before` are what earlier claimers already took.
pub fn miner_reward(subsidy: u64, count: u64, total_shares: u64, claimed_before: u64, paid_before: u64) -> u64 {
    if total_shares == 0 || count == 0 {
        return 0;
    }
    if claimed_before + count >= total_shares {
        return subsidy.saturating_sub(paid_before);
    }
    ((subsidy as u128 * count as u128) / total_shares as u128) as u64
}

/// Net lamports of an agent who posts a job (W, P) and is also the only miner and
/// the deliverer: W comes back, P is gone, the subsidy is earned.
pub fn self_dealer_net(protocol_fee: u64, subsidy_received: u64) -> i128 {
    subsidy_received as i128 - protocol_fee as i128
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*, deterministic, no extra deps.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }

    #[test]
    fn budget_and_cap() {
        assert_eq!(epoch_budget(1_000_000_000, 42), 4_200_000);
        assert_eq!(epoch_budget(0, 42), 0);
        assert_eq!(epoch_budget(u64::MAX, 10_000), u64::MAX);
        assert_eq!(job_cap(1_000, 8_000), 800);
        assert_eq!(job_cap(999, 8_000), 799);
    }

    #[test]
    fn fee_share_splits_by_p() {
        // E = 1000; jobs with P 1, 3 -> 250, 750
        assert_eq!(fee_share(1000, 1, 4), 250);
        assert_eq!(fee_share(1000, 3, 4), 750);
        assert_eq!(fee_share(1000, 3, 0), 0);
    }

    #[test]
    fn subsidy_capped_by_alpha_p() {
        // big pool, small fee: cap binds
        assert_eq!(job_subsidy(1_000_000, 100, 100, job_cap(100, 8000)), 80);
        // small pool: E share binds
        assert_eq!(job_subsidy(10, 100, 100, job_cap(100, 8000)), 10);
    }

    #[test]
    fn miner_split_sums_exactly() {
        let s = 1_000_003u64;
        let counts = [3u64, 7, 1, 11];
        let total: u64 = counts.iter().sum();
        let (mut claimed, mut paid) = (0, 0);
        for c in counts {
            let r = miner_reward(s, c, total, claimed, paid);
            claimed += c;
            paid += r;
        }
        assert_eq!(paid, s);
        // order does not change the sum
        let (mut claimed, mut paid) = (0, 0);
        for c in counts.iter().rev() {
            paid += miner_reward(s, *c, total, claimed, paid);
            claimed += c;
        }
        assert_eq!(paid, s);
    }

    /// Anti self-dealing (SPEC 3.5): post a job, mine it yourself in every epoch,
    /// be the only active job, deliver and get W back. Over any number of epochs and
    /// any pool size, the net is at most -(1 - alpha) x P.
    #[test]
    fn self_dealer_always_loses_one_minus_alpha_p() {
        let mut rng = Rng(0x9E3779B97F4A7C15);
        for _ in 0..20_000 {
            let alpha_bps = (rng.below(9_999) + 1) as u16; // 1..=9999, alpha < 1
            let p = rng.below(1_000_000_000_000) + 1;
            let emission_bps = rng.below(10_001) as u16;
            let mut pool = rng.below(1_000_000_000_000_000) + p;
            let epochs = rng.below(400) + 1;
            let cap = job_cap(p, alpha_bps);
            let mut received = 0u64;
            for _ in 0..epochs {
                let e = epoch_budget(pool, emission_bps);
                // other honest jobs active this epoch (possibly none)
                let others = rng.below(3) * rng.below(1_000_000_000);
                let s = job_subsidy(e, p, p + others, cap - received);
                received += s;
                pool -= s;
            }
            assert!(received <= cap);
            let net = self_dealer_net(p, received);
            // net <= -(P - alpha P) where alpha P is floored, so the loss is at least (1 - alpha) P
            let min_loss = p as i128 - cap as i128;
            assert!(net <= -min_loss, "net {net} min_loss {min_loss}");
            assert!(min_loss as u128 * BPS as u128 >= p as u128 * (BPS - alpha_bps as u64) as u128);
        }
    }

    #[test]
    fn self_dealer_exact_numbers() {
        // W = 5, P = 1000, alpha = 0.8, huge pool: gets exactly 800 back, loses 200
        let cap = job_cap(1000, 8000);
        let s1 = job_subsidy(epoch_budget(1_000_000_000, 42), 1000, 1000, cap);
        assert_eq!(s1, 800);
        let s2 = job_subsidy(epoch_budget(1_000_000_000, 42), 1000, 1000, cap - s1);
        assert_eq!(s2, 0);
        assert_eq!(self_dealer_net(1000, s1 + s2), -200);
    }
}
