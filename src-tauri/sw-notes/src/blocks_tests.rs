use super::*;

fn segment(text: &str) -> TranscriptSegment {
    TranscriptSegment {
        source: TranscriptSource::Others,
        utterance: text.to_string(),
        speaker_id: None,
        speaker_label: None,
    }
}

fn block(start: usize, end: usize, summary: &str) -> BlockSummary {
    BlockSummary {
        start_index: start,
        end_index: end,
        summary: summary.to_string(),
        created_at_ms: 0,
    }
}

#[test]
fn no_block_until_target_is_reached() {
    let segments = vec![segment(&"a".repeat(100)); 10];
    assert_eq!(next_block_end(&segments, 0), None);
}

#[test]
fn block_closes_on_the_segment_that_crosses_the_target() {
    let segments = vec![segment(&"a".repeat(1000)); 10];
    let end = next_block_end(&segments, 0).expect("block should close");
    assert_eq!(end, 5);
}

#[test]
fn blocks_advance_past_covered_segments() {
    let segments = vec![segment(&"a".repeat(1000)); 20];
    let first = next_block_end(&segments, 0).unwrap();
    let second = next_block_end(&segments, first).unwrap();
    assert_eq!(first, 5);
    assert_eq!(second, 10);
}

#[test]
fn no_block_when_everything_is_covered() {
    let segments = vec![segment(&"a".repeat(1000)); 5];
    assert_eq!(next_block_end(&segments, 5), None);
    assert_eq!(next_block_end(&segments, 9), None);
}

#[test]
fn contiguous_coverage_stops_at_the_first_gap() {
    let blocks = vec![block(0, 5, "a"), block(5, 9, "b"), block(20, 30, "c")];
    assert_eq!(contiguous_coverage(&blocks), (2, 9));
}

#[test]
fn contiguous_coverage_rejects_blocks_that_do_not_start_at_zero() {
    let blocks = vec![block(3, 8, "a"), block(8, 12, "b")];
    assert_eq!(contiguous_coverage(&blocks), (0, 0));
    assert_eq!(covered_to(&blocks), 12);
}

#[test]
fn contiguous_coverage_matches_covered_to_for_a_live_chain() {
    let blocks = vec![block(0, 5, "a"), block(5, 12, "b")];
    assert_eq!(contiguous_coverage(&blocks), (2, 12));
    assert_eq!(covered_to(&blocks), 12);
}

#[test]
fn split_and_merge_gaps_are_visible_to_contiguous_coverage() {
    let (_, after) = split_blocks(&[block(0, 5, "a"), block(10, 15, "c")], 7);
    assert_eq!(contiguous_coverage(&after), (0, 0));

    let merged = merge_blocks(&[block(0, 5, "a")], &[block(0, 4, "b")], 10);
    assert_eq!(contiguous_coverage(&merged), (1, 5));
}

#[test]
fn covered_to_uses_the_furthest_block() {
    let blocks = vec![block(0, 5, "one"), block(5, 12, "two")];
    assert_eq!(covered_to(&blocks), 12);
    assert_eq!(covered_to(&[]), 0);
}

#[test]
fn block_message_carries_the_stretch_and_no_title_rule() {
    let segments = vec![segment("we agreed on friday")];
    let message = build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS);
    assert!(message.contains("others: we agreed on friday"));
    assert!(message.contains("do not add a title"));
}

#[test]
fn one_oversized_utterance_cannot_blow_the_block_budget() {
    let segments = vec![segment(&"x".repeat(50_000))];
    let message = build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS);
    assert!(message.chars().count() <= BLOCK_PROMPT_BUDGET_CHARS);
    assert!(message.contains("transcript truncated"));
}

#[test]
fn a_normal_block_is_never_truncated() {
    let segments = vec![segment(&"we discussed the roadmap and the budget ".repeat(20)); 5];
    let message = build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS);
    assert!(message.chars().count() <= BLOCK_PROMPT_BUDGET_CHARS);
    assert!(!message.contains("transcript truncated"));
}

#[test]
fn a_relay_bound_block_never_exceeds_the_block_budget_under_the_relay_floor() {
    let segments = vec![segment(&"x".repeat(50_000))];
    let message = build_relay_block_summary_message(&segments, None, None);
    assert!(message.chars().count() <= crate::HOST_TEXT_MAX_CHARS);
    assert!(message.contains("transcript truncated"));
}

#[test]
fn a_relay_bound_block_for_a_polish_stretch_never_exceeds_the_block_budget_under_the_relay_floor() {
    let stretch = "Witam serdecznie, chciałbym omówić najważniejsze wyzwania i budżet naszego wspólnego projektu na przyszły kwartał. ".repeat(60);
    let segments = vec![segment(&stretch)];
    let message = build_relay_block_summary_message(&segments, None, None);
    assert!(message.chars().count() <= crate::HOST_TEXT_MAX_CHARS);
}

