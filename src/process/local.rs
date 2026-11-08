//! Rule-based cleanup that needs no model and adds no measurable latency.
//!
//! Whisper already punctuates and capitalizes; what's left is removing
//! disfluencies, tidying spacing, applying the custom dictionary, and
//! respecting the target app's format (no forced capitals or trailing period
//! in a terminal). This is the default path whenever Ollama isn't running,
//! so Vox is useful the moment the speech model is downloaded.

use std::collections::HashMap;

use crate::model::{EnrichedRequest, Format, ProcessedText};
use crate::process::dictionary::normalize_processed_text;
use crate::process::repeats::collapse_repeats;
use crate::process::TextProcessor;

/// Pure hesitation sounds. Deliberately excludes words that are also real
/// words ("like", "so", "well", "right", "ah", "hmm") - removing those
/// without understanding the sentence does more harm than good.
const FILLERS: &[&str] = &[
    "um", "umm", "ummm", "uh", "uhh", "uhhh", "uhm", "erm", "er", "eh",
];

pub struct LocalCleanup {
    custom_dictionary: Vec<String>,
    custom_aliases: HashMap<String, Vec<String>>,
}

impl LocalCleanup {
    pub fn new(
        custom_dictionary: Vec<String>,
        custom_aliases: HashMap<String, Vec<String>>,
    ) -> Self {
        Self {
            custom_dictionary,
            custom_aliases,
        }
    }

    pub fn clean(&self, raw: &str, format: Format) -> String {
        clean_text(raw, format, &self.custom_dictionary, &self.custom_aliases)
    }
}

impl TextProcessor for LocalCleanup {
    fn process(&self, req: &EnrichedRequest) -> crate::error::Result<ProcessedText> {
        Ok(ProcessedText::new(
            self.clean(&req.transcript.text, req.ctx.profile.format),
        ))
    }

    fn name(&self) -> &'static str {
        "local"
    }
}

pub fn clean_text(
    raw: &str,
    format: Format,
    dictionary: &[String],
    aliases: &HashMap<String, Vec<String>>,
) -> String {
    let without_fillers = remove_fillers(raw);
    // Shell commands repeat words on purpose ("echo echo", "git add add.js").
    let deduped = if format == Format::Shell {
        without_fillers
    } else {
        collapse_repeats(&without_fillers)
    };
    let spaced = tidy_spacing(&deduped);
    let mut text = normalize_processed_text(&spaced, dictionary, aliases);

    match format {
        Format::Shell => {
            text = strip_trailing_period(&text);
            text = lowercase_first(&text);
        }
        Format::CodeContext => {
            // Short identifier-ish phrases ("user id", "handle request")
            // shouldn't grow a capital and a period; real sentences keep them.
            if text.split_whitespace().count() <= 3 {
                text = strip_trailing_period(&text);
            }
        }
        Format::Casual | Format::CleanProse | Format::ProfessionalEmail | Format::Markdown => {
            text = capitalize_sentences(&text);
        }
    }
    text.trim().to_string()
}

fn is_filler(word: &str) -> bool {
    let core: String = word
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    // Only when the token is nothing but the filler plus punctuation
    // ("um", "Um,", "uh...") - never inside "umbrella" or "under".
    !core.is_empty()
        && FILLERS.contains(&core.as_str())
        && word.chars().filter(|c| c.is_alphanumeric()).count() == core.chars().count()
}

fn remove_fillers(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut sentence_start = 0usize;
    for word in text.split_whitespace() {
        if !is_filler(word) {
            out.push(word.to_string());
            if word.ends_with(['.', '?', '!']) {
                sentence_start = out.len();
            }
            continue;
        }
        // "we should, uh, ship it" -> "we should ship it", but keep the
        // comma after a sentence-opening interjection: "So, um, I" -> "So, I".
        if word.ends_with(',') && out.len() > sentence_start + 1 {
            if let Some(prev) = out.last_mut() {
                if prev.ends_with(',') {
                    prev.pop();
                }
            }
        }
        // Keep a sentence-ending mark the filler was carrying ("... it, um.")
        let terminal: String = word
            .chars()
            .rev()
            .take_while(|c| matches!(c, '.' | '?' | '!'))
            .collect();
        if !terminal.is_empty() {
            if let Some(prev) = out.last_mut() {
                let trimmed = prev.trim_end_matches([',', ';', ':']).to_string();
                if !trimmed.ends_with(['.', '?', '!']) {
                    *prev = trimmed + &terminal.chars().rev().collect::<String>();
                }
            }
        }
    }
    let joined = out.join(" ");
    // "So, um, we ship" -> "So, we ship"; "Um, so" -> "so" (capitalized later)
    joined
        .replace(",,", ",")
        .trim_start_matches([',', ' '])
        .to_string()
}

