use serde::{Deserialize, Serialize};

use crate::accumulate::{TranscriptSegment, TranscriptSource};

pub const BLOCK_CHAR_TARGET: usize = 5000;
pub const BLOCK_SUMMARY_MAX_CHARS: usize = 1200;
pub const BLOCK_PROMPT_BUDGET_CHARS: usize = 6000;
pub const PLAYBOOK_LENS_MAX_CHARS: usize = 1500;
pub const CARRIED_STATE_MAX_CHARS: usize = 1200;
pub const BLOCK_PROMPT_MAX_TOKENS: usize = 4096;

const SECTION_SEPARATOR: &str = "\n\n";
const MIN_SECTION_CHARS: usize = 16;

const BLOCK_LEAD: &str = "You are condensing one stretch of a longer call that is still in progress. User utterances are prefixed with \"user:\" and counterparty utterances with \"others:\". Write a dense factual digest of this stretch only, as markdown bullets, at most 8 bullets. Capture topics raised, claims made, decisions, numbers, names, and anything left open. Do not write an introduction or a conclusion, do not speculate about what comes next, and do not add a title. This digest will later be combined with the other stretches of the call, so preserve specifics rather than generalising.";

const BLOCK_LENS: &str = "\n\nThis call is being run against the playbook below. Open the digest with a PLAYBOOK STATE section, before any facts: where the conversation stands against the playbook and which parts of it have not been covered yet, naming them plainly. That section is required and always comes first. The factual bullets follow it.\n\nPLAYBOOK:\n";

const BLOCK_CARRY: &str = "\n\nThe playbook state below was carried forward from the earlier stretches of this same call. Treat everything it records as already established: do not repeat its facts, and never report a playbook item it already marks as covered as still outstanding. Restate it with whatever this stretch changes, so your PLAYBOOK STATE section reads as the state of the whole call so far rather than of this stretch alone.\n\nSTATE SO FAR:\n";

const BLOCK_TAIL: &str = "\n\n---\nSTRETCH:\n";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockSummary {
    pub start_index: usize,
    pub end_index: usize,
    pub summary: String,
    pub created_at_ms: u64,
}

pub fn covered_to(blocks: &[BlockSummary]) -> usize {
    blocks
        .iter()
        .map(|block| block.end_index)
        .max()
        .unwrap_or(0)
}

pub fn contiguous_coverage(blocks: &[BlockSummary]) -> (usize, usize) {
    let mut covered = 0usize;
    let mut used = 0usize;
    for block in blocks {
        if block.start_index > covered {
            break;
        }
        covered = covered.max(block.end_index);
        used += 1;
    }
    (used, covered)
}

pub fn next_block_end(segments: &[TranscriptSegment], covered_to: usize) -> Option<usize> {
    if covered_to >= segments.len() {
        return None;
    }
    let mut chars = 0usize;
    for (offset, segment) in segments[covered_to..].iter().enumerate() {
        chars += segment.utterance.chars().count() + 8;
        if chars >= BLOCK_CHAR_TARGET {
            return Some(covered_to + offset + 1);
        }
    }
    None
}

