use std::collections::HashMap;

pub fn normalize_processed_text(
    text: &str,
    custom_dictionary: &[String],
    custom_aliases: &HashMap<String, Vec<String>>,
) -> String {
    let mut out = normalize_sentence_spacing(text);
    for term in custom_dictionary {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let aliases = alias_set_for_term(term, custom_dictionary, custom_aliases);
        for alias in aliases {
            out = replace_standalone_ci(&out, &alias, term);
        }
    }
    normalize_sentence_spacing(&out)
}

pub fn normalize_sentence_spacing(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        out.push(chars[i]);
        if matches!(chars[i], '.' | '!' | '?' | ':' | ';') {
            if let Some(next) = chars.get(i + 1) {
                if next.is_alphanumeric() {
                    out.push(' ');
                }
            }
        }
    }
    out
}

fn alias_set_for_term(
    term: &str,
    dictionary: &[String],
    custom_aliases: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut aliases = Vec::new();

    // Generated aliases from multi-part terms
    let parts: Vec<&str> = term
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() >= 2 {
        aliases.push(parts.join(" "));
        aliases.push(parts.join(""));
    }

    // Configured aliases (case-insensitive key match)
    for (key, values) in custom_aliases {
        if key.eq_ignore_ascii_case(term) {
            for v in values {
                let sanitized = sanitize_alias(v);
                if !sanitized.is_empty() {
                    aliases.push(sanitized);
                }
            }
        }
    }

    // Dedup case-insensitively; skip alias == term or == another dictionary term
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for alias in aliases {
        if alias.eq_ignore_ascii_case(term) {
            continue;
        }
        if dictionary
            .iter()
            .any(|t| t.trim().eq_ignore_ascii_case(&alias) && !t.trim().eq_ignore_ascii_case(term))
        {
            continue;
        }
        let key = alias.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(alias);
        }
    }
    out
}

fn sanitize_alias(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Replace standalone ASCII case-insensitive occurrences of `alias` with `term`.
fn replace_standalone_ci(text: &str, alias: &str, term: &str) -> String {
    if alias.is_empty() {
        return text.to_string();
    }
    let text_bytes = text.as_bytes();
    let alias_bytes = alias.as_bytes();
    let alias_lower: Vec<u8> = alias_bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut out = String::new();
    let mut i = 0;
    while i < text_bytes.len() {
        if i + alias_bytes.len() <= text_bytes.len() {
            let window = &text_bytes[i..i + alias_bytes.len()];
            let matches = window
                .iter()
                .zip(alias_lower.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b);
            if matches {
                let before_ok = if i == 0 {
                    true
                } else {
                    !text_bytes[i - 1].is_ascii_alphanumeric()
                };
                let after_idx = i + alias_bytes.len();
                let after_ok = if after_idx >= text_bytes.len() {
                    true
                } else {
                    !text_bytes[after_idx].is_ascii_alphanumeric()
                };
                if before_ok && after_ok {
                    // Ensure we don't split a UTF-8 char: alias is ASCII-oriented;
                    // only replace when the slice is on char boundaries.
                    if text.is_char_boundary(i) && text.is_char_boundary(after_idx) {
                        out.push_str(term);
                        i = after_idx;
                        continue;
                    }
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_spacing() {
        assert_eq!(
            normalize_sentence_spacing("It is running.We will ship."),
            "It is running. We will ship."
        );
    }

    #[test]
    fn generated_aliases_and_boundaries() {
        let dict = vec!["CI/CD".into(), "pipeline-visualizer".into()];
        let aliases = HashMap::new();
        let text = "We use cicd and the pipeline visualizer daily.";
        let out = normalize_processed_text(text, &dict, &aliases);
        assert!(out.contains("CI/CD"));
        assert!(out.contains("pipeline-visualizer"));
        // Must not replace inside larger tokens
        let text2 = "mycicdtool";
        let out2 = normalize_processed_text(text2, &dict, &aliases);
        assert_eq!(out2, "mycicdtool");
    }
}
