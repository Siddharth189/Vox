use std::path::Path;
use std::time::Instant;

use crate::config::Settings;
use crate::context::{self, AppDetector};
use crate::error::{Result, VoxError};
use crate::inject::{AutoPasteInjector, ClipboardInjector, TextInjector};
use crate::model::{
    elapsed_ms, AppContext, AudioChunk, EnrichedRequest, Format, InjectionResult, LatencyMetrics,
    Mode, ProcessedText, Transcript,
};
use crate::privacy::PrivacyPolicy;
use crate::process::{join_segments, CleanupProcessor, Segment, TextProcessor, VoiceCommands};
use crate::stt::{self, LocalWhisper, StreamingSession, Transcriber};

/// The real pipeline: platform app detection, local Whisper, LLM-or-local
/// cleanup, and auto-paste (or clipboard-only). Loads and warms up the
/// Whisper model, so call it off the UI thread.
pub fn build_live_pipeline(settings: &Settings, model_path: &Path) -> Result<Pipeline> {
    let transcriber = LocalWhisper::new(model_path)?
        .with_input_language(settings.input_language.clone())
        .with_initial_prompt(&settings.custom_dictionary);
    transcriber.warm_up();

    let processor = CleanupProcessor::from_settings(settings)?;
    let injector: Box<dyn TextInjector> = if settings.auto_paste {
        Box::new(AutoPasteInjector)
    } else {
        Box::new(ClipboardInjector)
    };
    Ok(Pipeline::new(
        context::platform_detector(settings),
        Box::new(transcriber),
        Box::new(processor),
        injector,
    )
    .with_commands(VoiceCommands::new(
        settings.voice_commands,
        &settings.snippets,
    )))
}

pub struct Pipeline {
    pub detector: Box<dyn AppDetector>,
    pub transcriber: Box<dyn Transcriber>,
    pub processor: Box<dyn TextProcessor>,
    pub injector: Box<dyn TextInjector>,
    pub privacy: PrivacyPolicy,
    /// "new line" / "scratch that" and snippet expansion.
    pub commands: VoiceCommands,
}

#[derive(Debug, Clone)]
pub struct PreparedReport {
    pub app_name: String,
    pub ctx: AppContext,
    pub raw_text: String,
    pub model_text: Option<String>,
    pub processed_text: String,
    pub latency: LatencyMetrics,
    /// Length of the recording, for usage stats.
    pub audio_ms: u64,
    /// Whisper's mean token probability for the transcript, when known.
    pub confidence: Option<f32>,
    /// Speech that was thrown away rather than inserted, e.g. "Arabic
    /// script" for speech Whisper decoded in a language the user doesn't
    /// use. With empty `processed_text`, the whole dictation; otherwise a
    /// part of a streamed one.
    pub rejected: Option<String>,
}

/// The speech of one dictation: a finished recording, or a transcription
/// that ran while it was recorded.
pub enum Speech {
    Clip(AudioChunk),
    Streamed(StreamingSession),
}

/// One edit by voice, done.
#[derive(Debug, Clone)]
pub struct EditReport {
    pub ctx: AppContext,
    /// What the user said to do.
    pub instruction: String,
    /// Length of the text that was selected (0: nothing was, and the
    /// result was inserted at the cursor). The text itself isn't kept.
    pub selection_chars: usize,
    /// What replaced the selection.
    pub text: String,
    pub injection: InjectionResult,
    pub latency: LatencyMetrics,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct PipelineReport {
    pub prepared: PreparedReport,
    pub injection: InjectionResult,
    pub latency: LatencyMetrics,
}

impl Pipeline {
    pub fn new(
        detector: Box<dyn AppDetector>,
        transcriber: Box<dyn Transcriber>,
        processor: Box<dyn TextProcessor>,
        injector: Box<dyn TextInjector>,
    ) -> Self {
        Self {
            detector,
            transcriber,
            processor,
            injector,
            privacy: PrivacyPolicy,
            commands: VoiceCommands::new(true, &[]),
        }
    }

    pub fn with_commands(mut self, commands: VoiceCommands) -> Self {
        self.commands = commands;
        self
    }

    pub fn run(&self, audio: &AudioChunk, mode: Mode) -> Result<PipelineReport> {
        let ctx = self.detector.current()?;
        self.run_with_ctx(audio, ctx, mode)
    }