pub fn render_transcript(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .map(|segment| match segment.source {
            TranscriptSource::You => format!("user: {}", segment.utterance),
            TranscriptSource::Others => format!("others: {}", segment.utterance),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn build_block_summary_message(
    segments: &[TranscriptSegment],
    playbook: Option<&str>,
    carried_state: Option<&str>,
    message_budget_chars: usize,
) -> String {
    build_block_summary_message_within(segments, playbook, carried_state, message_budget_chars)
}

pub fn build_relay_block_summary_message(
    segments: &[TranscriptSegment],
    playbook: Option<&str>,
    carried_state: Option<&str>,
) -> String {
    build_block_summary_message_within(
        segments,
        playbook,
        carried_state,
        crate::HOST_TEXT_MAX_CHARS,
    )
}

fn build_block_summary_message_within(
    segments: &[TranscriptSegment],
    playbook: Option<&str>,
    carried_state: Option<&str>,
    message_budget_chars: usize,
) -> String {
    let instruction = block_instruction(playbook, carried_state);
    let budget = message_budget_chars.saturating_sub(instruction.chars().count());
    let (body, _truncated) =
        crate::chunking::chunk_to_char_budget(&render_transcript(segments), budget);
    format!("{instruction}{body}")
}

pub fn carried_playbook_state(blocks: &[BlockSummary]) -> Option<&str> {
    blocks
        .last()
        .map(|block| block.summary.trim())
        .filter(|summary| !summary.is_empty())
}

fn block_instruction(playbook: Option<&str>, carried_state: Option<&str>) -> String {
    let Some(playbook) = playbook.map(str::trim).filter(|text| !text.is_empty()) else {
        return format!("{BLOCK_LEAD}{BLOCK_TAIL}");
    };
    let lens = format!(
        "{BLOCK_LENS}{}",
        clamp_to(playbook, PLAYBOOK_LENS_MAX_CHARS)
    );
    let carry = match carried_state.map(str::trim).filter(|text| !text.is_empty()) {
        Some(state) => format!("{BLOCK_CARRY}{}", clamp_to(state, CARRIED_STATE_MAX_CHARS)),
        None => String::new(),
    };
    format!("{BLOCK_LEAD}{lens}{carry}{BLOCK_TAIL}")
}

pub fn clamp_block_summary(text: &str) -> String {
    clamp_to(text, BLOCK_SUMMARY_MAX_CHARS)
}

pub fn clamp_playbook_lens(text: &str) -> String {
    clamp_to(text, PLAYBOOK_LENS_MAX_CHARS)
}

fn clamp_to(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    trimmed
        .chars()
        .take(limit)
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub fn split_blocks(
    blocks: &[BlockSummary],
    at_index: usize,
) -> (Vec<BlockSummary>, Vec<BlockSummary>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for block in blocks {
        if block.end_index <= at_index {
            before.push(block.clone());
        } else if block.start_index >= at_index {
            after.push(BlockSummary {
                start_index: block.start_index - at_index,
                end_index: block.end_index - at_index,
                summary: block.summary.clone(),
                created_at_ms: block.created_at_ms,
            });
        }
    }
    (before, after)
}

pub fn merge_blocks(
    first: &[BlockSummary],
    second: &[BlockSummary],
    offset: usize,
) -> Vec<BlockSummary> {
    let mut merged = first.to_vec();
    merged.extend(second.iter().map(|block| BlockSummary {
        start_index: block.start_index + offset,
        end_index: block.end_index + offset,
        summary: block.summary.clone(),
        created_at_ms: block.created_at_ms,
    }));
    merged
}

pub fn render_blocks_section(blocks: &[BlockSummary]) -> String {
    render_sections(blocks, None)
}

pub fn render_blocks_section_within(blocks: &[BlockSummary], budget: usize) -> String {
    if blocks.is_empty() {
        return String::new();
    }
    let full = render_sections(blocks, None);
    if full.chars().count() <= budget {
        return full;
    }
    let per_section = section_header(blocks.len()).chars().count()
        + SECTION_SEPARATOR.chars().count()
        + MIN_SECTION_CHARS;
    let capacity = (budget / per_section).max(1);
    let kept = &blocks[blocks.len().saturating_sub(capacity)..];
    let share = budget.saturating_sub(frame_len(kept)) / kept.len();
    clamp_to(&render_sections(kept, Some(share)), budget)
}

fn frame_len(blocks: &[BlockSummary]) -> usize {
    blocks
        .iter()
        .enumerate()
        .map(|(index, _)| section_header(index).chars().count())
        .sum::<usize>()
        + SECTION_SEPARATOR.chars().count() * (blocks.len() - 1)
}

fn section_header(index: usize) -> String {
    format!("SECTION {} SUMMARY:\n", index + 1)
}

fn render_sections(blocks: &[BlockSummary], share: Option<usize>) -> String {
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let summary = match share {
                Some(limit) => clamp_to(&block.summary, limit),
                None => block.summary.trim().to_string(),
            };
            format!("{}{}", section_header(index), summary)
        })
        .collect::<Vec<_>>()
        .join(SECTION_SEPARATOR)
}

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;
