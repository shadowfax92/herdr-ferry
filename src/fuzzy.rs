pub fn score(query: &str, candidate: &str) -> Option<i64> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();
    let query_chars = query.chars().collect::<Vec<_>>();
    let candidate_chars = candidate.chars().collect::<Vec<_>>();
    let mut positions = Vec::with_capacity(query_chars.len());
    let mut cursor = 0;

    for query_char in query_chars {
        let relative = candidate_chars[cursor..]
            .iter()
            .position(|candidate_char| *candidate_char == query_char)?;
        let position = cursor + relative;
        positions.push(position);
        cursor = position + 1;
    }

    let mut total = 100;
    for (index, position) in positions.iter().copied().enumerate() {
        if position == 0
            || candidate_chars
                .get(position.saturating_sub(1))
                .is_some_and(|character| !character.is_alphanumeric())
        {
            total += 24;
        }
        if index > 0 {
            let gap = position.saturating_sub(positions[index - 1] + 1);
            if gap == 0 {
                total += 18;
            } else {
                total -= i64::try_from(gap.min(20)).unwrap_or(20);
            }
        }
    }

    if candidate.contains(&query) {
        total += 80;
    }
    total -= i64::try_from(positions[0].min(40)).unwrap_or(40);
    total -= i64::try_from(candidate_chars.len().saturating_sub(query.len()).min(40)).unwrap_or(40);
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequences_and_rejects_missing_characters() {
        assert!(score("hfy", "Herdr Ferry").is_some());
        assert!(score("xyz", "Herdr Ferry").is_none());
    }

    #[test]
    fn ranks_exact_and_boundary_matches_above_scattered_matches() {
        let exact = score("api", "api server").unwrap();
        let boundary = score("api", "workspace / api").unwrap();
        let scattered = score("api", "alpha pane idle").unwrap();

        assert!(exact > scattered);
        assert!(boundary > scattered);
    }

    #[test]
    fn empty_queries_preserve_input_order_with_equal_scores() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("   ", "anything"), Some(0));
    }
}