    pub fn run_with_ctx(
        &self,
        audio: &AudioChunk,
        ctx: AppContext,
        mode: Mode,
    ) -> Result<PipelineReport> {
        let prepared = self.prepare_with_ctx(audio, ctx, mode)?;
        let inject_start = Instant::now();
        let injection = self.inject_processed_text(&prepared.processed_text, &prepared.ctx)?;
        let inject_ms = elapsed_ms(inject_start.elapsed());
        let latency = prepared.latency.clone().with_injection(inject_ms);
        Ok(PipelineReport {
            prepared,
            injection,
            latency,
        })
    }

    pub fn prepare_with_ctx(
        &self,
        audio: &AudioChunk,
        ctx: AppContext,
        mode: Mode,
    ) -> Result<PreparedReport> {
        let privacy_start = Instant::now();
        self.privacy.ensure_allowed(&ctx)?;
        let privacy_ms = elapsed_ms(privacy_start.elapsed());

        let transcribe_start = Instant::now();
        let transcript = self.transcriber.transcribe(audio, &ctx)?;
        let transcribe_ms = elapsed_ms(transcribe_start.elapsed());
        let audio_ms = (audio.duration_secs() * 1000.0).round() as u64;
        let latency =
            LatencyMetrics::prepare_only(privacy_ms, transcribe_ms, 0).with_audio(audio_ms);
        Ok(self.process_transcript(transcript, ctx, mode, latency))
    }

    /// Like `prepare_with_ctx`, for a dictation transcribed while it was
    /// recorded. Call once capture has stopped; `ctx` is the app focused
    /// now. The privacy switch is checked again first: if the user moved to
    /// a disabled app mid-dictation, the session is dropped (cancelled) and
    /// its text never goes anywhere. `transcribe_ms` is the wait for the
    /// rest of the audio, which is what the user actually waits through;
    /// `stt_work_ms` is Whisper's total time, for speed tracking.
    pub fn prepare_streamed(
        &self,
        session: StreamingSession,
        ctx: AppContext,
        mode: Mode,
    ) -> Result<PreparedReport> {
        let privacy_start = Instant::now();
        self.privacy.ensure_allowed(&ctx)?;
        let privacy_ms = elapsed_ms(privacy_start.elapsed());

        let transcribe_start = Instant::now();
        let outcome = session.finish()?;
        let transcribe_ms = elapsed_ms(transcribe_start.elapsed());
        tracing::debug!(
            "streamed stt: {} segments ({} while speaking), fell back = {}, \
             whisper {} ms for {} ms of speech",
            outcome.segments,
            outcome.segments_while_speaking,
            outcome.fell_back,
            outcome.work_ms,
            outcome.audio_ms
        );
        let mut latency =
            LatencyMetrics::prepare_only(privacy_ms, transcribe_ms, 0).with_audio(outcome.audio_ms);
        latency.streamed_segments = outcome.segments_while_speaking;
        latency.stt_work_ms = outcome.work_ms;
        Ok(self.process_transcript(outcome.transcript, ctx, mode, latency))
    }

    /// Voice commands and cleanup on a finished transcript. `latency`
    /// arrives with the privacy and transcription stages filled in.
    fn process_transcript(
        &self,
        transcript: Transcript,
        ctx: AppContext,
        mode: Mode,
        latency: LatencyMetrics,
    ) -> PreparedReport {
        let raw_text = transcript.text.clone();
        let audio_ms = latency.audio_ms;

        let confidence = transcript.confidence;

        if stt::is_non_speech(&raw_text) {
            return PreparedReport {
                app_name: ctx.app_name.clone(),
                ctx,
                raw_text,
                model_text: None,
                processed_text: String::new(),
                latency,
                audio_ms,
                confidence,
                rejected: transcript.rejected,
            };
        }

        let process_start = Instant::now();
        let processed = self.process_segments(&transcript, &ctx, mode);
        let process_ms = elapsed_ms(process_start.elapsed());

        let mut latency = latency;
        latency.process_ms = process_ms;
        latency.total_ms = latency.privacy_ms + latency.transcribe_ms + process_ms;
        PreparedReport {
            app_name: ctx.app_name.clone(),
            ctx,
            raw_text,
            model_text: processed.model_text.clone(),
            processed_text: processed.text,
            latency,
            audio_ms,
            confidence,
            // Set with text too when part of a streamed dictation was lost.
            rejected: transcript.rejected,
        }
    }