/// Collapses whitespace and removes spaces before punctuation.
fn tidy_spacing(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    for c in collapsed.chars() {
        if matches!(c, ',' | '.' | '!' | '?' | ';' | ':') && out.ends_with(' ') {
            out.pop();
        }
        out.push(c);
    }
    out
}

/// Capitalizes the first letter of the text and of each sentence. A sentence
/// starts after `.`, `!`, or `?` *followed by whitespace*, so "node.js",
/// "3.5", and "example.com" are left alone.
fn capitalize_sentences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut capitalize_next = true;
    let mut after_mark = false;
    for c in text.chars() {
        if after_mark {
            after_mark = false;
            if c.is_whitespace() {
                capitalize_next = true;
            }
        }
        if capitalize_next && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            capitalize_next = false;
            continue;
        }
        if c.is_alphanumeric() {
            capitalize_next = false;
        }
        out.push(c);
        if matches!(c, '.' | '!' | '?') {
            after_mark = true;
        }
    }
    out
}

fn strip_trailing_period(text: &str) -> String {
    let t = text.trim_end();
    if t.ends_with("...") {
        return t.to_string();
    }
    t.strip_suffix('.').unwrap_or(t).to_string()
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        // Leave acronyms/paths alone ("SSH", "/usr"): only lowercase a
        // capitalized ordinary word.
        Some(first)
            if first.is_uppercase() && chars.clone().next().is_some_and(|c| c.is_lowercase()) =>
        {
            first.to_lowercase().chain(chars).collect()
        }
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(raw: &str, format: Format) -> String {
        clean_text(raw, format, &[], &HashMap::new())
    }

    #[test]
    fn removes_hesitations_but_not_real_words() {
        assert_eq!(
            clean(
                "So, um, I think we should, uh, ship it.",
                Format::CleanProse
            ),
            "So, I think we should ship it."
        );
        assert_eq!(
            clean("Um, can you review this?", Format::CleanProse),
            "Can you review this?"
        );
        assert_eq!(
            clean("I like the umbrella under the bridge.", Format::CleanProse),
            "I like the umbrella under the bridge."
        );
        assert_eq!(
            clean("We ship Friday, um.", Format::CleanProse),
            "We ship Friday."
        );
    }

    #[test]
    fn fixes_spacing_and_capitalization() {
        assert_eq!(
            clean(
                "  hello there , how are you ?  fine.Thanks",
                Format::CleanProse
            ),
            "Hello there, how are you? Fine. Thanks"
        );
    }

    #[test]
    fn keeps_numbers_and_code_like_tokens() {
        assert_eq!(
            clean(
                "version 2.5 runs on localhost:8080 with node.js.",
                Format::CleanProse
            ),
            "Version 2.5 runs on localhost:8080 with node.js."
        );
    }

    #[test]
    fn shell_profile_drops_period_and_capital() {
        assert_eq!(clean("Git status.", Format::Shell), "git status");
        assert_eq!(
            clean("SSH into the box.", Format::Shell),
            "SSH into the box"
        );
    }

    #[test]
    fn code_profile_leaves_short_identifiers_alone() {
        assert_eq!(clean("user id.", Format::CodeContext), "user id");
        assert_eq!(
            clean(
                "This function retries the request three times.",
                Format::CodeContext
            ),
            "This function retries the request three times."
        );
    }

    #[test]
    fn applies_dictionary_aliases() {
        let dict = vec!["Kubernetes".to_string()];
        let mut aliases = HashMap::new();
        aliases.insert("Kubernetes".to_string(), vec!["coopernetes".to_string()]);
        assert_eq!(
            clean_text(
                "deploy to coopernetes today",
                Format::CleanProse,
                &dict,
                &aliases
            ),
            "Deploy to Kubernetes today"
        );
    }

    #[test]
    fn spoken_repeats_are_tidied() {
        // A real dictation (the words were repeated on purpose).
        assert_eq!(
            clean(
                "so that we can you know this PR before doing it so. So what I was thinking is to, um, to create a image out of the build",
                Format::CleanProse
            ),
            "So that we can you know this PR before doing it. So what I was thinking is to create a image out of the build"
        );
        assert_eq!(
            clean("so so, the the cache is stale", Format::CleanProse),
            "So, the cache is stale"
        );
        assert_eq!(clean("echo echo hello", Format::Shell), "echo echo hello");
    }

    #[test]
    fn empty_and_filler_only_input() {
        assert_eq!(clean("", Format::CleanProse), "");
        assert_eq!(clean("Um. Uh.", Format::CleanProse), "");
    }
}
