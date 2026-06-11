use crate::accumulate::{TranscriptSegment, TranscriptSource};

pub const RELAY_TEXT_BUDGET: usize = 7500;

const SUMMARY_INSTRUCTION: &str = "You are this user's own AI assistant. Below is the full transcript of a call the user just finished. User utterances are prefixed with \"user:\" and couterparty (could be multiple people) utterances are prefixed with \"others:\". Write a concise, well-structured markdown summary of the call: a short overview, the key points and decisions, and any open questions. End with a \"## Action Items\" section as a markdown checklist, grounded ONLY in what you (this assistant) can actually do given your own tools, integrations, and memory. Do not invent capabilities you do not have. Reply with plain markdown only: no JSON and no surrounding code fences.\n\n---\nTRANSCRIPT:\n";

pub fn build_summary_message(segments: &[TranscriptSegment]) -> String {
    let transcript = render_transcript_for_prompt(segments);
    let budget = RELAY_TEXT_BUDGET.saturating_sub(SUMMARY_INSTRUCTION.chars().count());
    let (body, _truncated) = truncate_for_relay(&transcript, budget);
    format!("{SUMMARY_INSTRUCTION}{body}")
}

pub fn truncate_for_relay(text: &str, budget: usize) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= budget {
        return (text.to_string(), false);
    }
    let omitted = chars.len() - budget;
    let marker = format!("\n…[transcript truncated, {omitted} chars omitted]…\n");
    let marker_len = marker.chars().count();
    let usable = budget.saturating_sub(marker_len);
    let head_len = usable * 6 / 10;
    let tail_len = usable - head_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    (format!("{head}{marker}{tail}"), true)
}

fn render_transcript_for_prompt(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .map(|s| match &s.source {
            TranscriptSource::You => format!("user: {}", s.utterance),
            TranscriptSource::Others => format!("others: {}", s.utterance),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use crate::accumulate::TranscriptSource;

    use super::*;

    #[test]
    fn short_transcript_is_not_truncated() {
        let (out, truncated) = truncate_for_relay("a short transcript", 7500);
        assert!(!truncated);
        assert_eq!(out, "a short transcript");
    }

    #[test]
    fn long_transcript_keeps_head_and_tail_under_budget() {
        let text: String = "x".repeat(20_000);
        let budget = 6900;
        let (out, truncated) = truncate_for_relay(&text, budget);
        assert!(truncated);
        assert!(out.chars().count() <= budget);
        assert!(out.contains("transcript truncated"));
    }

    #[test]
    fn summary_message_is_within_relay_cap_and_has_action_items() {
        let text: String = "word ".repeat(5000);
        let segments = vec![TranscriptSegment {
            source: TranscriptSource::You,
            utterance: text,
        }];

        let msg = build_summary_message(&segments);
        assert!(msg.chars().count() <= RELAY_TEXT_BUDGET);
        assert!(msg.contains("## Action Items"));
        assert!(msg.contains("TRANSCRIPT:"));
    }
}
