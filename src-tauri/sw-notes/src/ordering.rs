use crate::accumulate::TranscriptSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedUtterance {
    pub source: TranscriptSource,
    pub text: String,
    pub captured_at_ms: u64,
}

fn slot(source: TranscriptSource) -> usize {
    match source {
        TranscriptSource::You => 0,
        TranscriptSource::Others => 1,
    }
}

#[derive(Debug, Default)]
pub struct TranscriptOrderer {
    pending: Vec<OrderedUtterance>,
    processed_to_ms: [Option<u64>; 2],
    expected: [bool; 2],
    max_wait_ms: u64,
    released_late: u64,
}

impl TranscriptOrderer {
    pub fn new(max_wait_ms: u64, expected_sources: &[TranscriptSource]) -> Self {
        let mut expected = [false; 2];
        for source in expected_sources {
            expected[slot(*source)] = true;
        }
        Self {
            max_wait_ms,
            expected,
            ..Self::default()
        }
    }

    pub fn note_processed(&mut self, source: TranscriptSource, captured_at_ms: u64) {
        let current = &mut self.processed_to_ms[slot(source)];
        *current = Some(current.map_or(captured_at_ms, |seen| seen.max(captured_at_ms)));
    }

    pub fn push(&mut self, source: TranscriptSource, text: &str, captured_at_ms: u64) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        self.pending.push(OrderedUtterance {
            source,
            text: trimmed.to_string(),
            captured_at_ms,
        });
    }

    pub fn is_waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn released_late(&self) -> u64 {
        self.released_late
    }

    pub fn release(&mut self, now_ms: u64) -> Vec<OrderedUtterance> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let settled = self.settled_through();
        let forced = now_ms.saturating_sub(self.max_wait_ms);
        self.pending
            .sort_by_key(|utterance| utterance.captured_at_ms);
        let ready = self
            .pending
            .partition_point(|utterance| utterance.captured_at_ms <= settled.max(forced));
        let released: Vec<OrderedUtterance> = self.pending.drain(..ready).collect();
        self.released_late += released
            .iter()
            .filter(|utterance| utterance.captured_at_ms > settled)
            .count() as u64;
        released
    }

    pub fn drain(&mut self) -> Vec<OrderedUtterance> {
        self.pending
            .sort_by_key(|utterance| utterance.captured_at_ms);
        std::mem::take(&mut self.pending)
    }

    fn settled_through(&self) -> u64 {
        self.expected
            .iter()
            .enumerate()
            .filter(|(_, expected)| **expected)
            .map(|(index, _)| self.processed_to_ms[index].unwrap_or(0))
            .min()
            .unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_WAIT_MS: u64 = 5_000;

    fn orderer() -> TranscriptOrderer {
        TranscriptOrderer::new(
            MAX_WAIT_MS,
            &[TranscriptSource::You, TranscriptSource::Others],
        )
    }

    fn room_only() -> TranscriptOrderer {
        TranscriptOrderer::new(MAX_WAIT_MS, &[TranscriptSource::Others])
    }

    fn texts(released: Vec<OrderedUtterance>) -> Vec<String> {
        released
            .into_iter()
            .map(|utterance| utterance.text)
            .collect()
    }

    #[test]
    fn an_utterance_is_held_until_the_other_side_has_been_heard_that_far() {
        let mut orderer = orderer();
        orderer.note_processed(TranscriptSource::You, 5_000);
        orderer.note_processed(TranscriptSource::Others, 1_000);
        orderer.push(TranscriptSource::You, "my answer", 4_000);

        assert!(texts(orderer.release(5_000)).is_empty());

        orderer.note_processed(TranscriptSource::Others, 4_500);
        assert_eq!(texts(orderer.release(5_000)), vec!["my answer".to_string()]);
    }

    #[test]
    fn a_lagging_pipeline_no_longer_pushes_its_speech_behind_later_speech() {
        let mut orderer = orderer();
        orderer.note_processed(TranscriptSource::You, 3_000);
        orderer.push(TranscriptSource::You, "spoken second", 2_500);
        assert!(texts(orderer.release(3_000)).is_empty());

        orderer.note_processed(TranscriptSource::Others, 3_000);
        orderer.push(TranscriptSource::Others, "spoken first", 1_000);

        assert_eq!(
            texts(orderer.release(3_000)),
            vec!["spoken first".to_string(), "spoken second".to_string()]
        );
    }

    #[test]
    fn a_session_with_the_microphone_off_never_waits_for_a_side_that_is_not_listening() {
        let mut orderer = room_only();
        orderer.note_processed(TranscriptSource::Others, 2_000);
        orderer.push(TranscriptSource::Others, "the room", 2_000);

        assert_eq!(texts(orderer.release(2_000)), vec!["the room".to_string()]);
    }

    #[test]
    fn speech_at_the_very_start_waits_for_the_side_that_has_not_reported_yet() {
        let mut orderer = orderer();
        orderer.note_processed(TranscriptSource::Others, 2_000);
        orderer.push(TranscriptSource::Others, "the room", 2_000);

        assert!(texts(orderer.release(2_000)).is_empty());

        orderer.note_processed(TranscriptSource::You, 2_000);
        assert_eq!(texts(orderer.release(2_000)), vec!["the room".to_string()]);
    }

    #[test]
    fn a_side_that_stops_reporting_cannot_hold_the_transcript_hostage_forever() {
        let mut orderer = orderer();
        orderer.note_processed(TranscriptSource::You, 10_000);
        orderer.note_processed(TranscriptSource::Others, 1_000);
        orderer.push(TranscriptSource::You, "still speaking", 9_000);

        assert!(texts(orderer.release(10_000)).is_empty());
        assert_eq!(orderer.released_late(), 0);

        assert_eq!(
            texts(orderer.release(9_000 + MAX_WAIT_MS)),
            vec!["still speaking".to_string()]
        );
        assert_eq!(orderer.released_late(), 1);
    }

    #[test]
    fn utterances_captured_at_the_same_moment_keep_the_order_they_arrived_in() {
        let mut orderer = orderer();
        orderer.note_processed(TranscriptSource::You, 1_000);
        orderer.note_processed(TranscriptSource::Others, 1_000);
        orderer.push(TranscriptSource::Others, "first", 1_000);
        orderer.push(TranscriptSource::You, "second", 1_000);

        assert_eq!(
            texts(orderer.release(1_000)),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn shutting_down_gives_back_everything_still_waiting_in_capture_order() {
        let mut orderer = orderer();
        orderer.push(TranscriptSource::You, "later", 2_000);
        orderer.push(TranscriptSource::Others, "earlier", 1_000);

        assert!(orderer.is_waiting());
        assert_eq!(
            texts(orderer.drain()),
            vec!["earlier".to_string(), "later".to_string()]
        );
        assert!(!orderer.is_waiting());
    }

    #[test]
    fn blank_speech_is_never_buffered() {
        let mut orderer = orderer();
        orderer.push(TranscriptSource::You, "   ", 1_000);
        assert!(!orderer.is_waiting());
    }
}
