use super::chunk_to_char_budget;

const MARKER_HINT: &str = "truncated";

fn polish_call_transcript(repeats: usize) -> String {
    let line = "user: Wydaje mi się, że powinniśmy przesunąć rozmowę o przedłużeniu na następny kwartał i najpierw skupić się na brakach we wdrożeniu użytkowników.";
    std::iter::repeat_n(line, repeats)
        .collect::<Vec<_>>()
        .join("\n")
}

fn numbered_utterances(count: usize) -> String {
    (0..count)
        .map(|index| format!("user: utterance number {index} carrying its own distinct content"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn words_excluding_marker(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !line.contains(MARKER_HINT))
        .flat_map(str::split_whitespace)
}

#[test]
fn text_within_budget_is_returned_unchanged() {
    let text = numbered_utterances(4);
    let (out, truncated) = chunk_to_char_budget(&text, 10_000);
    assert_eq!(out, text);
    assert!(!truncated);
}

#[test]
fn the_result_never_exceeds_the_requested_character_budget() {
    let text = numbered_utterances(400);
    for budget in [64usize, 200, 900, 3000] {
        let (out, truncated) = chunk_to_char_budget(&text, budget);
        assert!(
            out.chars().count() <= budget,
            "budget {budget} produced {} chars",
            out.chars().count()
        );
        assert!(truncated);
    }
}

#[test]
fn a_zero_budget_never_panics_and_yields_nothing() {
    let (out, truncated) = chunk_to_char_budget(&numbered_utterances(20), 0);
    assert!(out.is_empty());
    assert!(truncated);
}

#[test]
fn a_budget_too_small_for_even_the_omission_marker_still_fits() {
    let text = numbered_utterances(50);
    let (out, truncated) = chunk_to_char_budget(&text, 12);
    assert!(out.chars().count() <= 12);
    assert!(truncated);
}

#[test]
fn truncation_keeps_both_the_earliest_and_latest_utterances() {
    let text = numbered_utterances(300);
    let (out, _) = chunk_to_char_budget(&text, 1200);
    assert!(out.contains("utterance number 0"));
    assert!(out.contains("utterance number 299"));
}

#[test]
fn a_truncated_middle_is_explicit_never_silent() {
    let (out, truncated) = chunk_to_char_budget(&numbered_utterances(300), 1200);
    assert!(truncated);
    assert!(out.contains(MARKER_HINT));
}

#[test]
fn character_budgeting_never_cuts_a_word_in_half() {
    let text = numbered_utterances(300);
    let (out, _) = chunk_to_char_budget(&text, 1500);
    let source_words: std::collections::HashSet<&str> = text.split_whitespace().collect();
    for word in words_excluding_marker(&out) {
        assert!(source_words.contains(word), "{word} is not a source word");
    }
}

#[test]
fn a_single_unbroken_run_of_characters_still_respects_the_budget() {
    let text = "x".repeat(20_000);
    let (out, truncated) = chunk_to_char_budget(&text, 500);
    assert!(out.chars().count() <= 500);
    assert!(truncated);
}

#[test]
fn a_real_polish_call_transcript_never_exceeds_its_requested_budget() {
    let text = polish_call_transcript(200);
    for budget in [300usize, 1200, 4000] {
        let (out, _) = chunk_to_char_budget(&text, budget);
        assert!(
            out.chars().count() <= budget,
            "budget {budget} produced {} chars",
            out.chars().count()
        );
    }
}

#[test]
fn a_polish_transcript_that_fits_the_budget_is_returned_in_full() {
    let text = polish_call_transcript(2);
    let (out, truncated) = chunk_to_char_budget(&text, 10_000);
    assert_eq!(out, text);
    assert!(!truncated);
}

#[test]
fn multibyte_text_is_counted_in_characters_not_bytes() {
    let text = polish_call_transcript(40);
    let (out, _) = chunk_to_char_budget(&text, 900);
    assert!(out.chars().count() <= 900);
    assert!(out.len() > out.chars().count());
}

#[test]
fn a_script_with_no_ascii_at_all_still_respects_the_budget() {
    let text = "日本語のテキストがここにあります。".repeat(500);
    let (out, truncated) = chunk_to_char_budget(&text, 400);
    assert!(out.chars().count() <= 400);
    assert!(truncated);
}

#[test]
fn the_budget_is_a_hard_ceiling_across_a_wide_sweep_of_sizes() {
    let text = polish_call_transcript(120);
    for budget in (0..2000).step_by(37) {
        let (out, _) = chunk_to_char_budget(&text, budget);
        assert!(
            out.chars().count() <= budget,
            "budget {budget} produced {} chars",
            out.chars().count()
        );
    }
}
