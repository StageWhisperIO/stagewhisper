const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.86;
const MIN_TOKEN_LEN: usize = 4;

pub fn correct_transcript(text: &str, vocabulary: &[String]) -> String {
    correct_transcript_with_threshold(text, vocabulary, DEFAULT_SIMILARITY_THRESHOLD)
}

pub fn correct_transcript_with_threshold(
    text: &str,
    vocabulary: &[String],
    threshold: f64,
) -> String {
    if vocabulary.is_empty() || text.is_empty() {
        return text.to_string();
    }

    let candidates: Vec<&String> = vocabulary
        .iter()
        .filter(|v| !v.trim().is_empty())
        .collect();
    if candidates.is_empty() {
        return text.to_string();
    }

    let max_words = candidates
        .iter()
        .map(|c| c.split_whitespace().count())
        .max()
        .unwrap_or(1)
        .max(1);

    let segs = segments(text);
    let word_positions: Vec<usize> = segs
        .iter()
        .enumerate()
        .filter(|(_, seg)| seg.is_word)
        .map(|(i, _)| i)
        .collect();

    let mut result = String::with_capacity(text.len());
    let mut emitted = 0usize;
    let mut wi = 0usize;

    while wi < word_positions.len() {
        let start = word_positions[wi];
        let mut handled = false;

        if max_words >= 2 {
            let max_span = max_words.min(word_positions.len() - wi);
            for span in (2..=max_span).rev() {
                if !span_is_space_joined(&segs, &word_positions, wi, span) {
                    continue;
                }
                let joined = (0..span)
                    .map(|k| segs[word_positions[wi + k]].text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if let Some(outcome) = best_phrase_match(&joined, &candidates, span, threshold) {
                    emit_between(&mut result, &segs, emitted, start);
                    let end = word_positions[wi + span - 1];
                    match outcome {
                        PhraseOutcome::Keep => emit_between(&mut result, &segs, start, end + 1),
                        PhraseOutcome::Replace(replacement) => {
                            result.push_str(&apply_phrase_casing(&joined, &replacement))
                        }
                    }
                    emitted = end + 1;
                    wi += span;
                    handled = true;
                    break;
                }
            }
        }
        if handled {
            continue;
        }

        emit_between(&mut result, &segs, emitted, start);
        let token = &segs[start].text;
        match best_match(token, &candidates, threshold) {
            Some(replacement) => result.push_str(&apply_casing(token, replacement)),
            None => result.push_str(token),
        }
        emitted = start + 1;
        wi += 1;
    }
    emit_between(&mut result, &segs, emitted, segs.len());

    result
}

struct Segment {
    text: String,
    is_word: bool,
}

fn segments(text: &str) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    for ch in text.chars() {
        let is_word = is_word_char(ch);
        match segs.last_mut() {
            Some(last) if last.is_word == is_word => last.text.push(ch),
            _ => segs.push(Segment {
                text: ch.to_string(),
                is_word,
            }),
        }
    }
    segs
}

fn emit_between(result: &mut String, segs: &[Segment], from: usize, to: usize) {
    for seg in &segs[from..to] {
        result.push_str(&seg.text);
    }
}

fn span_is_space_joined(
    segs: &[Segment],
    word_positions: &[usize],
    wi: usize,
    span: usize,
) -> bool {
    for k in 0..span - 1 {
        let gap_start = word_positions[wi + k] + 1;
        let gap_end = word_positions[wi + k + 1];
        for seg in &segs[gap_start..gap_end] {
            if !seg.text.chars().all(|c| c == ' ' || c == '\t') {
                return false;
            }
        }
    }
    true
}

enum PhraseOutcome {
    Keep,
    Replace(String),
}

