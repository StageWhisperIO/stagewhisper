use crate::accumulate::{TranscriptSegment, TranscriptSource};
use crate::store::InsightNote;

const UTTERANCE_EXCERPT_MAX: usize = 160;

pub fn render_suggestions_section(
    insights: &[InsightNote],
    segments: &[TranscriptSegment],
) -> Option<String> {
    if insights.is_empty() {
        return None;
    }
    let mut out = String::from("## Suggestions\n");
    for note in insights {
        let message = note.message.trim();
        if message.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n- **{}** {}\n",
            format_offset(note.offset_ms),
            message
        ));
        if let Some(segment) = note.segment_index.and_then(|index| segments.get(index)) {
            out.push_str(&format!(
                "  > {}: \"{}\"\n",
                speaker_name(segment),
                excerpt(&segment.utterance)
            ));
        }
    }
    if out.lines().count() <= 1 {
        return None;
    }
    Some(out)
}

fn speaker_name(segment: &TranscriptSegment) -> String {
    if let Some(label) = segment
        .speaker_label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        return label.to_string();
    }
    match segment.source {
        TranscriptSource::You => "You".to_string(),
        TranscriptSource::Others => "Others".to_string(),
    }
}

fn format_offset(offset_ms: u64) -> String {
    let total_seconds = offset_ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn excerpt(utterance: &str) -> String {
    let collapsed = utterance.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= UTTERANCE_EXCERPT_MAX {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(UTTERANCE_EXCERPT_MAX).collect();
    let cut = truncated.rfind(' ').unwrap_or(truncated.len());
    format!("{}…", &truncated[..cut])
}

pub fn split_insights(
    insights: &[InsightNote],
    at_index: usize,
) -> (Vec<InsightNote>, Vec<InsightNote>) {
    let mut first = Vec::new();
    let mut second = Vec::new();
    for note in insights {
        match note.segment_index {
            Some(index) if index >= at_index => second.push(InsightNote {
                segment_index: Some(index - at_index),
                ..note.clone()
            }),
            Some(_) => first.push(note.clone()),
            None => {}
        }
    }
    (first, second)
}

pub fn merge_insights(
    first: &[InsightNote],
    second: &[InsightNote],
    first_segment_count: usize,
    second_start_delta_ms: u64,
) -> Vec<InsightNote> {
    let mut merged = first.to_vec();
    merged.extend(second.iter().map(|note| InsightNote {
        offset_ms: note.offset_ms.saturating_add(second_start_delta_ms),
        segment_index: note.segment_index.map(|index| index + first_segment_count),
        ..note.clone()
    }));
    merged
}

pub fn append_missing_insights(target: &mut Vec<InsightNote>, incoming: &[InsightNote]) -> bool {
    let mut changed = false;
    for note in incoming {
        let exists = target
            .iter()
            .any(|existing| existing.offset_ms == note.offset_ms && existing.message == note.message);
        if !exists {
            target.push(note.clone());
            changed = true;
        }
    }
    changed
}

pub fn rfc3339_epoch_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp_millis().max(0) as u64)
}

pub fn rfc3339_delta_ms(base: &str, later: &str) -> u64 {
    let parse = |value: &str| {
        chrono::DateTime::parse_from_rfc3339(value)
            .map(|dt| dt.timestamp_millis())
            .ok()
    };
    match (parse(base), parse(later)) {
        (Some(base_ms), Some(later_ms)) => later_ms.saturating_sub(base_ms).max(0) as u64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(source: TranscriptSource, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            source,
            utterance: text.to_string(),
            speaker_id: None,
            speaker_label: None,
        }
    }

    fn note(offset_ms: u64, message: &str, segment_index: Option<usize>) -> InsightNote {
        InsightNote {
            offset_ms,
            severity: "green".to_string(),
            message: message.to_string(),
            segment_index,
        }
    }

    #[test]
    fn split_routes_anchored_and_drops_unanchored() {
        let insights = vec![
            note(1_000, "early anchored", Some(0)),
            note(50_000, "late anchored", Some(3)),
            note(70_000, "unanchored", None),
        ];
        let (first, second) = split_insights(&insights, 2);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].message, "early anchored");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].message, "late anchored");
        assert_eq!(second[0].segment_index, Some(1));
    }

    #[test]
    fn empty_insights_render_nothing() {
        assert_eq!(render_suggestions_section(&[], &[]), None);
        assert_eq!(
            render_suggestions_section(&[note(1000, "  ", Some(0))], &[]),
            None
        );
    }

    #[test]
    fn renders_suggestion_with_utterance_quote() {
        let segments = vec![
            segment(TranscriptSource::You, "Let me walk you through pricing."),
            segment(TranscriptSource::Others, "We are mostly on spreadsheets right now."),
        ];
        let insights = vec![note(134_000, "Ask what their current process costs", Some(1))];
        let section = render_suggestions_section(&insights, &segments).unwrap();
        assert!(section.starts_with("## Suggestions\n"));
        assert!(section.contains("- **02:14** Ask what their current process costs"));
        assert!(section.contains("> Others: \"We are mostly on spreadsheets right now.\""));
    }

    #[test]
    fn renders_without_quote_when_no_segment_yet() {
        let insights = vec![note(5_000, "Open with the agenda", None)];
        let section = render_suggestions_section(&insights, &[]).unwrap();
        assert!(section.contains("- **00:05** Open with the agenda"));
        assert!(!section.contains('>'));
    }

    #[test]
    fn prefers_speaker_label_and_formats_hours() {
        let mut seg = segment(TranscriptSource::Others, "Budget approval takes a quarter.");
        seg.speaker_label = Some("Dana".to_string());
        let insights = vec![note(3_723_000, "Suggest a pilot instead", Some(0))];
        let section = render_suggestions_section(&insights, &[seg]).unwrap();
        assert!(section.contains("- **1:02:03** Suggest a pilot instead"));
        assert!(section.contains("> Dana: \"Budget approval takes a quarter.\""));
    }

    #[test]
    fn long_utterances_are_truncated_on_word_boundary() {
        let long = "word ".repeat(60);
        let segments = vec![segment(TranscriptSource::Others, &long)];
        let insights = vec![note(60_000, "Summarize their point back", Some(0))];
        let section = render_suggestions_section(&insights, &segments).unwrap();
        let quote_line = section.lines().find(|l| l.contains('>')).unwrap();
        assert!(quote_line.chars().count() < 200);
        assert!(quote_line.contains('…'));
    }

    #[test]
    fn out_of_range_segment_index_is_ignored() {
        let insights = vec![note(1_000, "Note the objection", Some(9))];
        let section = render_suggestions_section(&insights, &[]).unwrap();
        assert!(section.contains("Note the objection"));
        assert!(!section.contains('>'));
    }
}
