use std::path::Path;
use std::time::Instant;

use crate::config::Settings;
use crate::context::{self, AppDetector};
use crate::error::Result;
use crate::inject::{AutoPasteInjector, ClipboardInjector, TextInjector};
use crate::model::{
    elapsed_ms, AppContext, AudioChunk, EnrichedRequest, InjectionResult, LatencyMetrics, Mode,
    ProcessedText, Transcript,
};
use crate::privacy::PrivacyPolicy;
use crate::process::{join_segments, CleanupProcessor, Segment, TextProcessor, VoiceCommands};
use crate::stt::{self, LocalWhisper, Transcriber};

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
        let raw_text = transcript.text.clone();
        let audio_ms = (audio.duration_secs() * 1000.0).round() as u64;

        if stt::is_non_speech(&raw_text) {
            return Ok(PreparedReport {
                app_name: ctx.app_name.clone(),
                ctx,
                raw_text,
                model_text: None,
                processed_text: String::new(),
                latency: LatencyMetrics::prepare_only(privacy_ms, transcribe_ms, 0),
                audio_ms,
            });
        }

        let process_start = Instant::now();
        let processed = self.process_segments(&transcript, &ctx, mode);
        let process_ms = elapsed_ms(process_start.elapsed());

        Ok(PreparedReport {
            app_name: ctx.app_name.clone(),
            ctx,
            raw_text,
            model_text: processed.model_text.clone(),
            processed_text: processed.text,
            latency: LatencyMetrics::prepare_only(privacy_ms, transcribe_ms, process_ms),
            audio_ms,
        })
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
}