    /// Whether dictations are going through the LLM right now: `None` when
    /// LLM cleanup is off, `Some(false)` when it's on but unreachable.
    pub fn llm_status(&self) -> Option<bool> {
        self.processor.llm_status()
    }

    /// Cleans up the spoken parts of the transcript and splices in what the
    /// voice commands and snippets insert verbatim. A processor failure
    /// falls back to the raw words rather than losing the dictation.
    fn process_segments(
        &self,
        transcript: &Transcript,
        ctx: &AppContext,
        mode: Mode,
    ) -> ProcessedText {
        let process = |text: &str| {
            let req = EnrichedRequest {
                transcript: Transcript {
                    text: text.to_string(),
                    language: transcript.language.clone(),
                    ..Transcript::default()
                },
                ctx: ctx.clone(),
                mode,
            };
            self.processor
                .process(&req)
                .unwrap_or_else(|_| ProcessedText::new(text))
        };

        let segments = self.commands.split(&transcript.text, ctx.profile.format);
        if let [Segment::Speech(only)] = segments.as_slice() {
            return process(only);
        }

        let mut pieces = Vec::with_capacity(segments.len());
        let mut model_pieces = Vec::new();
        for segment in &segments {
            match segment {
                Segment::Speech(text) => {
                    let p = process(text);
                    if let Some(m) = p.model_text {
                        model_pieces.push((false, m));
                    }
                    pieces.push((false, p.text));
                }
                Segment::Literal(text) => {
                    model_pieces.push((true, text.clone()));
                    pieces.push((true, text.clone()));
                }
            }
        }
        let text = join_segments(&pieces);
        if model_pieces.iter().any(|(literal, _)| !literal) {
            ProcessedText::with_model_text(text, join_segments(&model_pieces))
        } else {
            ProcessedText::new(text)
        }
    }

    /// Edit by voice: the selection in the focused app is rewritten as the
    /// spoken instruction says. The selection is copied first, while that
    /// app still has focus; then the instruction is transcribed, the LLM
    /// rewrites, and the result is pasted over the selection. Whatever
    /// fails, the user's clipboard is given back and nothing is pasted.
    pub fn edit(&self, speech: Speech, ctx: AppContext) -> Result<EditReport> {
        let privacy_start = Instant::now();
        self.privacy.ensure_allowed(&ctx)?;
        // Ctrl+C interrupts the running program in a terminal.
        if ctx.profile.format == Format::Shell {
            return Err(VoxError::Injection(
                "edit by voice is off in terminals".into(),
            ));
        }
        let privacy_ms = elapsed_ms(privacy_start.elapsed());

        let copy_start = Instant::now();
        let target = self.injector.begin_edit(&ctx)?;
        let copy_ms = elapsed_ms(copy_start.elapsed());

        let transcribe_start = Instant::now();
        let heard = match self.transcribe_speech(speech, &ctx) {
            Ok(t) => t,
            Err(e) => {
                self.injector.abort_edit(target);
                return Err(e);
            }
        };
        let transcribe_ms = elapsed_ms(transcribe_start.elapsed());
        let instruction = heard.transcript.text.trim().to_string();
        if stt::is_non_speech(&instruction) {
            self.injector.abort_edit(target);
            return Err(VoxError::Transcription(
                "didn't catch the instruction".into(),
            ));
        }

        let process_start = Instant::now();
        let text = match self.processor.edit(&instruction, &target.selection, &ctx) {
            Ok(text) => text,
            Err(e) => {
                self.injector.abort_edit(target);
                return Err(e);
            }
        };
        let process_ms = elapsed_ms(process_start.elapsed());

        let selection_chars = target.selection.chars().count();
        let inject_start = Instant::now();
        let injection = self.injector.finish_edit(target, &text, &ctx)?;
        let inject_ms = copy_ms + elapsed_ms(inject_start.elapsed());

        let mut latency = LatencyMetrics::prepare_only(privacy_ms, transcribe_ms, process_ms)
            .with_audio(heard.audio_ms)
            .with_injection(inject_ms);
        latency.streamed_segments = heard.streamed_segments;
        latency.stt_work_ms = heard.stt_work_ms;
        Ok(EditReport {
            ctx,
            instruction,
            selection_chars,
            text,
            injection,
            latency,
            confidence: heard.transcript.confidence,
        })
    }