#[test]
fn each_block_message_is_bounded_by_the_budget_its_own_destination_imposes() {
    let segments = vec![segment(&"x".repeat(50_000))];
    let local = build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS);
    let relay = build_relay_block_summary_message(&segments, None, None);
    assert!(local.chars().count() <= BLOCK_PROMPT_BUDGET_CHARS);
    assert!(relay.chars().count() <= crate::HOST_TEXT_MAX_CHARS);
}

#[test]
fn without_a_playbook_the_block_prompt_carries_no_lens() {
    let segments = vec![segment("we agreed on friday")];
    let message = build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS);
    assert!(message.starts_with("You are condensing one stretch"));
    assert!(!message.contains("PLAYBOOK:"));
    assert!(!message.contains("have not been covered yet"));
    assert!(message.contains("---\nSTRETCH:\nothers: we agreed on friday"));
    assert_eq!(
        message,
        build_block_summary_message(&segments, Some("   \n  "), None, BLOCK_PROMPT_BUDGET_CHARS)
    );
}

#[test]
fn a_playbook_lenses_the_block_prompt() {
    let segments = vec![segment("we agreed on friday")];
    let message = build_block_summary_message(
        &segments,
        Some("Follow CLOSER: clarify, label"),
        None,
        BLOCK_PROMPT_BUDGET_CHARS,
    );
    assert!(message.contains("PLAYBOOK:\nFollow CLOSER: clarify, label"));
    assert!(message.contains("have not been covered yet"));
    assert!(message.contains("---\nSTRETCH:\nothers: we agreed on friday"));
}

#[test]
fn the_playbook_state_is_asked_for_before_the_facts() {
    let segments = vec![segment("we agreed on friday")];
    let message = build_block_summary_message(
        &segments,
        Some("clarify, label"),
        None,
        BLOCK_PROMPT_BUDGET_CHARS,
    );
    assert!(message.contains("PLAYBOOK STATE"));
    assert!(message.contains("always comes first"));
}

#[test]
fn clamping_a_block_keeps_the_playbook_state_it_opens_with() {
    let block = format!(
        "PLAYBOOK STATE\n- clarify: not covered\n- label: covered\n\n{}",
        "- a factual bullet about the call\n".repeat(200)
    );
    let clamped = clamp_block_summary(&block);
    assert_eq!(clamped.chars().count(), BLOCK_SUMMARY_MAX_CHARS);
    assert!(clamped.starts_with("PLAYBOOK STATE\n- clarify: not covered"));
}

#[test]
fn the_playbook_can_never_crowd_out_the_stretch() {
    let segments = vec![segment(&"x".repeat(50_000))];
    let playbook = "playbook line ".repeat(4000);
    let lensed =
        build_block_summary_message(&segments, Some(&playbook), None, BLOCK_PROMPT_BUDGET_CHARS);
    let plain = build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS);
    assert!(lensed.chars().count() <= BLOCK_PROMPT_BUDGET_CHARS);
    let tail_len = plain.len().min(500);
    assert!(lensed.ends_with(&plain[plain.len() - tail_len..]));
}

#[test]
fn a_later_block_is_told_the_state_the_earlier_ones_reached() {
    let segments = vec![segment("we agreed on friday")];
    let message = build_block_summary_message(
        &segments,
        Some("clarify, label"),
        Some("PLAYBOOK STATE\n- clarify: covered"),
        BLOCK_PROMPT_BUDGET_CHARS,
    );
    assert!(message.contains("STATE SO FAR:\nPLAYBOOK STATE\n- clarify: covered"));
    assert!(message.contains("already established"));
    assert!(message.find("PLAYBOOK:").unwrap() < message.find("STATE SO FAR:").unwrap());
}

#[test]
fn the_first_block_of_a_call_carries_no_state() {
    let segments = vec![segment("we agreed on friday")];
    assert_eq!(
        build_block_summary_message(
            &segments,
            Some("clarify, label"),
            None,
            BLOCK_PROMPT_BUDGET_CHARS
        ),
        build_block_summary_message(
            &segments,
            Some("clarify, label"),
            Some("  \n "),
            BLOCK_PROMPT_BUDGET_CHARS
        ),
    );
    assert!(!build_block_summary_message(
        &segments,
        Some("clarify, label"),
        None,
        BLOCK_PROMPT_BUDGET_CHARS
    )
    .contains("STATE SO FAR:"));
}

#[test]
fn carried_state_is_ignored_when_there_is_no_playbook() {
    let segments = vec![segment("we agreed on friday")];
    assert_eq!(
        build_block_summary_message(
            &segments,
            None,
            Some("PLAYBOOK STATE\n- clarify: covered"),
            BLOCK_PROMPT_BUDGET_CHARS
        ),
        build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS),
    );
}

