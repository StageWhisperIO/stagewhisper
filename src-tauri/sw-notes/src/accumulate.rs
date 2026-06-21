use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptSource {
    You,
    Others,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub source: TranscriptSource,
    pub utterance: String,
}

#[derive(Default)]
pub struct TranscriptAccumulator {
    segments: Vec<TranscriptSegment>,
}

impl TranscriptAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_final(&mut self, source: TranscriptSource, text: &str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            self.segments.push(TranscriptSegment {
                source,
                utterance: trimmed.to_string(),
            });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn segments(&self) -> &[TranscriptSegment] {
        &self.segments
    }

    pub fn into_segments(self) -> Vec<TranscriptSegment> {
        self.segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_finals_and_skips_blanks() {
        let mut acc = TranscriptAccumulator::new();
        acc.push_final(TranscriptSource::You, "hello");
        acc.push_final(TranscriptSource::Others, "   ");
        acc.push_final(TranscriptSource::You, "world");
        assert!(!acc.is_empty());
        assert_eq!(acc.segments().len(), 2);
    }

    #[test]
    fn empty_by_default() {
        let acc = TranscriptAccumulator::new();
        assert!(acc.is_empty());
        assert_eq!(acc.segments().len(), 0);
    }
}
