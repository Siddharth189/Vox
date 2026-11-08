//! Spoken repeats, without a language model.
//!
//! People repeat themselves while they think: "so so what I was thinking",
//! "I, I think", "we need to, we need to ship". An LLM tidies that up; the
//! built-in cleanup has to decide from the text alone, so these rules only
//! act where a repeat is almost never meant, and leave the rest alone:
//! "very very good", "no no no", "bye bye", "I know that that works", and
//! phone numbers ("five five five") all survive.
//!
//! Across a full stop the text is more ambiguous. Whisper puts a full stop
//! wherever the speaker paused, so a restart like "go to the [pause] the
//! store" arrives as "go to the. The store". That is merged only for words
//! that can't end a sentence. "so. So" is the one exception: "So" is the
//! usual way to start the next thought, so the stray "so" ending the
//! previous sentence is dropped, unless it really ends it ("I think so.").
//! "before. Before" and the like are left alone: "We've done this before.
//! Before we start..." is ordinary speech.

/// Words that are repeated on purpose often enough that a doubled one is
/// left as spoken.
const DELIBERATE_DOUBLES: &[&str] = &[
    // grammar: "I know that that works", "what it is is", "had had"
    "that", "had", "is", "was", "do", "does", "did", // emphasis
    "very", "really", "much", "many", "more", "far", "too", "super",
    // interjections and set phrases
    "no", "yes", "yeah", "yep", "nope", "ok", "okay", "bye", "ha", "haha", "hey", "hi", "wow", "oh",
    "ah", "aw", "blah", "bla", "knock", "chop", "night", "tut", "la", "na", "go",
    // particles that meet a preposition: "check in in March", "log on on Monday"
    "again", "over", "around", "round", "on", "off", "out", "up", "down", "in", "by", "back",
];

const NUMBER_WORDS: &[&str] = &[
    "zero",
    "oh",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
    "double",
    "triple",
];

/// Words that can't end an English sentence, so "the. The" is a restart.
/// Not "a": "Plan A." is a sentence.
const NEVER_FINAL: &[&str] = &[
    "the", "an", "and", "but", "or", "nor", "because", "than", "whether", "my", "your", "our",
    "their", "its",
];

/// Words before a sentence-final "so" that make it a real ending
/// ("I think so.", "I told you so.", "Even so.", "Is that so?").
const SO_ENDINGS: &[&str] = &[
    "think", "hope", "guess", "believe", "suppose", "expect", "imagine", "fear", "say", "said",
    "says", "told", "you", "me", "him", "her", "them", "us", "even", "not", "do", "did", "does",
    "that", "why", "if", "seem", "seems", "seemed", "appear", "appears", "more", "less", "be",
    "become", "remain", "is", "was", "are", "were",
];

/// Longest repeated phrase looked for ("I think we should, I think we should").
const MAX_PHRASE_WORDS: usize = 4;

/// Lowercased letters and digits of a token: "So," -> "so", "don't" -> "dont".
fn core(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Ends a sentence or clause: a repeat across it isn't a stutter.
fn ends_clause(token: &str) -> bool {
    token.ends_with(['.', '!', '?', ':', ';', '"', ')', '…'])
}

fn is_number(core: &str) -> bool {
    core.chars().any(|c| c.is_ascii_digit()) || NUMBER_WORDS.contains(&core)
}

/// Collapses stutters and repeated phrases, then restarts across a full stop.
pub fn collapse_repeats(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let tokens = collapse_stutters(&tokens);
    merge_restarts(&tokens).join(" ")
}

/// "so so what" -> "so what"; "I, I think" -> "I think"; "I think I think we"
/// -> "I think we". The repeat must be adjacent with at most commas in
/// between; the kept copy carries the capital if the dropped one had it.
fn collapse_stutters(tokens: &[&str]) -> Vec<String> {
    let cores: Vec<String> = tokens.iter().map(|t| core(t)).collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut carry_capital = false;
    let mut i = 0;
    while i < tokens.len() {
        let repeat = (1..=MAX_PHRASE_WORDS).rev().find(|&n| {
            if i + 2 * n > tokens.len() {
                return false;
            }
            let (a, b) = (&cores[i..i + n], &cores[i + n..i + 2 * n]);
            a == b
                && a.iter().all(|c| !c.is_empty() && !is_number(c))
                && !tokens[i..i + n].iter().any(|t| ends_clause(t))
                && (n > 1 || !DELIBERATE_DOUBLES.contains(&a[0].as_str()))
        });
        if let Some(n) = repeat {
            carry_capital |= tokens[i].chars().next().is_some_and(char::is_uppercase);
            i += n; // drop the first copy; the second is checked again
            continue;
        }
        let mut token = tokens[i].to_string();
        if std::mem::take(&mut carry_capital) {
            token = capitalize_first(&token);
        }
        out.push(token);
        i += 1;
    }
    out
}

/// "go to the. The store" -> "go to the store"; "doing it so. So what" ->
/// "doing it. So what" (see the module docs for why only these).
fn merge_restarts(tokens: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        let next = tokens.get(i + 1);
        let word = core(token);
        let full_stop = token.ends_with('.') && !token.ends_with("..");
        let same_next = next.is_some_and(|n| core(n) == word && !word.is_empty());
        if full_stop && same_next && NEVER_FINAL.contains(&word.as_str()) {
            // Keep the first copy without its stop, and whatever trails the
            // second ("the. The," -> "the,").
            let next = next.expect("checked above");
            let trailing: String = next
                .chars()
                .rev()
                .take_while(|c| !c.is_alphanumeric())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            out.push(format!("{}{}", token.trim_end_matches('.'), trailing));
            i += 2;
            continue;
        }
        if full_stop && same_next && word == "so" {
            let before = out.last().map(|t| core(t)).unwrap_or_default();
            let real_ending = before.is_empty() || SO_ENDINGS.contains(&before.as_str());
            if !real_ending {
                // Move the full stop onto the previous word, drop this "so".
                if let Some(prev) = out.last_mut() {
                    let stripped = prev.trim_end_matches([',', ';', ':']).to_string();
                    *prev = format!("{stripped}.");
                }
                i += 1;
                continue;
            }
        }
        out.push(token.clone());
        i += 1;
    }
    out
}