    /// Transcribes `speech` as is: no voice commands, no cleanup.
    fn transcribe_speech(&self, speech: Speech, ctx: &AppContext) -> Result<Heard> {
        match speech {
            Speech::Clip(audio) => Ok(Heard {
                transcript: self.transcriber.transcribe(&audio, ctx)?,
                audio_ms: (audio.duration_secs() * 1000.0).round() as u64,
                streamed_segments: 0,
                stt_work_ms: 0,
            }),
            Speech::Streamed(session) => {
                let outcome = session.finish()?;
                Ok(Heard {
                    transcript: outcome.transcript,
                    audio_ms: outcome.audio_ms,
                    streamed_segments: outcome.segments_while_speaking,
                    stt_work_ms: outcome.work_ms,
                })
            }
        }
    }

    pub fn inject_processed_text(
        &self,
        processed_text: &str,
        ctx: &AppContext,
    ) -> Result<InjectionResult> {
        if processed_text.is_empty() {
            return Ok(InjectionResult {
                injected_chars: 0,
                strategy: "skipped".into(),
            });
        }
        self.injector.inject(processed_text, ctx)
    }
}

/// A transcription and what it took.
struct Heard {
    transcript: Transcript,
    audio_ms: u64,
    streamed_segments: u32,
    stt_work_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::StubAppDetector;
    use crate::inject::FakeTextInjector;
    use crate::model::{AppProfile, Format, Privacy};
    use crate::process::FakeTextProcessor;
    use crate::stt::FakeTranscriber;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn ctx_with_privacy(privacy: Privacy) -> AppContext {
        AppContext {
            bundle_id: "com.example.app".into(),
            app_name: "Example".into(),
            window_title: None,
            profile: AppProfile {
                format: Format::CleanProse,
                privacy,
            },
        }
    }

    fn audio() -> AudioChunk {
        AudioChunk {
            samples: vec![0.1; 1600],
            sample_rate: 16_000,
        }
    }

