mod local_whisper;
pub mod streaming;

pub use local_whisper::LocalWhisper;
pub use streaming::{SegmentHint, StreamOutcome, StreamingSession};

use crate::error::Result;
use crate::model::{AppContext, AudioChunk, Transcript};

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, audio: &AudioChunk, ctx: &AppContext) -> Result<Transcript>;

    /// One segment of a dictation streamed while the user speaks, with what
    /// came before it. Transcribers that can't use the hint ignore it.
    fn transcribe_segment(
        &self,
        audio: &AudioChunk,
        ctx: &AppContext,
        hint: &SegmentHint,
    ) -> Result<Transcript> {
        let _ = hint;
        self.transcribe(audio, ctx)
    }

    fn name(&self) -> &'static str;
}

pub struct FakeTranscriber {
    pub text: String,
}

impl FakeTranscriber {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe(&self, _audio: &AudioChunk, _ctx: &AppContext) -> Result<Transcript> {
        Ok(Transcript::new(self.text.clone()))
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}

/// Empty, or fully wrapped in `[...]` / `(...)` - whisper non-speech tokens.
pub fn is_non_speech(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    let bytes = t.as_bytes();
    (bytes.first() == Some(&b'[') && bytes.last() == Some(&b']'))
        || (bytes.first() == Some(&b'(') && bytes.last() == Some(&b')'))
}

/// Writing systems, for telling whether a transcript is in a script none of
/// the user's languages use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Devanagari,
    Arabic,
    Cyrillic,
    Greek,
    Hebrew,
    Bengali,
    Gurmukhi,
    Gujarati,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
    Thai,
    Hangul,
    Kana,
    Han,
    Other,
}

impl Script {
    fn name(self) -> &'static str {
        match self {
            Script::Latin => "Latin",
            Script::Devanagari => "Devanagari",
            Script::Arabic => "Arabic",
            Script::Cyrillic => "Cyrillic",
            Script::Greek => "Greek",
            Script::Hebrew => "Hebrew",
            Script::Bengali => "Bengali",
            Script::Gurmukhi => "Gurmukhi",
            Script::Gujarati => "Gujarati",
            Script::Tamil => "Tamil",
            Script::Telugu => "Telugu",
            Script::Kannada => "Kannada",
            Script::Malayalam => "Malayalam",
            Script::Thai => "Thai",
            Script::Hangul => "Korean",
            Script::Kana => "Japanese",
            Script::Han => "Chinese",
            Script::Other => "unfamiliar",
        }
    }
}

fn script_of(c: char) -> Option<Script> {
    if !c.is_alphabetic() {
        return None;
    }
    let s = match c as u32 {
        0x0041..=0x024F | 0x1E00..=0x1EFF => Script::Latin,
        0x0370..=0x03FF => Script::Greek,
        0x0400..=0x052F => Script::Cyrillic,
        0x0590..=0x05FF => Script::Hebrew,
        0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
        0x0900..=0x097F => Script::Devanagari,
        0x0980..=0x09FF => Script::Bengali,
        0x0A00..=0x0A7F => Script::Gurmukhi,
        0x0A80..=0x0AFF => Script::Gujarati,
        0x0B80..=0x0BFF => Script::Tamil,
        0x0C00..=0x0C7F => Script::Telugu,
        0x0C80..=0x0CFF => Script::Kannada,
        0x0D00..=0x0D7F => Script::Malayalam,
        0x0E00..=0x0E7F => Script::Thai,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Script::Hangul,
        0x3040..=0x30FF => Script::Kana,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF => Script::Han,
        _ => Script::Other,
    };
    Some(s)
}

