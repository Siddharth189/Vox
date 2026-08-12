use std::time::Instant;

use crate::context::AppDetector;
use crate::error::Result;
use crate::inject::TextInjector;
use crate::model::{
    elapsed_ms, AppContext, AudioChunk, EnrichedRequest, InjectionResult, LatencyMetrics, Mode,
    ProcessedText,
};
use crate::privacy::PrivacyPolicy;
use crate::process::TextProcessor;
use crate::stt::{self, Transcriber};

pub struct Pipeline {
    pub detector: Box<dyn AppDetector>,
    pub transcriber: Box<dyn Transcriber>,
    pub processor: Box<dyn TextProcessor>,
    pub injector: Box<dyn TextInjector>,
    pub privacy: PrivacyPolicy,
}

#[derive(Debug, Clone)]
pub struct PreparedReport {
    pub app_name: String,
    pub ctx: AppContext,
    pub raw_text: String,
    pub model_text: Option<String>,
    pub processed_text: String,
    pub latency: LatencyMetrics,
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
        }
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

        if stt::is_non_speech(&raw_text) {
            return Ok(PreparedReport {
                app_name: ctx.app_name.clone(),
                ctx,
                raw_text,
                model_text: None,
                processed_text: String::new(),
                latency: LatencyMetrics::prepare_only(privacy_ms, transcribe_ms, 0),
            });
        }

        let process_start = Instant::now();
        let req = EnrichedRequest {
            transcript,
            ctx: ctx.clone(),
            mode,
        };
        let processed = match self.processor.process(&req) {
            Ok(p) => p,
            Err(_) => ProcessedText::new(raw_text.clone()),
        };
        let process_ms = elapsed_ms(process_start.elapsed());

        Ok(PreparedReport {
            app_name: ctx.app_name.clone(),
            ctx,
            raw_text,
            model_text: processed.model_text.clone(),
            processed_text: processed.text,
            latency: LatencyMetrics::prepare_only(privacy_ms, transcribe_ms, process_ms),
        })
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
        fn transcribe(
            &self,
            _: &AudioChunk,
            _: &AppContext,
        ) -> Result<crate::model::Transcript> {
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