#[test]
fn carried_state_can_never_crowd_out_the_stretch() {
    let segments = vec![segment(&"x".repeat(50_000))];
    let playbook = "playbook line ".repeat(4000);
    let carried = "carried state line ".repeat(4000);
    let message = build_block_summary_message(
        &segments,
        Some(&playbook),
        Some(carried.as_str()),
        BLOCK_PROMPT_BUDGET_CHARS,
    );
    let plain = build_block_summary_message(&segments, None, None, BLOCK_PROMPT_BUDGET_CHARS);
    assert!(message.chars().count() <= BLOCK_PROMPT_BUDGET_CHARS);
    let tail_len = plain.len().min(500);
    assert!(message.ends_with(&plain[plain.len() - tail_len..]));
}

#[test]
fn the_carried_state_is_the_most_recent_block() {
    let blocks = vec![block(0, 5, "first state"), block(5, 9, "latest state")];
    assert_eq!(carried_playbook_state(&blocks), Some("latest state"));
    assert_eq!(carried_playbook_state(&[]), None);
    assert_eq!(carried_playbook_state(&[block(0, 5, "  \n ")]), None);
}

#[test]
fn long_block_summary_is_clamped() {
    let long = "x".repeat(BLOCK_SUMMARY_MAX_CHARS + 500);
    assert_eq!(
        clamp_block_summary(&long).chars().count(),
        BLOCK_SUMMARY_MAX_CHARS
    );
    assert_eq!(clamp_block_summary("  tidy  "), "tidy");
}

#[test]
fn split_keeps_whole_blocks_and_drops_straddlers() {
    let blocks = vec![block(0, 5, "a"), block(5, 10, "b"), block(10, 15, "c")];
    let (before, after) = split_blocks(&blocks, 10);
    assert_eq!(before, vec![block(0, 5, "a"), block(5, 10, "b")]);
    assert_eq!(after, vec![block(0, 5, "c")]);

    let (before, after) = split_blocks(&blocks, 7);
    assert_eq!(before, vec![block(0, 5, "a")]);
    assert_eq!(after, vec![block(3, 8, "c")]);
}

#[test]
fn merge_rebases_the_second_half() {
    let first = vec![block(0, 5, "a")];
    let second = vec![block(0, 4, "b")];
    assert_eq!(
        merge_blocks(&first, &second, 5),
        vec![block(0, 5, "a"), block(5, 9, "b")]
    );
}

#[test]
fn sections_that_fit_are_rendered_verbatim() {
    let blocks = vec![block(0, 5, "first things"), block(5, 9, "later things")];
    let full = render_blocks_section(&blocks);
    assert_eq!(
        render_blocks_section_within(&blocks, full.chars().count()),
        full
    );
}

#[test]
fn every_section_survives_a_tight_budget() {
    let blocks: Vec<BlockSummary> = (0..40)
        .map(|i| {
            block(
                i * 10,
                (i + 1) * 10,
                &format!("- section {i} {}", "detail ".repeat(60)),
            )
        })
        .collect();
    let budget = 4000;
    let rendered = render_blocks_section_within(&blocks, budget);
    assert!(rendered.chars().count() <= budget);
    for index in 0..40 {
        assert!(rendered.contains(&format!("SECTION {} SUMMARY:", index + 1)));
        assert!(rendered.contains(&format!("- section {index} ")));
    }
}

#[test]
fn the_budget_holds_when_the_headers_alone_do_not_fit() {
    let blocks: Vec<BlockSummary> = (0..500)
        .map(|i| block(i * 10, (i + 1) * 10, "- something happened"))
        .collect();
    for budget in [0, 10, 400, 4000] {
        assert!(
            render_blocks_section_within(&blocks, budget)
                .chars()
                .count()
                <= budget
        );
    }
}

#[test]
fn a_call_too_long_to_frame_keeps_the_most_recent_sections() {
    let blocks: Vec<BlockSummary> = (0..500)
        .map(|i| block(i * 10, (i + 1) * 10, &format!("- stretch {i} happened")))
        .collect();
    let rendered = render_blocks_section_within(&blocks, 4000);
    assert!(rendered.contains("- stretch 499"));
    assert!(!rendered.contains("- stretch 100"));
}

#[test]
fn blocks_section_is_numbered_in_order() {
    let blocks = vec![block(0, 5, "first things"), block(5, 9, "later things")];
    let rendered = render_blocks_section(&blocks);
    assert!(rendered.starts_with("SECTION 1 SUMMARY:\nfirst things"));
    assert!(rendered.contains("SECTION 2 SUMMARY:\nlater things"));
}