    struct CountingTranscriber(Arc<AtomicUsize>);
    impl Transcriber for CountingTranscriber {
        fn transcribe(&self, _: &AudioChunk, _: &AppContext) -> Result<crate::model::Transcript> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(crate::model::Transcript::new("x"))
        }
        fn name(&self) -> &'static str {
            "counting"
        }
    }

    #[test]
    fn privacy_blocks_before_transcription() {
        let counting = Arc::new(AtomicUsize::new(0));
        let pipeline = Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::Disabled))),
            Box::new(CountingTranscriber(counting.clone())),
            Box::new(FakeTextProcessor::passthrough()),
            Box::new(FakeTextInjector::new()),
        );
        let err = pipeline
            .prepare_with_ctx(&audio(), ctx_with_privacy(Privacy::Disabled), Mode::Auto)
            .unwrap_err();
        assert!(matches!(err, crate::error::VoxError::PrivacyBlocked(_)));
        assert_eq!(counting.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn non_speech_skips_process_and_inject() {
        let pipeline = Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::LocalOnly))),
            Box::new(FakeTranscriber::new("[BLANK_AUDIO]")),
            Box::new(FakeTextProcessor::returning("hallucinated")),
            Box::new(FakeTextInjector::new()),
        );
        let report = pipeline.run(&audio(), Mode::Auto).unwrap();
        assert!(report.prepared.processed_text.is_empty());
        assert_eq!(report.injection.strategy, "skipped");
    }

    #[test]
    fn streamed_dictation_is_processed_like_a_clip() {
        let pipeline = Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::LocalOnly))),
            Box::new(FakeTranscriber::new("unused")),
            Box::new(FakeTextProcessor::returning("Cleaned.")),
            Box::new(FakeTextInjector::new()),
        );
        let tap = crate::audio::LiveTap::new(16_000);
        tap.push(&vec![0.2; 16_000 * 2]);
        let session = StreamingSession::start(tap, |_: &AudioChunk, _: &stt::SegmentHint| {
            Ok(Transcript::new("streamed words"))
        });
        let report = pipeline
            .prepare_streamed(session, ctx_with_privacy(Privacy::LocalOnly), Mode::Auto)
            .unwrap();
        assert_eq!(report.raw_text, "streamed words");
        assert_eq!(report.processed_text, "Cleaned.");
        assert_eq!(report.audio_ms, 2_000);
        assert_eq!(report.latency.audio_ms, 2_000);
        assert_eq!(
            report.latency.total_ms,
            report.latency.privacy_ms + report.latency.transcribe_ms + report.latency.process_ms
        );
    }

    #[test]
    fn partly_lost_streamed_dictation_is_inserted_and_flagged() {
        let pipeline = Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::LocalOnly))),
            Box::new(FakeTranscriber::new("unused")),
            Box::new(FakeTextProcessor::passthrough()),
            Box::new(FakeTextInjector::new()),
        );
        let tap = crate::audio::LiveTap::new(16_000);
        tap.push(&vec![0.2; 16_000 * 2]);
        let session = StreamingSession::start(tap, |_: &AudioChunk, _: &stt::SegmentHint| {
            Ok(Transcript {
                text: "what was caught".into(),
                rejected: Some("Arabic script".into()),
                ..Transcript::default()
            })
        });
        let report = pipeline
            .prepare_streamed(session, ctx_with_privacy(Privacy::LocalOnly), Mode::Auto)
            .unwrap();
        assert_eq!(report.processed_text, "what was caught");
        assert_eq!(report.rejected.as_deref(), Some("Arabic script"));
    }

    #[test]
    fn streamed_dictation_into_disabled_app_is_discarded() {
        let pipeline = Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::LocalOnly))),
            Box::new(FakeTranscriber::new("unused")),
            Box::new(FakeTextProcessor::passthrough()),
            Box::new(FakeTextInjector::new()),
        );
        let tap = crate::audio::LiveTap::new(16_000);
        tap.push(&vec![0.2; 16_000 * 2]);
        let session = StreamingSession::start(tap, |_: &AudioChunk, _: &stt::SegmentHint| {
            Ok(Transcript::new("secret"))
        });
        let err = pipeline
            .prepare_streamed(session, ctx_with_privacy(Privacy::Disabled), Mode::Auto)
            .unwrap_err();
        assert!(matches!(err, crate::error::VoxError::PrivacyBlocked(_)));
    }

    #[test]
    fn commands_and_snippets_survive_cleanup() {
        use crate::config::Snippet;
        use crate::process::LocalCleanup;
        use std::collections::HashMap;

        let pipeline = Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::LocalOnly))),
            Box::new(FakeTranscriber::new(
                "hi sam, new paragraph. um, lets meet tuesday. scratch that. \
                 lets meet wednesday, book at my link.",
            )),
            Box::new(LocalCleanup::new(Vec::new(), HashMap::new())),
            Box::new(FakeTextInjector::new()),
        )
        .with_commands(VoiceCommands::new(
            true,
            &[Snippet {
                trigger: "my link".into(),
                text: "https://cal.example/sam".into(),
            }],
        ));
        let report = pipeline.run(&audio(), Mode::Auto).unwrap();
        assert_eq!(
            report.prepared.processed_text,
            "Hi sam,\n\nLets meet wednesday, book at https://cal.example/sam."
        );
        assert_eq!(report.prepared.audio_ms, 100);
    }

    #[test]
    fn llm_failure_falls_back_to_raw() {
        let pipeline = Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::LocalOnly))),
            Box::new(FakeTranscriber::new("raw dictation text")),
            Box::new(FakeTextProcessor::failing()),
            Box::new(FakeTextInjector::new()),
        );
        let report = pipeline.run(&audio(), Mode::Auto).unwrap();
        assert_eq!(report.prepared.processed_text, "raw dictation text");
        assert_eq!(
            report.injection.injected_chars,
            "raw dictation text".chars().count()
        );
    }

    /// Records what an edit did to the clipboard and the app.
    struct SpyInjector {
        selection: &'static str,
        events: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl SpyInjector {
        fn new(selection: &'static str) -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
            let events = Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    selection,
                    events: events.clone(),
                },
                events,
            )
        }

        fn log(&self, event: String) {
            self.events.lock().unwrap().push(event);
        }
    }

    impl TextInjector for SpyInjector {
        fn inject(&self, text: &str, _: &AppContext) -> Result<InjectionResult> {
            self.log(format!("paste {text}"));
            Ok(InjectionResult {
                injected_chars: text.chars().count(),
                strategy: "spy".into(),
            })
        }
        fn name(&self) -> &'static str {
            "spy"
        }
        fn begin_edit(&self, _: &AppContext) -> Result<crate::inject::EditTarget> {
            self.log("copy".into());
            Ok(crate::inject::EditTarget::detached(self.selection))
        }
        fn finish_edit(
            &self,
            _: crate::inject::EditTarget,
            text: &str,
            ctx: &AppContext,
        ) -> Result<InjectionResult> {
            self.inject(text, ctx)
        }
        fn abort_edit(&self, _: crate::inject::EditTarget) {
            self.log("restore".into());
        }
    }

    fn ctx_with_format(format: Format) -> AppContext {
        AppContext {
            profile: AppProfile {
                format,
                privacy: Privacy::LocalOnly,
            },
            ..ctx_with_privacy(Privacy::LocalOnly)
        }
    }

    fn edit_pipeline(
        transcriber: Box<dyn Transcriber>,
        processor: FakeTextProcessor,
        injector: SpyInjector,
    ) -> Pipeline {
        Pipeline::new(
            Box::new(StubAppDetector::new(ctx_with_privacy(Privacy::LocalOnly))),
            transcriber,
            Box::new(processor),
            Box::new(injector),
        )
    }

    #[test]
    fn edit_copies_first_then_pastes_the_rewrite_over_the_selection() {
        let (spy, events) = SpyInjector::new("send me the report now");
        let pipeline = edit_pipeline(
            Box::new(FakeTranscriber::new(" make it polite ")),
            FakeTextProcessor::passthrough(),
            spy,
        );
        let report = pipeline
            .edit(Speech::Clip(audio()), ctx_with_format(Format::CleanProse))
            .unwrap();
        assert_eq!(report.instruction, "make it polite");
        assert_eq!(report.selection_chars, "send me the report now".len());
        assert_eq!(report.text, "make it polite: send me the report now");
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "copy".to_string(),
                "paste make it polite: send me the report now".to_string()
            ]
        );
    }

    #[test]
    fn edit_with_nothing_selected_inserts_what_was_asked_for() {
        let (spy, _) = SpyInjector::new("");
        let pipeline = edit_pipeline(
            Box::new(FakeTranscriber::new("write a thank you note")),
            FakeTextProcessor::returning("Thank you!"),
            spy,
        );
        let report = pipeline
            .edit(Speech::Clip(audio()), ctx_with_format(Format::Casual))
            .unwrap();
        assert_eq!(report.selection_chars, 0);
        assert_eq!(report.text, "Thank you!");
    }

    #[test]
    fn edit_is_refused_in_terminals_and_private_apps_before_listening() {
        for (ctx, privacy_error) in [
            (ctx_with_format(Format::Shell), false),
            (ctx_with_privacy(Privacy::Disabled), true),
        ] {
            let counting = Arc::new(AtomicUsize::new(0));
            let (spy, events) = SpyInjector::new("text");
            let pipeline = edit_pipeline(
                Box::new(CountingTranscriber(counting.clone())),
                FakeTextProcessor::passthrough(),
                spy,
            );
            let err = pipeline.edit(Speech::Clip(audio()), ctx).unwrap_err();
            assert_eq!(
                matches!(err, crate::error::VoxError::PrivacyBlocked(_)),
                privacy_error
            );
            assert_eq!(counting.load(Ordering::SeqCst), 0);
            assert!(
                events.lock().unwrap().is_empty(),
                "nothing copied or pasted"
            );
        }
    }

    #[test]
    fn failed_edit_gives_the_clipboard_back_and_pastes_nothing() {
        // The LLM fails, or the instruction was silence.
        let cases: [(Box<dyn Transcriber>, FakeTextProcessor); 2] = [
            (
                Box::new(FakeTranscriber::new("make it shorter")),
                FakeTextProcessor::failing(),
            ),
            (
                Box::new(FakeTranscriber::new("[BLANK_AUDIO]")),
                FakeTextProcessor::passthrough(),
            ),
        ];
        for (transcriber, processor) in cases {
            let (spy, events) = SpyInjector::new("some text");
            let pipeline = edit_pipeline(transcriber, processor, spy);
            assert!(pipeline
                .edit(Speech::Clip(audio()), ctx_with_format(Format::CleanProse))
                .is_err());
            assert_eq!(
                *events.lock().unwrap(),
                vec!["copy".to_string(), "restore".to_string()]
            );
        }
    }
}
