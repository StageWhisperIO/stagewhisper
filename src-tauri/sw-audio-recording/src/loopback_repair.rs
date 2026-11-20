use std::time::{Duration, Instant};

pub(crate) const FIRST_RETRY: Duration = Duration::from_secs(2);
const MAX_RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub(crate) struct EndpointRepair {
    next_attempt: Option<Instant>,
    failed_attempts: u32,
}

impl EndpointRepair {
    pub(crate) fn schedule(&mut self, now: Instant) {
        self.failed_attempts = 0;
        self.next_attempt = Some(now + FIRST_RETRY);
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn due_at(&self) -> Option<Instant> {
        self.next_attempt
    }

    pub(crate) fn attempt_failed(&mut self, now: Instant) -> Duration {
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        let delay = retry_delay(self.failed_attempts);
        self.next_attempt = Some(now + delay);
        delay
    }
}

fn retry_delay(failed_attempts: u32) -> Duration {
    FIRST_RETRY
        .saturating_mul(1 << failed_attempts.min(16))
        .min(MAX_RETRY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_due_until_a_partial_bind_schedules_a_retry() {
        let repair = EndpointRepair::default();
        assert_eq!(repair.due_at(), None);
    }

    #[test]
    fn first_retry_follows_a_partial_bind_quickly() {
        let now = Instant::now();
        let mut repair = EndpointRepair::default();
        repair.schedule(now);
        assert_eq!(repair.due_at(), Some(now + FIRST_RETRY));
    }

    #[test]
    fn failed_retries_back_off_up_to_the_cap() {
        let now = Instant::now();
        let mut repair = EndpointRepair::default();
        repair.schedule(now);
        let delays: Vec<u64> = (0..7)
            .map(|_| repair.attempt_failed(now).as_secs())
            .collect();
        assert_eq!(delays, vec![4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(repair.due_at(), Some(now + MAX_RETRY));
    }

    #[test]
    fn retries_keep_going_for_the_whole_session() {
        let now = Instant::now();
        let mut repair = EndpointRepair::default();
        repair.schedule(now);
        for _ in 0..1000 {
            repair.attempt_failed(now);
        }
        assert_eq!(repair.due_at(), Some(now + MAX_RETRY));
    }

    #[test]
    fn a_new_partial_bind_restarts_the_backoff() {
        let now = Instant::now();
        let mut repair = EndpointRepair::default();
        repair.schedule(now);
        repair.attempt_failed(now);
        repair.attempt_failed(now);
        repair.schedule(now);
        assert_eq!(repair.attempt_failed(now), Duration::from_secs(4));
    }

    #[test]
    fn a_full_bind_clears_the_retry() {
        let now = Instant::now();
        let mut repair = EndpointRepair::default();
        repair.schedule(now);
        repair.clear();
        assert_eq!(repair.due_at(), None);
    }
}