fn best_phrase_match(
    joined: &str,
    candidates: &[&String],
    word_count: usize,
    threshold: f64,
) -> Option<PhraseOutcome> {
    let lowered = joined.to_lowercase();
    let mut best: Option<(f64, &str)> = None;
    for candidate in candidates {
        if candidate.split_whitespace().count() != word_count {
            continue;
        }
        let lowered_candidate = candidate.to_lowercase();
        if lowered_candidate == lowered {
            return Some(PhraseOutcome::Keep);
        }
        if !length_is_comparable(&lowered, &lowered_candidate) {
            continue;
        }
        let score = jaro_winkler(&lowered, &lowered_candidate);
        if score >= threshold {
            match best {
                Some((best_score, _)) if best_score >= score => {}
                _ => best = Some((score, candidate.as_str())),
            }
        }
    }
    best.map(|(_, candidate)| PhraseOutcome::Replace(candidate.to_string()))
}

fn best_match(token: &str, candidates: &[&String], threshold: f64) -> Option<String> {
    if token.chars().count() < MIN_TOKEN_LEN {
        return None;
    }

    let lowered_token = token.to_lowercase();

    for candidate in candidates {
        if candidate.to_lowercase() == lowered_token {
            return None;
        }
    }

    let mut best: Option<(f64, &str)> = None;
    for candidate in candidates {
        let lowered_candidate = candidate.to_lowercase();
        if !length_is_comparable(&lowered_token, &lowered_candidate) {
            continue;
        }
        let score = jaro_winkler(&lowered_token, &lowered_candidate);
        if score >= threshold {
            match best {
                Some((best_score, _)) if best_score >= score => {}
                _ => best = Some((score, candidate.as_str())),
            }
        }
    }

    best.map(|(_, candidate)| candidate.to_string())
}

fn length_is_comparable(a: &str, b: &str) -> bool {
    let la = a.chars().count() as isize;
    let lb = b.chars().count() as isize;
    (la - lb).abs() <= 2
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '\''
}

