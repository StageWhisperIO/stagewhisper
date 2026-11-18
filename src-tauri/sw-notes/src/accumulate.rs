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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
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
                speaker_id: None,
                speaker_label: None,
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

    #[test]
    fn old_segment_without_speaker_fields_deserializes_to_none() {
        let legacy = serde_json::json!({
            "source": "others",
            "utterance": "hello there",
        });
        let segment: TranscriptSegment = serde_json::from_value(legacy).unwrap();
        assert_eq!(segment.source, TranscriptSource::Others);
        assert_eq!(segment.utterance, "hello there");
        assert_eq!(segment.speaker_id, None);
        assert_eq!(segment.speaker_label, None);
    }

    #[test]
    fn segment_without_speaker_fields_skips_them_when_serialized() {
        let segment = TranscriptSegment {
            source: TranscriptSource::You,
            utterance: "hi".to_string(),
            speaker_id: None,
            speaker_label: None,
        };
        let value = serde_json::to_value(&segment).unwrap();
        let object = value.as_object().unwrap();
        assert!(!object.contains_key("speaker_id"));
        assert!(!object.contains_key("speaker_label"));
    }
}
