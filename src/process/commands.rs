//! Spoken commands and snippets, applied to the raw transcript before cleanup.
//!
//! The transcript is split into speech (cleaned up as usual, by the LLM or
//! the local rules) and literals that must land exactly as written: line
//! breaks for "new line" / "new paragraph", and the stored text of a snippet
//! whose trigger was spoken. "scratch that" deletes the sentence before it.
//!
//! Built-in commands only count when they stand alone as their own clause
//! ("Thanks. New paragraph. See you" - Whisper punctuates the pause), so
//! "add a new line to the file" is left alone. Snippet triggers are
//! user-chosen phrases and match anywhere.

use crate::config::{trigger_words, Snippet};
use crate::model::Format;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Dictated words, still to be cleaned up.
    Speech(String),
    /// Inserted verbatim.
    Literal(String),
}

enum Command {
    Insert(&'static str),
    Scratch,
}

const BUILTINS: &[(&[&str], Command)] = &[
    (&["new", "paragraph"], Command::Insert("\n\n")),
    (&["new", "line"], Command::Insert("\n")),
    (&["newline"], Command::Insert("\n")),
    (&["scratch", "that"], Command::Scratch),
];

#[derive(Debug, Clone, Default)]
pub struct VoiceCommands {
    builtins: bool,
    /// Trigger words and expansion, longest trigger first so "my work
    /// email" wins over "my work".
    snippets: Vec<(Vec<String>, String)>,
}

impl VoiceCommands {
    pub fn new(builtins: bool, snippets: &[Snippet]) -> Self {
        let mut snippets: Vec<(Vec<String>, String)> = snippets
            .iter()
            .map(|s| (trigger_words(&s.trigger), s.text.clone()))
            .filter(|(words, text)| !words.is_empty() && !text.is_empty())
            .collect();
        snippets.sort_by_key(|(words, _)| std::cmp::Reverse(words.len()));
        Self { builtins, snippets }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    /// Splits `raw` into segments. Returns a single `Speech(raw)` (the exact
    /// input) when nothing matched, so the common case is untouched.
    pub fn split(&self, raw: &str, format: Format) -> Vec<Segment> {
        if !self.builtins && self.snippets.is_empty() {
            return vec![Segment::Speech(raw.to_string())];
        }
        // A pasted line break in a terminal runs the command; never insert one there.
        let line_breaks = format != Format::Shell;

        let tokens: Vec<&str> = raw.split_whitespace().collect();
        let cores: Vec<String> = tokens.iter().map(|t| core(t)).collect();
        let mut out: Vec<Segment> = Vec::new();
        let mut speech: Vec<&str> = Vec::new();
        let mut matched = false;
        let mut i = 0;

        while i < tokens.len() {
            if let Some((len, text)) = self.match_snippet(&cores, i) {
                flush(&mut speech, &mut out);
                // "write to my email." keeps its period; a multi-line block
                // (a signature) stands on its own.
                let trailing: String = if text.contains('\n') {
                    String::new()
                } else {
                    let last = tokens[i + len - 1];
                    let alnum_end = last.rfind(char::is_alphanumeric).map_or(0, |p| {
                        p + last[p..].chars().next().map_or(0, char::len_utf8)
                    });
                    last[alnum_end..]
                        .chars()
                        .filter(|c| matches!(c, '.' | ',' | '!' | '?' | ';' | ':'))
                        .collect()
                };
                out.push(Segment::Literal(format!("{text}{trailing}")));
                matched = true;
                i += len;
                continue;
            }
            if self.builtins {
                if let Some((len, command)) = match_builtin(&tokens, &cores, i) {
                    match command {
                        Command::Insert(text) if line_breaks => {
                            flush(&mut speech, &mut out);
                            out.push(Segment::Literal(text.to_string()));
                            matched = true;
                            i += len;
                            continue;
                        }
                        Command::Insert(_) => {}
                        Command::Scratch => {
                            scratch(&mut speech, &mut out);
                            matched = true;
                            i += len;
                            continue;
                        }
                    }
                }
            }
            speech.push(tokens[i]);
            i += 1;
        }

        if !matched {
            return vec![Segment::Speech(raw.to_string())];
        }
        flush(&mut speech, &mut out);
        out
    }

    fn match_snippet(&self, cores: &[String], at: usize) -> Option<(usize, &str)> {
        self.snippets.iter().find_map(|(words, text)| {
            let end = at + words.len();
            (end <= cores.len() && cores[at..end] == words[..])
                .then_some((words.len(), text.as_str()))
        })
    }
}

/// Joins cleaned-up speech and literals back into one string: a space
/// between pieces, none around line breaks.
pub fn join_segments(pieces: &[(bool, String)]) -> String {
    let mut out = String::new();
    for (is_literal, text) in pieces {
        let text = if *is_literal {
            text.as_str()
        } else {
            text.trim()
        };
        if text.is_empty() {
            continue;
        }
        let breaks_line = |s: &str| s.ends_with('\n') || s.starts_with('\n');
        if !out.is_empty() && !out.ends_with('\n') && !breaks_line(text) {
            out.push(' ');
        }
        out.push_str(text);
    }
    out.trim_matches(' ').to_string()
}

fn core(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn ends_clause(token: &str) -> bool {
    token.ends_with(['.', ',', '!', '?', ';', ':'])
}

fn match_builtin(
    tokens: &[&str],
    cores: &[String],
    at: usize,
) -> Option<(usize, &'static Command)> {
    BUILTINS.iter().find_map(|(words, command)| {
        let end = at + words.len();
        if end > cores.len() || !cores[at..end].iter().zip(words.iter()).all(|(c, w)| c == w) {
            return None;
        }
        // Inner words carrying punctuation ("new, line") aren't the command.
        if tokens[at..end - 1].iter().any(|t| ends_clause(t)) {
            return None;
        }
        let starts = at == 0 || ends_clause(tokens[at - 1]);
        let finishes = end == tokens.len() || ends_clause(tokens[end - 1]);
        (starts && finishes).then_some((words.len(), command))
    })
}

fn flush(speech: &mut Vec<&str>, out: &mut Vec<Segment>) {
    if !speech.is_empty() {
        out.push(Segment::Speech(speech.join(" ")));
        speech.clear();
    }
}

/// Drops the sentence spoken just before "scratch that": back to the last
/// sentence end, or the whole pending speech if there is none. With nothing
/// pending, it undoes the previous inserted piece.
fn scratch(speech: &mut Vec<&str>, out: &mut Vec<Segment>) {
    if speech.is_empty() {
        out.pop();
        return;
    }
    let keep = speech[..speech.len() - 1]
        .iter()
        .rposition(|t| t.ends_with(['.', '!', '?']))
        .map_or(0, |i| i + 1);
    speech.truncate(keep);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(trigger: &str, text: &str) -> Snippet {
        Snippet {
            trigger: trigger.into(),
            text: text.into(),
        }
    }

    fn speech(s: &str) -> Segment {
        Segment::Speech(s.into())
    }

    fn literal(s: &str) -> Segment {
        Segment::Literal(s.into())
    }

    fn split(raw: &str) -> Vec<Segment> {
        VoiceCommands::new(true, &[]).split(raw, Format::CleanProse)
    }

    #[test]
    fn untouched_when_nothing_matches() {
        let raw = "  Add a new line to the file,  then ship it. ";
        assert_eq!(split(raw), vec![speech(raw)]);
    }

    #[test]
    fn standalone_line_commands_insert_breaks() {
        assert_eq!(
            split("Hi Sam, new paragraph. Thanks for the update. New line. Alex"),
            vec![
                speech("Hi Sam,"),
                literal("\n\n"),
                speech("Thanks for the update."),
                literal("\n"),
                speech("Alex"),
            ]
        );
        assert_eq!(split("Newline."), vec![literal("\n")]);
    }

    #[test]
    fn commands_inside_a_sentence_are_words() {
        assert_eq!(
            split("Start a new paragraph here."),
            vec![speech("Start a new paragraph here.")]
        );
        assert_eq!(split("Draw a new line."), vec![speech("Draw a new line.")]);
        assert_eq!(
            split("Don't scratch that surface."),
            vec![speech("Don't scratch that surface.")]
        );
        assert_eq!(split("Okay. New, line."), vec![speech("Okay. New, line.")]);
    }

    #[test]
    fn scratch_that_drops_the_previous_sentence() {
        assert_eq!(
            split("I agree. Let's meet Tuesday. Scratch that. Let's meet Wednesday."),
            vec![speech("I agree. Let's meet Wednesday.")]
        );
        assert_eq!(
            split("Send it to John, scratch that, send it to Mary."),
            vec![speech("send it to Mary.")]
        );
        // nothing pending: undo the last inserted piece
        assert_eq!(
            split("Hello. New line. Scratch that."),
            vec![speech("Hello.")]
        );
        assert_eq!(split("Scratch that."), Vec::<Segment>::new());
    }

    #[test]
    fn shell_never_gets_line_breaks() {
        let vc = VoiceCommands::new(true, &[]);
        assert_eq!(
            vc.split("ls. New line. pwd", Format::Shell),
            vec![speech("ls. New line. pwd")]
        );
        assert_eq!(
            vc.split("rm -rf build, scratch that, ls", Format::Shell),
            vec![speech("ls")]
        );
    }

    #[test]
    fn snippets_expand_anywhere_longest_first() {
        let vc = VoiceCommands::new(
            false,
            &[
                snippet("my link", "https://cal.example/short"),
                snippet("my link long", "https://cal.example/long"),
                snippet("My Email", "sam@example.com"),
            ],
        );
        assert_eq!(
            vc.split(
                "Book time at my link long and write to my email.",
                Format::CleanProse
            ),
            vec![
                speech("Book time at"),
                literal("https://cal.example/long"),
                speech("and write to"),
                literal("sam@example.com."),
            ]
        );
    }

    #[test]
    fn multi_line_snippets_drop_trailing_punctuation() {
        let vc = VoiceCommands::new(false, &[snippet("my signature", "Thanks,\nSam")]);
        assert_eq!(
            vc.split("See you then. My signature.", Format::CleanProse),
            vec![speech("See you then."), literal("Thanks,\nSam")]
        );
    }

    #[test]
    fn disabled_builtins_leave_commands_as_words() {
        let vc = VoiceCommands::new(false, &[]);
        assert_eq!(
            vc.split("Hi. New line. Bye.", Format::CleanProse),
            vec![speech("Hi. New line. Bye.")]
        );
        assert_eq!(
            VoiceCommands::disabled().split("Scratch that.", Format::CleanProse),
            vec![speech("Scratch that.")]
        );
    }

    #[test]
    fn join_puts_spaces_between_words_not_around_breaks() {
        let pieces = vec![
            (false, "Hi Sam,".to_string()),
            (true, "\n\n".to_string()),
            (false, " Thanks. ".to_string()),
            (false, String::new()),
            (true, "sam@example.com".to_string()),
            (true, "\n".to_string()),
            (false, "Alex".to_string()),
        ];
        assert_eq!(
            join_segments(&pieces),
            "Hi Sam,\n\nThanks. sam@example.com\nAlex"
        );
    }
}