fn capitalize_first(token: &str) -> String {
    let mut chars = token.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => token.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stutters_collapse() {
        assert_eq!(
            collapse_repeats("so so what I was thinking"),
            "so what I was thinking"
        );
        assert_eq!(
            collapse_repeats("I, I think we should"),
            "I think we should"
        );
        assert_eq!(
            collapse_repeats("put it in the the cache"),
            "put it in the cache"
        );
        assert_eq!(collapse_repeats("the the the cache"), "the cache");
        assert_eq!(collapse_repeats("So so we ship"), "So we ship");
    }

    #[test]
    fn repeated_phrases_collapse() {
        assert_eq!(
            collapse_repeats("I think I think we should ship"),
            "I think we should ship"
        );
        assert_eq!(
            collapse_repeats("we need to, we need to ship it today"),
            "we need to ship it today"
        );
    }

    #[test]
    fn deliberate_repeats_stay() {
        for text in [
            "it was very very good",
            "check in in the morning",
            "We go with plan A. A second plan is ready.",
            "no no no, the other one",
            "bye bye",
            "I know that that works",
            "what it is is a cache",
            "call five five five one two one two",
            "the 2 2 split",
            "Yes. Yes.",
            "We ship. We ship today.",
        ] {
            assert_eq!(collapse_repeats(text), text, "{text}");
        }
    }

    #[test]
    fn repeats_across_punctuation_are_not_stutters() {
        // A clause boundary between them means two separate statements.
        assert_eq!(
            collapse_repeats("Ship it; ship it now"),
            "Ship it; ship it now"
        );
        assert_eq!(
            collapse_repeats("He said stop. Stop."),
            "He said stop. Stop."
        );
    }

    #[test]
    fn restarts_on_words_that_cannot_end_a_sentence_merge() {
        assert_eq!(
            collapse_repeats("we go to the. The store is closed."),
            "we go to the store is closed."
        );
        assert_eq!(
            collapse_repeats("it failed because. Because the cache was stale."),
            "it failed because the cache was stale."
        );
    }

    #[test]
    fn stray_so_before_a_new_thought_is_dropped() {
        // Measured: a real dictation, words repeated on purpose.
        assert_eq!(
            collapse_repeats(
                "so that we can review this PR before doing it so. So what I was thinking is"
            ),
            "so that we can review this PR before doing it. So what I was thinking is"
        );
    }

    #[test]
    fn a_real_so_ending_stays() {
        for text in [
            "I think so. So we ship.",
            "I told you so. So there.",
            "Even so. So what?",
            "So. So what now?",
        ] {
            assert_eq!(collapse_repeats(text), text, "{text}");
        }
    }

    #[test]
    fn ambiguous_restarts_are_left_alone() {
        // "We've done this before. Before we start..." is ordinary speech.
        let text = "create an image out of the build before. Before starting, plan it out.";
        assert_eq!(collapse_repeats(text), text);
    }

    #[test]
    fn empty_and_single_words() {
        assert_eq!(collapse_repeats(""), "");
        assert_eq!(collapse_repeats("hello"), "hello");
        assert_eq!(collapse_repeats("so"), "so");
    }
}