/// Scripts a language is normally written in, besides Latin (which every
/// language gets: names, technical terms, romanized speech). `None` for a
/// language this table doesn't know, so it is never second-guessed.
fn native_scripts(code: &str) -> Option<&'static [Script]> {
    let scripts: &'static [Script] = match code {
        "en" | "es" | "fr" | "de" | "pt" | "it" | "nl" | "pl" | "tr" | "sv" | "da" | "no"
        | "nn" | "fi" | "cs" | "sk" | "ro" | "hu" | "hr" | "sl" | "id" | "ms" | "vi" | "tl"
        | "sw" | "ca" | "et" | "lv" | "lt" | "af" | "eu" | "gl" | "cy" | "is" | "sq" => &[],
        "hi" | "mr" | "ne" | "sa" => &[Script::Devanagari],
        "ar" | "fa" | "ur" | "ps" | "sd" => &[Script::Arabic],
        "ru" | "uk" | "bg" | "sr" | "be" | "kk" | "mk" | "mn" | "tg" | "ba" | "tt" => {
            &[Script::Cyrillic]
        }
        "el" => &[Script::Greek],
        "he" | "yi" => &[Script::Hebrew],
        "bn" | "as" => &[Script::Bengali],
        "pa" => &[Script::Gurmukhi],
        "gu" => &[Script::Gujarati],
        "ta" => &[Script::Tamil],
        "te" => &[Script::Telugu],
        "kn" => &[Script::Kannada],
        "ml" => &[Script::Malayalam],
        "th" => &[Script::Thai],
        "ko" => &[Script::Hangul],
        "ja" => &[Script::Kana, Script::Han],
        "zh" | "yue" => &[Script::Han],
        _ => return None,
    };
    Some(scripts)
}

/// When most of `text` is in a script none of `allowed` (whisper language
/// codes) is written in, the name of that script: Whisper decoded the
/// audio as some other language, and the text is useless to paste.
pub fn script_mismatch(text: &str, allowed: &[&str]) -> Option<&'static str> {
    if allowed.is_empty() {
        return None;
    }
    let mut ok: Vec<Script> = vec![Script::Latin];
    for code in allowed {
        ok.extend_from_slice(native_scripts(code)?);
    }
    let mut letters = 0usize;
    let mut foreign: Vec<(Script, usize)> = Vec::new();
    for script in text.chars().filter_map(script_of) {
        letters += 1;
        if !ok.contains(&script) {
            match foreign.iter_mut().find(|(s, _)| *s == script) {
                Some((_, n)) => *n += 1,
                None => foreign.push((script, 1)),
            }
        }
    }
    let foreign_total: usize = foreign.iter().map(|(_, n)| n).sum();
    if letters == 0 || foreign_total * 2 <= letters {
        return None;
    }
    foreign
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(s, _)| s.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_guard_rejects_only_foreign_scripts() {
        let en_hi = ["en", "hi"];
        // The Arabic Vox once pasted for a Hindi/English speaker.
        assert_eq!(
            script_mismatch("هناك أمام منذ ميناتك من أسطارت", &en_hi),
            Some("Arabic")
        );
        assert_eq!(script_mismatch("Pull request is open", &en_hi), None);
        assert_eq!(script_mismatch("kal deployment ho gaya", &en_hi), None);
        assert_eq!(script_mismatch("डिप्लॉयमेंट पूरा हो गया है", &en_hi), None);
        assert_eq!(
            script_mismatch("डिप्लॉयमेंट पूरा हो गया है", &["en"]),
            Some("Devanagari")
        );
        // A few foreign letters in mostly-Latin text are fine.
        assert_eq!(
            script_mismatch("Meeting with Сергей about the release", &["en"]),
            None
        );
        // No letters, unknown languages, or no constraint: never rejected.
        assert_eq!(script_mismatch("123 456", &["en"]), None);
        assert_eq!(script_mismatch("هناك أمام", &["xx"]), None);
        assert_eq!(script_mismatch("هناك أمام", &[]), None);
    }

    #[test]
    fn non_speech_detection() {
        assert!(is_non_speech(""));
        assert!(is_non_speech("  "));
        assert!(is_non_speech("[BLANK_AUDIO]"));
        assert!(is_non_speech("(upbeat music)"));
        assert!(!is_non_speech("hello"));
        assert!(!is_non_speech("[partial"));
    }
}
