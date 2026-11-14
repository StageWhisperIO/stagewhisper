use std::collections::VecDeque;

pub(crate) const MAX_SOURCE_LEAD_SAMPLES: usize = 3200;

pub(crate) struct EndpointMixer {
    pending: Vec<VecDeque<i16>>,
    max_lead: usize,
}

impl EndpointMixer {
    pub(crate) fn new(max_lead: usize) -> Self {
        Self {
            pending: Vec::new(),
            max_lead,
        }
    }

    pub(crate) fn reset(&mut self, sources: usize) {
        self.pending = (0..sources).map(|_| VecDeque::new()).collect();
    }

    pub(crate) fn push(&mut self, source: usize, samples: &[i16]) -> Vec<i16> {
        if source >= self.pending.len() {
            return Vec::new();
        }
        if self.pending.len() == 1 {
            return samples.to_vec();
        }
        self.pending[source].extend(samples.iter().copied());

        let mut mixed = Vec::new();
        loop {
            let ready = self.pending.iter().map(VecDeque::len).min().unwrap_or(0);
            let longest = self.pending.iter().map(VecDeque::len).max().unwrap_or(0);
            let take = if ready > 0 {
                ready
            } else {
                longest.saturating_sub(self.max_lead)
            };
            if take == 0 {
                break;
            }
            mixed.extend((0..take).map(|_| self.mix_front()));
        }
        mixed
    }

    fn mix_front(&mut self) -> i16 {
        let sum: i32 = self
            .pending
            .iter_mut()
            .map(|queue| queue.pop_front().map_or(0, i32::from))
            .sum();
        clip_to_i16(sum)
    }
}

pub(crate) fn clip_to_i16(sum: i32) -> i16 {
    sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixer(sources: usize) -> EndpointMixer {
        let mut mixer = EndpointMixer::new(MAX_SOURCE_LEAD_SAMPLES);
        mixer.reset(sources);
        mixer
    }

    #[test]
    fn a_single_endpoint_passes_through_untouched() {
        let mut mixer = mixer(1);
        assert_eq!(mixer.push(0, &[1, -2, 3]), vec![1, -2, 3]);
    }

    #[test]
    fn two_endpoints_are_summed_sample_by_sample() {
        let mut mixer = mixer(2);
        assert!(mixer.push(0, &[100, 200, 300]).is_empty());
        assert_eq!(mixer.push(1, &[10, 20]), vec![110, 220]);
        assert_eq!(mixer.push(1, &[30, 40]), vec![330]);
        assert_eq!(mixer.push(0, &[400]), vec![440]);
    }

    #[test]
    fn the_sum_is_clipped_instead_of_wrapping() {
        let mut mixer = mixer(2);
        mixer.push(0, &[30_000, -30_000, 1_000]);
        assert_eq!(
            mixer.push(1, &[30_000, -30_000, 1_000]),
            vec![i16::MAX, i16::MIN, 2_000]
        );
    }

    #[test]
    fn an_endpoint_that_stops_delivering_holds_the_other_back_by_at_most_the_lead() {
        let mut mixer = mixer(2);
        let chunk = vec![7_i16; 320];
        let mut emitted = 0;
        for _ in 0..40 {
            emitted += mixer.push(0, &chunk).len();
        }
        let delivered = 40 * chunk.len();
        assert_eq!(emitted, delivered - MAX_SOURCE_LEAD_SAMPLES);
        assert!(mixer.push(0, &chunk).iter().all(|&sample| sample == 7));
    }

    #[test]
    fn audio_from_an_endpoint_dropped_by_a_rebind_is_ignored() {
        let mut mixer = mixer(2);
        mixer.push(1, &[5, 5]);
        mixer.reset(1);
        assert!(mixer.push(1, &[9, 9]).is_empty());
        assert_eq!(mixer.push(0, &[3]), vec![3]);
    }

    #[test]
    fn a_rebind_discards_audio_still_waiting_for_its_partner() {
        let mut mixer = mixer(2);
        mixer.push(0, &[1_000, 1_000]);
        mixer.reset(2);
        assert!(mixer.push(1, &[1]).is_empty());
        assert_eq!(mixer.push(0, &[2]), vec![3]);
    }

    #[test]
    fn no_sources_yields_nothing() {
        let mut mixer = mixer(0);
        assert!(mixer.push(0, &[1, 2, 3]).is_empty());
    }
}