fn apply_phrase_casing(original: &str, replacement: &str) -> String {
    let original_words: Vec<&str> = original.split_whitespace().collect();
    let replacement_words: Vec<&str> = replacement.split_whitespace().collect();
    if original_words.len() != replacement_words.len() {
        return apply_casing(original, replacement.to_string());
    }
    original_words
        .iter()
        .zip(replacement_words.iter())
        .map(|(orig, rep)| apply_casing(orig, rep.to_string()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply_casing(original: &str, replacement: String) -> String {
    let mut original_chars = original.chars();
    let first = original_chars.next();
    let all_upper = original.chars().all(|c| !c.is_lowercase());
    let has_letters = original.chars().any(|c| c.is_alphabetic());

    if all_upper && has_letters && original.chars().count() > 1 {
        return replacement.to_uppercase();
    }

    if let Some(first) = first {
        if first.is_uppercase() {
            let mut chars = replacement.chars();
            if let Some(rep_first) = chars.next() {
                let rest: String = chars.collect();
                return format!("{}{}", rep_first.to_uppercase(), rest);
            }
        }
    }

    replacement
}

fn jaro_winkler(a: &str, b: &str) -> f64 {
    let jaro = jaro(a, b);
    if jaro < 0.7 {
        return jaro;
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let max_prefix = 4.min(a_chars.len()).min(b_chars.len());
    let mut prefix = 0usize;
    for i in 0..max_prefix {
        if a_chars[i] == b_chars[i] {
            prefix += 1;
        } else {
            break;
        }
    }

    jaro + (prefix as f64) * 0.1 * (1.0 - jaro)
}

fn jaro(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 && b_len == 0 {
        return 1.0;
    }
    if a_len == 0 || b_len == 0 {
        return 0.0;
    }

    let match_distance = (a_len.max(b_len) / 2).saturating_sub(1);

    let mut a_matches = vec![false; a_len];
    let mut b_matches = vec![false; b_len];
    let mut matches = 0usize;

    for (i, a_ch) in a_chars.iter().enumerate() {
        let start = i.saturating_sub(match_distance);
        let end = (i + match_distance + 1).min(b_len);
        for j in start..end {
            if b_matches[j] || b_chars[j] != *a_ch {
                continue;
            }
            a_matches[i] = true;
            b_matches[j] = true;
            matches += 1;
            break;
        }
    }

    if matches == 0 {
        return 0.0;
    }

    let mut transpositions = 0usize;
    let mut k = 0usize;
    for i in 0..a_len {
        if !a_matches[i] {
            continue;
        }
        while !b_matches[k] {
            k += 1;
        }
        if a_chars[i] != b_chars[k] {
            transpositions += 1;
        }
        k += 1;
    }

    let matches = matches as f64;
    let transpositions = (transpositions / 2) as f64;
    (matches / a_len as f64 + matches / b_len as f64 + (matches - transpositions) / matches) / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vec<String> {
        vec![
            "Kubernetes".to_string(),
            "StageWhisper".to_string(),
            "Parakeet".to_string(),
            "Anthropic".to_string(),
        ]
    }

    #[test]
    fn exact_match_is_never_rewritten() {
        let input = "we deployed Kubernetes today";
        assert_eq!(correct_transcript(input, &vocab()), input);
    }

    #[test]
    fn exact_match_case_insensitive_is_left_alone() {
        let input = "we use kubernetes here";
        assert_eq!(correct_transcript(input, &vocab()), input);
    }

    #[test]
    fn near_miss_is_corrected() {
        let out = correct_transcript("we deployed kubernetis today", &vocab());
        assert_eq!(out, "we deployed Kubernetes today");
    }

    #[test]
    fn unrelated_words_are_not_touched() {
        let input = "the meeting happened in the afternoon";
        assert_eq!(correct_transcript(input, &vocab()), input);
    }

    #[test]
    fn short_tokens_are_not_corrected() {
        let input = "the cat sat on it";
        assert_eq!(correct_transcript(input, &vocab()), input);
    }

    #[test]
    fn casing_of_original_is_preserved_for_leading_capital() {
        let out = correct_transcript("Kubernetis is great", &vocab());
        assert_eq!(out, "Kubernetes is great");
    }

    #[test]
    fn punctuation_and_whitespace_preserved() {
        let out = correct_transcript("Parakeet, anthropik!", &vocab());
        assert_eq!(out, "Parakeet, Anthropic!");
    }

    #[test]
    fn empty_vocab_returns_input() {
        let input = "anything at all";
        assert_eq!(correct_transcript(input, &[]), input);
    }

    #[test]
    fn distant_word_not_overcorrected() {
        let out = correct_transcript("kubernetes documentation database", &vocab());
        assert_eq!(out, "kubernetes documentation database");
    }

    #[test]
    fn whitespace_only_vocab_entries_ignored() {
        let v = vec!["   ".to_string(), "".to_string()];
        let input = "nothing should change here";
        assert_eq!(correct_transcript(input, &v), input);
    }

    #[test]
    fn multi_word_phrase_is_corrected() {
        let v = vec!["New York".to_string()];
        let out = correct_transcript("we flew to new yourk yesterday", &v);
        assert_eq!(out, "we flew to New York yesterday");
    }

    #[test]
    fn multi_word_phrase_exact_is_left_alone() {
        let v = vec!["New York".to_string()];
        let input = "we flew to New York yesterday";
        assert_eq!(correct_transcript(input, &v), input);
    }

    #[test]
    fn multi_word_phrase_preserves_all_caps_casing() {
        let v = vec!["New York".to_string()];
        let out = correct_transcript("we flew to NEW YOURK yesterday", &v);
        assert_eq!(out, "we flew to NEW YORK yesterday");
    }

    #[test]
    fn multi_word_phrase_preserves_leading_capital_casing() {
        let v = vec!["new york".to_string()];
        let out = correct_transcript("we flew to New Yourk yesterday", &v);
        assert_eq!(out, "we flew to New York yesterday");
    }
}
