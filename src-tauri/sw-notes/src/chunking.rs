use text_splitter::TextSplitter;

const TAIL_SHARE_PERCENT: usize = 40;
const GRANULARITY_CHARS: usize = 160;

pub fn chunk_to_char_budget(text: &str, budget_chars: usize) -> (String, bool) {
    let total_chars = text.chars().count();
    if total_chars <= budget_chars {
        return (text.to_string(), false);
    }
    if budget_chars == 0 {
        return (String::new(), true);
    }

    let marker_reserve = omission_marker(total_chars).chars().count();
    if marker_reserve >= budget_chars {
        return (leading_slice(text, budget_chars), true);
    }

    let usable = budget_chars - marker_reserve;
    let head_budget = usable * (100 - TAIL_SHARE_PERCENT) / 100;
    let tail_budget = usable - head_budget;

    let head_text = leading_slice(text, head_budget);
    let tail_text = trailing_slice(&text[head_text.len()..], tail_budget);

    let kept_chars = head_text.chars().count() + tail_text.chars().count();
    let omitted_chars = total_chars.saturating_sub(kept_chars);
    let result = format!("{head_text}{}{tail_text}", omission_marker(omitted_chars));
    if result.chars().count() > budget_chars {
        return (leading_slice(text, budget_chars), true);
    }
    (result, true)
}

fn granular_chunk_offsets(text: &str, budget_chars: usize) -> Vec<(usize, &str)> {
    let capacity = budget_chars.clamp(1, GRANULARITY_CHARS);
    TextSplitter::new(capacity).chunk_indices(text).collect()
}

fn leading_slice(text: &str, budget_chars: usize) -> String {
    if budget_chars == 0 || text.is_empty() {
        return String::new();
    }
    let mut kept_end = 0usize;
    for (offset, chunk) in granular_chunk_offsets(text, budget_chars) {
        let candidate_end = offset + chunk.len();
        if text[..candidate_end].chars().count() > budget_chars {
            break;
        }
        kept_end = candidate_end;
    }
    if kept_end == 0 {
        return hard_leading_slice(text, budget_chars);
    }
    text[..kept_end].to_string()
}

fn trailing_slice(text: &str, budget_chars: usize) -> String {
    if budget_chars == 0 || text.is_empty() {
        return String::new();
    }
    let chunks = granular_chunk_offsets(text, budget_chars);
    let mut kept_start = text.len();
    for (offset, _) in chunks.iter().rev() {
        if text[*offset..].chars().count() > budget_chars {
            break;
        }
        kept_start = *offset;
    }
    if kept_start == text.len() {
        return hard_trailing_slice(text, budget_chars);
    }
    text[kept_start..].to_string()
}

fn hard_leading_slice(text: &str, budget_chars: usize) -> String {
    match text.char_indices().nth(budget_chars) {
        Some((byte_idx, _)) => text[..byte_idx].to_string(),
        None => text.to_string(),
    }
}

fn hard_trailing_slice(text: &str, budget_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= budget_chars {
        return text.to_string();
    }
    let skip = total_chars - budget_chars;
    match text.char_indices().nth(skip) {
        Some((byte_idx, _)) => text[byte_idx..].to_string(),
        None => text.to_string(),
    }
}

fn omission_marker(omitted_chars: usize) -> String {
    format!("\n…[transcript truncated, {omitted_chars} characters omitted]…\n")
}

#[cfg(test)]
#[path = "chunking_tests.rs"]
mod tests;
