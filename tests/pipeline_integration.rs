use vox::context::StubAppDetector;
use vox::inject::FakeTextInjector;
use vox::model::{AppContext, AppProfile, AudioChunk, Format, Mode, Privacy};
use vox::pipeline::Pipeline;
use vox::process::FakeTextProcessor;
use vox::stt::FakeTranscriber;

fn ctx(privacy: Privacy) -> AppContext {
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

fn silence() -> AudioChunk {
    AudioChunk {
        samples: vec![0.0; 1600],
        sample_rate: 16_000,
    }
}

#[test]
fn end_to_end_fake_pipeline_injects_cleaned_text() {
    let pipeline = Pipeline::new(
        Box::new(StubAppDetector::new(ctx(Privacy::LocalOnly))),
        Box::new(FakeTranscriber::new("hey there um this is a test")),
        Box::new(FakeTextProcessor::returning("Hey there, this is a test.")),
        Box::new(FakeTextInjector::new()),
    );
    let report = pipeline.run(&silence(), Mode::Auto).unwrap();
    assert_eq!(report.prepared.processed_text, "Hey there, this is a test.");
    assert_eq!(report.injection.strategy, "fake");
    assert!(report.latency.total_ms >= report.latency.privacy_ms);
}

#[test]
fn disabled_app_never_reaches_transcriber() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use vox::error::Result;
    use vox::model::Transcript;
    use vox::stt::Transcriber;

    struct FlagTranscriber(Arc<AtomicBool>);
    impl Transcriber for FlagTranscriber {
        fn transcribe(&self, _: &AudioChunk, _: &AppContext) -> Result<Transcript> {
            self.0.store(true, Ordering::SeqCst);
            Ok(Transcript::new("nope"))
        }
        fn name(&self) -> &'static str {
            "flag"
        }
    }

    let called = Arc::new(AtomicBool::new(false));
    let pipeline = Pipeline::new(
        Box::new(StubAppDetector::new(ctx(Privacy::Disabled))),
        Box::new(FlagTranscriber(called.clone())),
        Box::new(FakeTextProcessor::passthrough()),
        Box::new(FakeTextInjector::new()),
    );
    assert!(pipeline.run(&silence(), Mode::Auto).is_err());
    assert!(!called.load(Ordering::SeqCst));
}
